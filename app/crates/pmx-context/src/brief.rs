//! The project brief: built from the sources by the caller's model call, cached on disk,
//! and rebuilt only when the sources (or the About me text) change.

use crate::secrets::scrub_secrets;
use crate::sources::{fingerprint, hex, sources};
use crate::{home_dir, Brief, Project, Source, SourceLevel, SourcePrefs};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::future::Future;
use std::io::Write;
use std::path::{Path, PathBuf};

/// Max words in a brief.
pub const BRIEF_MAX_WORDS: usize = 300;
/// Max bytes of source text handed to `summarize` (user-level and earlier files are trimmed first).
pub const SOURCE_TEXT_CAP: usize = 40_000;
const FILE_READ_CAP: u64 = 256 * 1024;
const ABOUT_ME_CAP: usize = 4_000;

/// The instruction for the summarize call. The caller sends this as the system prompt
/// and the text given to `summarize` as the user message.
pub const BRIEF_PROMPT: &str = "\
You turn a person's instruction files (CLAUDE.md, AGENTS.md, memory index, About me notes) into a short project brief. \
A prompt writer reads the brief to write better prompts for this person and project. It is not shown to the person.

Rules:
- Facts only, taken from the files. Never add, guess or explain.
- Keep exact paths, commands, file names, branch names, URLs, product and people names word for word.
- Cover, in this order, only what the files give: what the project is; stack and tools; key commands (build, test, run, deploy); \
rules the person wants agents to follow; live, risky or protected areas (production, client data, things that must not change or must be asked first).
- Later and closer files win: when two files disagree, keep the one that appears later.
- Skip greetings, history, style advice for chat replies, and anything that does not change how a prompt should be written.
- Never include keys, tokens, passwords or other secrets. Text shown as [secret removed] stays removed; do not mention it.
- Plain short lines starting with \"- \", grouped under these headings when they have content: Project, Stack, Commands, Rules, Live and risky.
- Max 300 words. Output only the brief.";

/// Where briefs are cached: `~/.local/share/promptmaxxer/briefs/`.
pub fn default_cache_dir() -> PathBuf {
    home_dir().join(".local/share/promptmaxxer/briefs")
}

/// The fingerprint a brief is stored under: the sources' fingerprint plus the About me text
/// and this brief prompt (a new prompt rebuilds every brief once).
pub fn brief_fingerprint(sources: &[Source], about_me: &str) -> String {
    let mut h = Sha256::new();
    h.update(fingerprint(sources).as_bytes());
    h.update(b"\0about\0");
    h.update(about_me.trim().as_bytes());
    h.update(b"\0prompt\0");
    h.update(BRIEF_PROMPT.as_bytes());
    hex(&h.finalize())
}

#[derive(Serialize, Deserialize)]
struct CacheFile {
    fingerprint: String,
    root: Option<PathBuf>,
    text: String,
    sources: Vec<Source>,
    created_unix: u64,
}

fn sha256_hex(s: &str) -> String {
    hex(&Sha256::digest(s.as_bytes()))
}

/// Cache file for a project: `<dir>/<sha256 of root>.json`. Without a project the key is the
/// empty string (the user-level brief).
pub(crate) fn cache_path(dir: &Path, project: Option<&Project>) -> PathBuf {
    let key = project.map(|p| p.root.to_string_lossy().into_owned()).unwrap_or_default();
    dir.join(format!("{}.json", sha256_hex(&key)))
}

/// Build (or load) the brief. See [`brief_in`]; this uses [`default_cache_dir`].
pub async fn brief<F, Fut>(project: Option<&Project>, prefs: &SourcePrefs, summarize: F) -> Option<Brief>
where
    F: Fn(String) -> Fut,
    Fut: Future<Output = Option<String>>,
{
    brief_in(&default_cache_dir(), project, prefs, summarize).await
}

/// Build (or load) the brief with the cache in `cache_dir`.
///
/// - No sources and no About me text: `None`, `summarize` is not called.
/// - The fingerprint matches the cache: the cached brief (`cached: true`), `summarize` is not called.
/// - Otherwise: the sources are joined (path headers, max 40 KB, earlier files trimmed first),
///   secrets are scrubbed, `summarize(text)` is called once, its answer is scrubbed again,
///   capped at 300 words, saved (mode 600) and returned (`cached: false`).
/// - `summarize` returns `None` or an empty answer: `None`, nothing is saved.
pub async fn brief_in<F, Fut>(cache_dir: &Path, project: Option<&Project>, prefs: &SourcePrefs, summarize: F) -> Option<Brief>
where
    F: Fn(String) -> Fut,
    Fut: Future<Output = Option<String>>,
{
    let srcs = sources(project, prefs);
    let about = prefs.about_me.trim();
    if srcs.is_empty() && about.is_empty() {
        return None;
    }
    let fp = brief_fingerprint(&srcs, about);
    let file = cache_path(cache_dir, project);
    if let Some(c) = read_cache(&file) {
        if c.fingerprint == fp && !c.text.trim().is_empty() {
            return Some(Brief { text: c.text, sources: srcs, fingerprint: fp, cached: true });
        }
    }
    let input = scrub_secrets(&source_text(&srcs, about));
    let answer = summarize(input).await?;
    let text = cap_words(scrub_secrets(answer.trim()).trim(), BRIEF_MAX_WORDS);
    if text.is_empty() {
        return None;
    }
    let cache = CacheFile {
        fingerprint: fp.clone(),
        root: project.map(|p| p.root.clone()),
        text: text.clone(),
        sources: srcs.clone(),
        created_unix: std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0),
    };
    let _ = write_cache(&file, &cache);
    Some(Brief { text, sources: srcs, fingerprint: fp, cached: false })
}

/// The last saved brief text for a project, whatever its fingerprint. For use while a fresh
/// brief is being built in the background. `None` when nothing was ever saved.
pub fn cached_brief_text(project: Option<&Project>) -> Option<String> {
    cached_brief_text_in(&default_cache_dir(), project)
}

pub fn cached_brief_text_in(cache_dir: &Path, project: Option<&Project>) -> Option<String> {
    read_cache(&cache_path(cache_dir, project)).map(|c| c.text).filter(|t| !t.trim().is_empty())
}

fn read_cache(file: &Path) -> Option<CacheFile> {
    let raw = std::fs::read(file).ok()?;
    serde_json::from_slice(&raw).ok()
}

fn write_cache(file: &Path, cache: &CacheFile) -> std::io::Result<()> {
    use std::os::unix::fs::{DirBuilderExt, OpenOptionsExt, PermissionsExt};
    let dir = file.parent().unwrap_or(Path::new("."));
    std::fs::DirBuilder::new().recursive(true).mode(0o700).create(dir)?;
    let tmp = dir.join(format!(".{}.tmp{}", file.file_name().unwrap_or_default().to_string_lossy(), std::process::id()));
    {
        let mut f = std::fs::OpenOptions::new().write(true).create(true).truncate(true).mode(0o600).open(&tmp)?;
        f.write_all(&serde_json::to_vec_pretty(cache).map_err(std::io::Error::other)?)?;
        f.sync_all()?;
    }
    std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600))?;
    std::fs::rename(&tmp, file)
}

fn read_capped(path: &Path) -> String {
    use std::io::Read;
    let mut buf = Vec::new();
    if let Ok(f) = std::fs::File::open(path) {
        let _ = f.take(FILE_READ_CAP).read_to_end(&mut buf);
    }
    String::from_utf8_lossy(&buf).into_owned()
}

fn truncate_at(s: &str, max: usize) -> &str {
    if s.len() <= max {
        return s;
    }
    let mut end = max;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

/// Join About me and every source under a path header. When the total is over
/// [`SOURCE_TEXT_CAP`], the earliest sources (user level first) are cut first, and a later,
/// closer file is cut only when the earlier ones are gone.
pub(crate) fn source_text(srcs: &[Source], about_me: &str) -> String {
    const TRIM_NOTE: &str = "\n[... trimmed]\n";
    let about = truncate_at(about_me.trim(), ABOUT_ME_CAP);
    let head = if about.is_empty() { String::new() } else { format!("### About me (user notes)\n{about}\n\n") };
    let mut sections: Vec<(String, String)> = srcs
        .iter()
        .map(|s| {
            let level = match s.level {
                SourceLevel::User => "user level",
                SourceLevel::Project => "project level",
            };
            (format!("### {} ({}, {})\n", s.path.display(), s.label, level), read_capped(&s.path).trim().to_string())
        })
        .collect();
    let size = |secs: &Vec<(String, String)>| head.len() + secs.iter().map(|(h, b)| h.len() + b.len() + 2).sum::<usize>();
    let mut i = 0;
    while size(&sections) > SOURCE_TEXT_CAP && i < sections.len() {
        let over = size(&sections) - SOURCE_TEXT_CAP;
        let body_len = sections[i].1.len();
        if body_len <= over + TRIM_NOTE.len() {
            sections.remove(i); // nothing useful left of this file
            continue;
        }
        let keep = truncate_at(&sections[i].1, body_len - over - TRIM_NOTE.len()).to_string();
        sections[i].1 = format!("{keep}{TRIM_NOTE}");
        i += 1;
    }
    let mut out = head;
    for (h, b) in sections {
        out.push_str(&h);
        out.push_str(&b);
        out.push_str("\n\n");
    }
    out.trim_end().to_string()
}

/// Keep at most `max` words, keeping the original line breaks.
pub(crate) fn cap_words(s: &str, max: usize) -> String {
    let mut count = 0;
    let mut in_word = false;
    for (i, c) in s.char_indices() {
        if c.is_whitespace() {
            in_word = false;
        } else if !in_word {
            in_word = true;
            count += 1;
            if count > max {
                return s[..i].trim_end().to_string();
            }
        }
    }
    s.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::secrets::SECRET_MARK;
    use std::cell::Cell;
    use std::fs;
    use std::rc::Rc;

    fn block_on<F: Future>(f: F) -> F::Output {
        use std::task::{Context, Poll, Waker};
        let mut cx = Context::from_waker(Waker::noop());
        let mut f = std::pin::pin!(f);
        loop {
            if let Poll::Ready(v) = f.as_mut().poll(&mut cx) {
                return v;
            }
            std::thread::yield_now();
        }
    }

    fn project_with(files: &[(&str, &str)]) -> (tempfile::TempDir, Project) {
        let tmp = tempfile::tempdir().unwrap();
        let root = fs::canonicalize(tmp.path()).unwrap().join("shop");
        fs::create_dir_all(root.join(".git")).unwrap();
        for (rel, body) in files {
            let p = root.join(rel);
            fs::create_dir_all(p.parent().unwrap()).unwrap();
            fs::write(p, body).unwrap();
        }
        let p = Project { root: root.clone(), cwd: root, name: "shop".into() };
        (tmp, p)
    }

    fn project_prefs() -> SourcePrefs {
        SourcePrefs { about_me: String::new(), claude_user: false, codex_user: false, project_files: true, blocked_projects: vec![] }
    }

    #[test]
    fn cache_miss_then_hit_then_miss_on_change() {
        let (tmp, p) = project_with(&[("CLAUDE.md", "- Build: cargo build\n- Never push to main")]);
        let cache = tmp.path().join("briefs");
        let calls = Rc::new(Cell::new(0));
        let seen = Rc::new(std::cell::RefCell::new(String::new()));
        let summarize = |text: String| {
            let calls = calls.clone();
            let seen = seen.clone();
            async move {
                calls.set(calls.get() + 1);
                *seen.borrow_mut() = text;
                Some(format!("- Build: cargo build (call {})", calls.get()))
            }
        };
        let prefs = project_prefs();

        let a = block_on(brief_in(&cache, Some(&p), &prefs, &summarize)).unwrap();
        assert!(!a.cached);
        assert_eq!(calls.get(), 1);
        assert_eq!(a.text, "- Build: cargo build (call 1)");
        assert!(seen.borrow().contains("Never push to main"));
        assert!(seen.borrow().contains(&format!("### {}", p.root.join("CLAUDE.md").display())));

        // Hit: summarize not called.
        let b = block_on(brief_in(&cache, Some(&p), &prefs, &summarize)).unwrap();
        assert!(b.cached);
        assert_eq!(calls.get(), 1);
        assert_eq!(b.text, a.text);
        assert_eq!(b.fingerprint, a.fingerprint);

        // Cache file is mode 600 and its folder 700.
        use std::os::unix::fs::PermissionsExt;
        let file = cache_path(&cache, Some(&p));
        assert_eq!(fs::metadata(&file).unwrap().permissions().mode() & 0o777, 0o600);
        assert_eq!(fs::metadata(&cache).unwrap().permissions().mode() & 0o777, 0o700);
        assert!(file.file_name().unwrap().to_string_lossy().len() == 64 + 5);

        // Change a file: miss.
        fs::write(p.root.join("CLAUDE.md"), "- Build: make\n").unwrap();
        let c = block_on(brief_in(&cache, Some(&p), &prefs, &summarize)).unwrap();
        assert!(!c.cached);
        assert_eq!(calls.get(), 2);
        assert_ne!(c.fingerprint, a.fingerprint);

        // About me change: miss.
        let mut prefs2 = prefs.clone();
        prefs2.about_me = "I run a bike shop.".into();
        let d = block_on(brief_in(&cache, Some(&p), &prefs2, &summarize)).unwrap();
        assert!(!d.cached);
        assert_eq!(calls.get(), 3);
        assert!(seen.borrow().starts_with("### About me"));
        let e = block_on(brief_in(&cache, Some(&p), &prefs2, &summarize)).unwrap();
        assert!(e.cached);
        assert_eq!(calls.get(), 3);
    }

    #[test]
    fn secrets_scrubbed_before_and_after_summarize() {
        let key = format!("sk-ant-api03-{}", "f".repeat(40));
        let (tmp, p) = project_with(&[("CLAUDE.md", &format!("ANTHROPIC_API_KEY={key}\n- Run: npm test"))]);
        let cache = tmp.path().join("briefs");
        let seen = Rc::new(std::cell::RefCell::new(String::new()));
        let leak = key.clone();
        let summarize = |text: String| {
            let seen = seen.clone();
            let leak = leak.clone();
            async move {
                *seen.borrow_mut() = text;
                Some(format!("- Run: npm test\n- Key: {leak}"))
            }
        };
        let b = block_on(brief_in(&cache, Some(&p), &project_prefs(), &summarize)).unwrap();
        assert!(!seen.borrow().contains(&key), "secret sent to summarize");
        assert!(seen.borrow().contains(SECRET_MARK));
        assert!(!b.text.contains(&key), "secret in brief");
        let raw = fs::read_to_string(cache_path(&cache, Some(&p))).unwrap();
        assert!(!raw.contains(&key), "secret in cache file");
    }

    #[test]
    fn nothing_to_brief_and_failed_summarize() {
        let (tmp, p) = project_with(&[]);
        let cache = tmp.path().join("briefs");
        let calls = Rc::new(Cell::new(0));
        let summarize = |_t: String| {
            let calls = calls.clone();
            async move {
                calls.set(calls.get() + 1);
                None
            }
        };
        assert!(block_on(brief_in(&cache, Some(&p), &project_prefs(), &summarize)).is_none());
        assert_eq!(calls.get(), 0, "no sources: no call");
        fs::write(p.root.join("AGENTS.md"), "- x").unwrap();
        assert!(block_on(brief_in(&cache, Some(&p), &project_prefs(), &summarize)).is_none());
        assert_eq!(calls.get(), 1);
        assert!(!cache_path(&cache, Some(&p)).exists(), "failure not cached");
    }

    #[test]
    fn brief_capped_at_300_words() {
        let (tmp, p) = project_with(&[("CLAUDE.md", "- x")]);
        let long: String = (0..500).map(|i| format!("w{i}")).collect::<Vec<_>>().join(" ");
        let summarize = |_t: String| {
            let long = long.clone();
            async move { Some(long) }
        };
        let b = block_on(brief_in(&tmp.path().join("b"), Some(&p), &project_prefs(), &summarize)).unwrap();
        assert_eq!(b.text.split_whitespace().count(), 300);
        assert!(b.text.ends_with("w299"));
    }

    #[test]
    fn cap_words_keeps_lines() {
        assert_eq!(cap_words("- a b\n- c d\n- e", 3), "- a b");
        assert_eq!(cap_words("one two\nthree", 3), "one two\nthree");
        assert_eq!(cap_words("", 3), "");
    }

    #[test]
    fn source_text_trims_earliest_first() {
        let tmp = tempfile::tempdir().unwrap();
        let user = tmp.path().join("user.md");
        let proj = tmp.path().join("proj.md");
        fs::write(&user, "U".repeat(30_000)).unwrap();
        fs::write(&proj, "P".repeat(30_000)).unwrap();
        let srcs = vec![
            Source { path: user, label: "CLAUDE.md".into(), level: SourceLevel::User },
            Source { path: proj, label: "CLAUDE.md".into(), level: SourceLevel::Project },
        ];
        let text = source_text(&srcs, "about me");
        assert!(text.len() <= SOURCE_TEXT_CAP, "{}", text.len());
        assert!(text.contains(&"P".repeat(30_000)), "closest file kept whole");
        assert!(text.contains("[... trimmed]"));
        assert!(text.starts_with("### About me"));
        // Very large: the user file goes completely, the project file is cut.
        let big = tmp.path().join("big.md");
        fs::write(&big, "B".repeat(60_000)).unwrap();
        let srcs2 = vec![srcs[0].clone(), Source { path: big, label: "AGENTS.md".into(), level: SourceLevel::Project }];
        let text = source_text(&srcs2, "");
        assert!(text.len() <= SOURCE_TEXT_CAP);
        assert!(!text.contains(&"U".repeat(100)));
        assert!(text.contains("BBBB"));
    }

    #[test]
    fn brief_prompt_says_the_rules() {
        assert!(BRIEF_PROMPT.contains("word for word"));
        assert!(BRIEF_PROMPT.contains("Max 300 words"));
        assert!(BRIEF_PROMPT.contains("secrets"));
    }

    #[test]
    fn stale_text_stays_readable_after_the_files_change() {
        let (tmp, p) = project_with(&[("CLAUDE.md", "- Build: cargo build")]);
        let cache = tmp.path().join("briefs");
        assert_eq!(cached_brief_text_in(&cache, Some(&p)), None);
        let prefs = project_prefs();
        let _ = block_on(brief_in(&cache, Some(&p), &prefs, |_| async { Some("- Build: cargo build".to_string()) })).unwrap();
        fs::write(p.root.join("CLAUDE.md"), "- Build: make").unwrap();
        // Fingerprint changed and the summarizer gives nothing: no fresh brief, the old text is still there.
        assert!(block_on(brief_in(&cache, Some(&p), &prefs, |_| async { None })).is_none());
        assert_eq!(cached_brief_text_in(&cache, Some(&p)).as_deref(), Some("- Build: cargo build"));
    }
}
