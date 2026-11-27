//! The CLI engines' process handling, with small shell scripts standing in for the CLIs.
//! No model calls. Unix only: the stand-ins are `/bin/sh` scripts.
#![cfg(unix)]

use pmx_engines::{complete_with, run_with, EngineError, EngineId, Event, Level, Request, RunOptions};
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::time::{Duration, Instant};
use tokio_util::sync::CancellationToken;

fn script(name: &str, body: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("pmx-fake-cli-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let p = dir.join(name);
    std::fs::write(&p, format!("#!/bin/sh\n{body}\n")).unwrap();
    std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
    p
}

fn req() -> Request {
    Request {
        draft: "the login page is broken fix it".into(),
        level: Level::Standard,
        app_name: "Ghostty".into(),
        target: None,
        context_brief: None,
        framework: "FRAMEWORK TEXT".into(),
    }
}

fn opts(cli: PathBuf) -> RunOptions {
    RunOptions { cli: Some(cli), ..Default::default() }
}

#[tokio::test]
async fn claude_stream_json_and_args() {
    // Echo the args and stdin to files, then print a canned stream.
    let dir = std::env::temp_dir().join(format!("pmx-fake-cli-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let args_file = dir.join("claude-args.txt");
    let stdin_file = dir.join("claude-stdin.txt");
    let env_file = dir.join("claude-env.txt");
    let cli = script(
        "claude-ok",
        &format!(
            r#"printf '%s\n' "$@" > '{a}'
cat > '{s}'
pwd > '{e}'; echo "MDS=$CLAUDE_CODE_DISABLE_CLAUDE_MDS MEM=$CLAUDE_CODE_DISABLE_AUTO_MEMORY" >> '{e}'
echo '{{"type":"system","subtype":"init"}}'
echo '{{"type":"stream_event","event":{{"type":"content_block_delta","index":0,"delta":{{"type":"text_delta","text":"The login page is broken. "}}}}}}'
echo '{{"type":"stream_event","event":{{"type":"content_block_delta","index":0,"delta":{{"type":"text_delta","text":"Find the cause and fix it."}}}}}}'
echo '{{"type":"result","subtype":"success","is_error":false,"result":"The login page is broken. Find the cause and fix it."}}'"#,
            a = args_file.display(),
            s = stdin_file.display(),
            e = env_file.display()
        ),
    );
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let out = run_with(EngineId::ClaudePlan, req(), tx, CancellationToken::new(), opts(cli)).await.unwrap();
    assert_eq!(out.text, "The login page is broken. Find the cause and fix it.");
    assert_eq!(out.model, "haiku");

    let args = std::fs::read_to_string(&args_file).unwrap();
    let args: Vec<&str> = args.lines().collect();
    for want in ["-p", "--strict-mcp-config", "--no-session-persistence", "--include-partial-messages", "--verbose", "--disable-slash-commands"] {
        assert!(args.contains(&want), "missing {want}: {args:?}");
    }
    let pos = |k: &str| args.iter().position(|a| *a == k).unwrap();
    assert_eq!(args[pos("--model") + 1], "haiku");
    assert_eq!(args[pos("--setting-sources") + 1], "");
    assert_eq!(args[pos("--tools") + 1], "");
    assert_eq!(args[pos("--output-format") + 1], "stream-json");
    let fw = args[pos("--system-prompt-file") + 1];
    assert_eq!(std::fs::read_to_string(fw).unwrap(), "FRAMEWORK TEXT");
    assert!(std::fs::read_to_string(&stdin_file).unwrap().contains("<draft>\nthe login page is broken fix it\n</draft>"));
    let env = std::fs::read_to_string(&env_file).unwrap();
    let run_dir = std::env::temp_dir().join("promptmaxxer-run");
    let run_dir_real = run_dir.canonicalize().unwrap_or(run_dir.clone());
    assert!(env.starts_with(run_dir.to_str().unwrap()) || env.starts_with(run_dir_real.to_str().unwrap()), "{env}");
    assert!(env.contains("MDS=1 MEM=1"));

    let mut deltas = 0;
    while let Ok(e) = rx.try_recv() {
        deltas += matches!(e, Event::Delta(_)) as usize;
    }
    assert_eq!(deltas, 2);
}

#[tokio::test]
async fn cli_error_text_reaches_the_caller() {
    let cli = script(
        "claude-err",
        r#"cat >/dev/null
echo '{"type":"result","subtype":"success","is_error":true,"result":"Not logged in · Please run /login"}'
exit 1"#,
    );
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let err = run_with(EngineId::ClaudePlan, req(), tx, CancellationToken::new(), opts(cli)).await.unwrap_err();
    assert_eq!(err, EngineError::Cli("Not logged in · Please run /login".into()));
}

fn alive(pid: i32) -> bool {
    unsafe { libc_kill(pid, 0) == 0 }
}
extern "C" {
    #[link_name = "kill"]
    fn libc_kill(pid: i32, sig: i32) -> i32;
}

#[tokio::test]
async fn cancel_kills_the_cli_and_its_children() {
    let dir = std::env::temp_dir().join(format!("pmx-fake-cli-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let pid_file = dir.join("pids.txt");
    let _ = std::fs::remove_file(&pid_file);
    let cli = script(
        "claude-slow",
        &format!(
            r#"cat >/dev/null
sleep 60 &
echo "$$ $!" > '{p}'
echo '{{"type":"stream_event","event":{{"type":"content_block_delta","index":0,"delta":{{"type":"text_delta","text":"partial"}}}}}}'
sleep 60"#,
            p = pid_file.display()
        ),
    );
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let cancel = CancellationToken::new();
    let c2 = cancel.clone();
    let task = tokio::spawn(run_with(EngineId::ClaudePlan, req(), tx, cancel, opts(cli)));
    // Wait for the first delta, then cancel.
    let t = Instant::now();
    loop {
        match tokio::time::timeout(Duration::from_secs(5), rx.recv()).await.unwrap() {
            Some(Event::Delta(d)) => {
                assert_eq!(d, "partial");
                break;
            }
            Some(_) => continue,
            None => panic!("channel closed"),
        }
    }
    c2.cancel();
    let err = task.await.unwrap().unwrap_err();
    assert_eq!(err, EngineError::Cancelled);
    assert!(t.elapsed() < Duration::from_secs(5));
    let pids = std::fs::read_to_string(&pid_file).unwrap();
    let pids: Vec<i32> = pids.split_whitespace().map(|p| p.parse().unwrap()).collect();
    tokio::time::sleep(Duration::from_millis(200)).await;
    for p in pids {
        assert!(!alive(p), "process {p} still running after cancel");
    }
}

#[tokio::test]
async fn timeout_kills_the_cli() {
    let cli = script("claude-hang", "cat >/dev/null\nsleep 60");
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let mut o = opts(cli);
    o.timeout = Some(Duration::from_millis(500));
    let t = Instant::now();
    let err = run_with(EngineId::ClaudePlan, req(), tx, CancellationToken::new(), o).await.unwrap_err();
    assert_eq!(err, EngineError::Timeout(0));
    assert!(t.elapsed() < Duration::from_secs(3));
}

#[tokio::test]
async fn codex_events_through_a_fake_cli() {
    let cli = script(
        "codex-ok",
        r#"cat >/dev/null
[ -n "$CODEX_HOME" ] || exit 3
[ -e "$CODEX_HOME/AGENTS.md" ] && exit 4
echo '{"type":"thread.started","thread_id":"t"}'
echo '{"type":"turn.started"}'
echo '{"type":"item.completed","item":{"id":"item_0","type":"agent_message","text":"The login page is broken. Find the cause and fix it."}}'
echo '{"type":"turn.completed","usage":{"input_tokens":1,"output_tokens":1}}'
sleep 30"#,
    );
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    // Codex lingers after its last event; the engine stops it at turn.completed.
    let t = Instant::now();
    let out = run_with(EngineId::ChatGptPlan, req(), tx, CancellationToken::new(), opts(cli)).await.unwrap();
    assert!(t.elapsed() < Duration::from_secs(5));
    assert_eq!(out.text, "The login page is broken. Find the cause and fix it.");
    assert_eq!(out.model, "gpt-6.2-sol");
}

#[tokio::test]
async fn gemini_takes_the_prompt_on_stdin_not_in_the_arguments() {
    let cli = script(
        "gemini-ok",
        r#"[ -n "$GEMINI_SYSTEM_MD" ] && [ "$(cat "$GEMINI_SYSTEM_MD")" = "FRAMEWORK TEXT" ] || exit 5
case "$*" in *"<draft>"*|*"login page"*) exit 6;; esac
case "$(cat)" in *"<draft>"*) ;; *) exit 7;; esac
echo '{"type":"init","model":"x"}'
echo '{"type":"message","role":"assistant","content":"The login page is broken. ","delta":true}'
echo '{"type":"message","role":"assistant","content":"Find the cause and fix it.","delta":true}'
echo '{"type":"result","status":"success"}'"#,
    );
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let out = run_with(EngineId::GeminiPlan, req(), tx, CancellationToken::new(), opts(cli)).await.unwrap();
    assert_eq!(out.text, "The login page is broken. Find the cause and fix it.");
    assert_eq!(out.model, "flash");
}

#[tokio::test]
async fn complete_sends_the_text_as_is_with_its_own_system_prompt() {
    let dir = std::env::temp_dir().join(format!("pmx-fake-cli-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let args_file = dir.join("complete-args.txt");
    let stdin_file = dir.join("complete-stdin.txt");
    let cli = script(
        "claude-brief",
        &format!(
            r#"printf '%s\n' "$@" > '{a}'
cat > '{s}'
echo '{{"type":"result","subtype":"success","is_error":false,"result":"  - Build: cargo build  "}}'"#,
            a = args_file.display(),
            s = stdin_file.display()
        ),
    );
    let out = complete_with(EngineId::ClaudePlan, Level::Standard, "BRIEF SYSTEM", "### CLAUDE.md\n- Build: cargo build".into(), CancellationToken::new(), opts(cli))
        .await
        .unwrap();
    assert_eq!(out, "- Build: cargo build");
    // No draft wrapper: the user text goes in unchanged.
    assert_eq!(std::fs::read_to_string(&stdin_file).unwrap(), "### CLAUDE.md\n- Build: cargo build");
    let args = std::fs::read_to_string(&args_file).unwrap();
    let fw_path = args.lines().skip_while(|l| !l.contains("system-prompt")).nth(1).unwrap_or("");
    assert!(args.contains("haiku"));
    if !fw_path.is_empty() && std::path::Path::new(fw_path).exists() {
        assert_eq!(std::fs::read_to_string(fw_path).unwrap(), "BRIEF SYSTEM");
    }
}
