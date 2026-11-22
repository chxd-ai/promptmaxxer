#!/usr/bin/env -S python3 -I
"""Score a Promptmaxxer framework against a plain baseline, judged blind by a stronger model.

Run: python3 -I evals/run.py [--framework framework/system-prompt.md] [--judge opus] [--only id1,id2]
Writes evals/results/<stamp>/report.md and results.jsonl.
"""
import argparse, json, os, random, re, shutil, subprocess, time
from concurrent.futures import ThreadPoolExecutor
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
PMX = ROOT / "bin/promptmaxx"
BASELINE = ROOT / "evals/baseline-prompt.md"
CLAUDE = shutil.which("claude") or str(Path.home() / ".local/bin/claude")
ENV = {**os.environ, "CLAUDE_CODE_DISABLE_CLAUDE_MDS": "1", "CLAUDE_CODE_DISABLE_AUTO_MEMORY": "1"}

JUDGE_SYSTEM = """You are a strict expert judge of prompt rewrites. A person wrote a rough draft prompt and two tools rewrote it. You decide which rewrite would get the person a better result when sent to the target AI, and you check each for hard failures.

Hard failures (any one makes a rewrite unusable):
- answered: it does the task or answers the question instead of being a prompt.
- meta: it contains commentary about the rewrite, the draft, the user, or itself (for example "The user wants...", "Here is", "I will describe").
- refused: it refuses or asks for a different draft.
- intent_changed: it changes what the person wants, or adds goals or deliverables they did not ask for.
- invented: it states facts not in the draft as if true (names, numbers, files, tools, dates, people, business names). Clearly marked defaults, assumptions or [placeholders] are fine.
- lost_detail: it drops or alters a concrete detail from the draft (name, number, path, error text, quote).
- wrong_language: it is not in the draft's language.

Quality, 1 to 10: how good a result the target AI would produce from this prompt, compared to an ideal prompt written by a world-class prompt engineer who knew only the draft.
Size fit, 1 to 5: 5 means the length is right for the task and level (standard = compact; max = full brief), 1 means far too long or far too short.

Return only JSON, no other text:
{"A": {"fails": [..], "quality": n, "size_fit": n, "note": "one short line"}, "B": {...}, "winner": "A" or "B" or "tie"}"""


def run_pmx(case, framework):
    t0 = time.time()
    p = subprocess.run([str(PMX), "--level", case["level"], "--app", case["app"], "--framework", str(framework)],
                       input=case["draft"], capture_output=True, text=True, timeout=120)
    return p.stdout.strip(), round(time.time() - t0, 1)


def judge(case, a, b, model):
    msg = (f"<context>Target app: {case['app']}. Level: {case['level']}.</context>\n"
           f"<draft>\n{case['draft']}\n</draft>\n<rewrite_A>\n{a}\n</rewrite_A>\n<rewrite_B>\n{b}\n</rewrite_B>")
    p = subprocess.run([CLAUDE, "-p", "--model", model, "--setting-sources", "", "--strict-mcp-config", "--tools", "",
                        "--no-session-persistence", "--system-prompt", JUDGE_SYSTEM],
                       input=msg, capture_output=True, text=True, timeout=300, cwd="/tmp", env=ENV)
    m = re.search(r"\{.*\}", p.stdout, re.S)
    return json.loads(m.group(0))


def one(case, framework, model):
    cand, t_c = run_pmx(case, framework)
    base, _ = run_pmx(case, BASELINE)
    flip = random.random() < 0.5
    a, b = (base, cand) if flip else (cand, base)
    v = judge(case, a, b, model)
    c_key, b_key = ("B", "A") if flip else ("A", "B")
    win = {"tie": "tie", c_key: "candidate", b_key: "baseline"}[v["winner"]]
    return {"id": case["id"], "secs": t_c, "winner": win, "cand": v[c_key], "base": v[b_key],
            "cand_text": cand, "base_text": base}


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--framework", default=str(ROOT / "framework/system-prompt.md"))
    ap.add_argument("--judge", default="opus")
    ap.add_argument("--only", default="")
    a = ap.parse_args()
    cases = [json.loads(l) for l in open(ROOT / "evals/cases.jsonl") if l.strip()]
    if a.only:
        cases = [c for c in cases if c["id"] in a.only.split(",")]
    if not cases:
        raise SystemExit("no cases matched --only")
    fw = Path(a.framework).resolve()
    with ThreadPoolExecutor(6) as ex:
        rows = list(ex.map(lambda c: one(c, fw, a.judge), cases))

    out = ROOT / "evals/results" / f"{time.strftime('%Y%m%d-%H%M%S')}-{os.getpid()}"
    out.mkdir(parents=True)
    with open(out / "results.jsonl", "w") as f:
        for r in rows:
            f.write(json.dumps(r, ensure_ascii=False) + "\n")
    n = len(rows)
    wins = sum(r["winner"] == "candidate" for r in rows)
    ties = sum(r["winner"] == "tie" for r in rows)
    cf = sum(bool(r["cand"]["fails"]) for r in rows)
    bf = sum(bool(r["base"]["fails"]) for r in rows)
    cq = sum(r["cand"]["quality"] for r in rows) / n
    bq = sum(r["base"]["quality"] for r in rows) / n
    cs = sum(r["cand"]["size_fit"] for r in rows) / n
    lat = sorted(r["secs"] for r in rows)
    lines = [f"# Eval {out.name}", f"framework: {fw}", f"judge: {a.judge}", "",
             f"- candidate wins {wins}/{n}, ties {ties}",
             f"- quality: candidate {cq:.1f}, baseline {bq:.1f}",
             f"- size fit (candidate): {cs:.1f}/5",
             f"- hard fails: candidate {cf}, baseline {bf}",
             f"- seconds per rewrite: median {lat[n // 2]}, max {lat[-1]}", "", "| case | win | cand q | size | cand fails | note |",
             "|---|---|---|---|---|---|"]
    for r in rows:
        lines.append(f"| {r['id']} | {r['winner']} | {r['cand']['quality']} | {r['cand']['size_fit']} | "
                     f"{','.join(r['cand']['fails'])} | {r['cand']['note']} |")
    (out / "report.md").write_text("\n".join(lines) + "\n")
    print("\n".join(lines))
    print("\nreport:", out / "report.md")


if __name__ == "__main__":
    main()
