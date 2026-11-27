//! Which instruction files to read, in Claude Code / Codex order, their fingerprint,
//! and the context chip label.

use crate::{home_dir, Project, Source, SourceLevel, SourcePrefs};
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

/// Files read in every folder from the Git root down to the working folder, in this order.
const PROJECT_FILES: &[(&str, &str)] = &[
    ("CLAUDE.md", "CLAUDE.md"),
    ("CLAUDE.local.md", "CLAUDE.local.md"),
    ("AGENTS.md", "AGENTS.md"),
    (".claude/CLAUDE.md", "CLAUDE.md"),
];

/// Claude Code's folder name for a project under `~/.claude/projects/`:
/// every character that is not a letter or digit becomes `-`
/// (for example `/Users/x/.local/state/y` -> `-Users-x--local-state-y`,
/// `Application Support` -> `Application-Support`).
pub(crate) fn claude_project_dir_name(path: &Path) -> String {
    path.to_string_lossy().chars().map(|c| if c.is_ascii_alphanumeric() { c } else { '-' }).collect()
}

fn is_blocked(project: &Project, blocked: &[PathBuf]) -> bool {
    let root = canon(&project.root);
    let cwd = canon(&project.cwd);
    blocked.iter().any(|b| {
        let b = canon(b);
        root.starts_with(&b) || cwd.starts_with(&b)
    })
}

fn canon(p: &Path) -> PathBuf {
    std::fs::canonicalize(p).unwrap_or_else(|_| p.to_path_buf())
}

/// Folders from `root` down to `cwd` (both included). Just `[root]` when `cwd` is not inside `root`.
fn chain(root: &Path, cwd: &Path) -> Vec<PathBuf> {
    let mut out = vec![root.to_path_buf()];
    if let Ok(rel) = cwd.strip_prefix(root) {
        let mut cur = root.to_path_buf();
        for comp in rel.components() {
            cur.push(comp);
            out.push(cur.clone());
        }
    }
    out
}

/// The instruction files to read, in order: user level first, then project level from the
/// Git root down to the working folder, then Claude's project memory index.
/// Later, closer files win. Only files that exist are listed; a file reached by two paths
/// (for example `CLAUDE.md` linked to `AGENTS.md`) is listed once.
pub fn sources(project: Option<&Project>, prefs: &SourcePrefs) -> Vec<Source> {
    sources_with_home(project, prefs, &home_dir())
}

pub(crate) fn sources_with_home(project: Option<&Project>, prefs: &SourcePrefs, home: &Path) -> Vec<Source> {
    let mut out = Vec::new();
    let mut seen = HashSet::new();
    let mut push = |path: PathBuf, label: &str, level: SourceLevel, out: &mut Vec<Source>| {
        if !path.is_file() {
            return;
        }
        if !seen.insert(canon(&path)) {
            return;
        }
        out.push(Source { path, label: label.to_string(), level });
    };

    if prefs.claude_user {
        push(home.join(".claude/CLAUDE.md"), "CLAUDE.md", SourceLevel::User, &mut out);
    }
    if prefs.codex_user {
        push(home.join(".codex/AGENTS.md"), "AGENTS.md", SourceLevel::User, &mut out);
    }
    if let Some(p) = project {
        if prefs.project_files && !is_blocked(p, &prefs.blocked_projects) {
            for dir in chain(&p.root, &p.cwd) {
                for (rel, label) in PROJECT_FILES {
                    push(dir.join(rel), label, SourceLevel::Project, &mut out);
                }
            }
            let mut mem_dirs = vec![p.root.clone()];
            if p.cwd != p.root {
                mem_dirs.push(p.cwd.clone());
            }
            for d in mem_dirs {
                let idx = home.join(".claude/projects").join(claude_project_dir_name(&d)).join("memory/MEMORY.md");
                push(idx, "memory", SourceLevel::Project, &mut out);
            }
        }
    }
    out
}

/// Hash of (path, size, mtime) of every source. Changes when any file is edited, added or removed.
pub fn fingerprint(sources: &[Source]) -> String {
    let mut h = Sha256::new();
    for s in sources {
        h.update(s.path.to_string_lossy().as_bytes());
        h.update([0]);
        match std::fs::metadata(&s.path) {
            Ok(m) => {
                let mtime = m.modified().ok().and_then(|t| t.duration_since(UNIX_EPOCH).ok()).map(|d| d.as_nanos()).unwrap_or(0);
                h.update(m.len().to_le_bytes());
                h.update(mtime.to_le_bytes());
            }
            Err(_) => h.update(b"missing"),
        }
        h.update(*b"\n");
    }
    hex(&h.finalize())
}

pub(crate) fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// The context chip: `"shop · CLAUDE.md + 2 memories"`. Counts the project's own files by label
/// (user-level files are the same in every project, so they are not shown).
/// `"shop · no project files"` when the project has none; `"No project"` without a project.
pub fn chip_label(project: Option<&Project>, sources: &[Source]) -> String {
    let Some(p) = project else {
        return "No project".to_string();
    };
    let mut counts: Vec<(&str, usize)> = Vec::new();
    for s in sources.iter().filter(|s| s.level == SourceLevel::Project) {
        match counts.iter_mut().find(|(l, _)| *l == s.label) {
            Some((_, n)) => *n += 1,
            None => counts.push((s.label.as_str(), 1)),
        }
    }
    if counts.is_empty() {
        return format!("{} \u{00b7} no project files", p.name);
    }
    let parts: Vec<String> = counts
        .iter()
        .map(|(label, n)| match (*label, *n) {
            ("memory", 1) => "memory".to_string(),
            ("memory", n) => format!("{n} memories"),
            (l, 1) => l.to_string(),
            (l, n) => format!("{n} {l}"),
        })
        .collect();
    format!("{} \u{00b7} {}", p.name, parts.join(" + "))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn write(p: &Path, s: &str) {
        fs::create_dir_all(p.parent().unwrap()).unwrap();
        fs::write(p, s).unwrap();
    }

    struct Fx {
        _tmp: tempfile::TempDir,
        home: PathBuf,
        root: PathBuf,
        cwd: PathBuf,
    }

    fn fixture() -> Fx {
        let tmp = tempfile::tempdir().unwrap();
        let base = fs::canonicalize(tmp.path()).unwrap();
        let home = base.join("home");
        let root = base.join("work/shop");
        let cwd = root.join("apps/web");
        fs::create_dir_all(root.join(".git")).unwrap();
        fs::create_dir_all(&cwd).unwrap();
        write(&home.join(".claude/CLAUDE.md"), "user claude");
        write(&home.join(".codex/AGENTS.md"), "user codex");
        write(&root.join("CLAUDE.md"), "root claude");
        write(&root.join("AGENTS.md"), "root agents");
        write(&root.join(".claude/CLAUDE.md"), "root dot claude");
        write(&root.join("apps/CLAUDE.local.md"), "apps local");
        write(&cwd.join("CLAUDE.md"), "web claude");
        write(&home.join(".claude/projects").join(claude_project_dir_name(&root)).join("memory/MEMORY.md"), "- memory");
        write(&home.join(".claude/projects").join(claude_project_dir_name(&cwd)).join("memory/MEMORY.md"), "- memory 2");
        Fx { _tmp: tmp, home, root, cwd }
    }

    fn project(fx: &Fx) -> Project {
        Project { root: fx.root.clone(), cwd: fx.cwd.clone(), name: "shop".into() }
    }

    #[test]
    fn order_user_then_root_down_to_cwd_then_memory() {
        let fx = fixture();
        let p = project(&fx);
        let got: Vec<(String, String, SourceLevel)> = sources_with_home(Some(&p), &SourcePrefs::all_on(), &fx.home)
            .into_iter()
            .map(|s| (s.path.strip_prefix(fx.home.parent().unwrap()).unwrap().to_string_lossy().replace('\\', "/"), s.label, s.level))
            .collect();
        let mem_root = format!("home/.claude/projects/{}/memory/MEMORY.md", claude_project_dir_name(&fx.root));
        let mem_cwd = format!("home/.claude/projects/{}/memory/MEMORY.md", claude_project_dir_name(&fx.cwd));
        let want = vec![
            ("home/.claude/CLAUDE.md".to_string(), "CLAUDE.md".to_string(), SourceLevel::User),
            ("home/.codex/AGENTS.md".into(), "AGENTS.md".into(), SourceLevel::User),
            ("work/shop/CLAUDE.md".into(), "CLAUDE.md".into(), SourceLevel::Project),
            ("work/shop/AGENTS.md".into(), "AGENTS.md".into(), SourceLevel::Project),
            ("work/shop/.claude/CLAUDE.md".into(), "CLAUDE.md".into(), SourceLevel::Project),
            ("work/shop/apps/CLAUDE.local.md".into(), "CLAUDE.local.md".into(), SourceLevel::Project),
            ("work/shop/apps/web/CLAUDE.md".into(), "CLAUDE.md".into(), SourceLevel::Project),
            (mem_root, "memory".into(), SourceLevel::Project),
            (mem_cwd, "memory".into(), SourceLevel::Project),
        ];
        assert_eq!(got, want);
    }

    #[test]
    fn prefs_switch_sources_off() {
        let fx = fixture();
        let p = project(&fx);
        let mut prefs = SourcePrefs::all_on();
        prefs.claude_user = false;
        prefs.codex_user = false;
        let s = sources_with_home(Some(&p), &prefs, &fx.home);
        assert!(s.iter().all(|s| s.level == SourceLevel::Project));
        prefs.project_files = false;
        assert!(sources_with_home(Some(&p), &prefs, &fx.home).is_empty());
        // Blocked project: user level only.
        let mut prefs = SourcePrefs::all_on();
        prefs.blocked_projects = vec![fx.root.clone()];
        let s = sources_with_home(Some(&p), &prefs, &fx.home);
        assert_eq!(s.len(), 2);
        assert!(s.iter().all(|s| s.level == SourceLevel::User));
        // No project: user level only.
        assert_eq!(sources_with_home(None, &SourcePrefs::all_on(), &fx.home).len(), 2);
    }

    #[cfg(unix)]
    #[test]
    fn linked_file_listed_once() {
        let fx = fixture();
        let other = fx.root.join("docs");
        fs::create_dir_all(&other).unwrap();
        fs::remove_file(fx.root.join("AGENTS.md")).unwrap();
        std::os::unix::fs::symlink(fx.root.join("CLAUDE.md"), fx.root.join("AGENTS.md")).unwrap();
        let p = project(&fx);
        let s = sources_with_home(Some(&p), &SourcePrefs::all_on(), &fx.home);
        assert!(!s.iter().any(|s| s.path == fx.root.join("AGENTS.md")));
        assert!(s.iter().any(|s| s.path == fx.root.join("CLAUDE.md")));
    }

    #[test]
    fn claude_dir_names_match_claude_code() {
        assert_eq!(claude_project_dir_name(Path::new("/Users/x")), "-Users-x");
        assert_eq!(claude_project_dir_name(Path::new("/Users/x/.local/state/some-tool")), "-Users-x--local-state-some-tool");
        assert_eq!(claude_project_dir_name(Path::new("/Users/x/Library/Application Support/My App")), "-Users-x-Library-Application-Support-My-App");
    }

    #[test]
    fn fingerprint_changes_with_files() {
        let fx = fixture();
        let p = project(&fx);
        let s = sources_with_home(Some(&p), &SourcePrefs::all_on(), &fx.home);
        let a = fingerprint(&s);
        assert_eq!(a, fingerprint(&s), "stable");
        assert_eq!(a.len(), 64);
        // Edit (size change).
        write(&fx.root.join("CLAUDE.md"), "root claude, longer now");
        let b = fingerprint(&s);
        assert_ne!(a, b);
        // Same size, new mtime.
        let f = fs::File::options().write(true).open(fx.root.join("CLAUDE.md")).unwrap();
        f.set_modified(UNIX_EPOCH + std::time::Duration::from_secs(1_000_000)).unwrap();
        let c = fingerprint(&s);
        assert_ne!(b, c);
        // A new file joins the list.
        write(&fx.cwd.join("AGENTS.md"), "new");
        let s2 = sources_with_home(Some(&p), &SourcePrefs::all_on(), &fx.home);
        assert_ne!(c, fingerprint(&s2));
        // Order matters and empty is stable.
        assert_eq!(fingerprint(&[]), fingerprint(&[]));
    }

    #[test]
    fn chip_labels() {
        let fx = fixture();
        let p = project(&fx);
        let s = sources_with_home(Some(&p), &SourcePrefs::all_on(), &fx.home);
        assert_eq!(chip_label(Some(&p), &s), "shop \u{00b7} 3 CLAUDE.md + AGENTS.md + CLAUDE.local.md + 2 memories");
        let src = |label: &str, level| Source { path: PathBuf::from("/x"), label: label.into(), level };
        let s = vec![
            src("CLAUDE.md", SourceLevel::User),
            src("CLAUDE.md", SourceLevel::Project),
            src("memory", SourceLevel::Project),
            src("memory", SourceLevel::Project),
        ];
        assert_eq!(chip_label(Some(&p), &s), "shop \u{00b7} CLAUDE.md + 2 memories");
        assert_eq!(chip_label(Some(&p), &s[..3]), "shop \u{00b7} CLAUDE.md + memory");
        assert_eq!(chip_label(Some(&p), &s[..1]), "shop \u{00b7} no project files");
        assert_eq!(chip_label(None, &s), "No project");
    }
}
