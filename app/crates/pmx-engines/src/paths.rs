//! Finding CLIs from a menu bar app (bare PATH) and the scratch folders the engines run in.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::io::Read;
use std::path::{Path, PathBuf};

pub(crate) fn home() -> PathBuf {
    std::env::var_os("HOME")
        .filter(|h| !h.is_empty())
        .map(PathBuf::from)
        .or_else(|| passwd().map(|(_, dir)| PathBuf::from(dir)))
        .unwrap_or_else(|| PathBuf::from("/"))
}

/// (user name, home folder) of the current user from the password database.
fn passwd() -> Option<(String, String)> {
    // SAFETY: getpwuid returns a pointer to static storage or null; we copy the strings at once.
    unsafe {
        let pw = libc::getpwuid(libc::getuid());
        if pw.is_null() {
            return None;
        }
        let name = std::ffi::CStr::from_ptr((*pw).pw_name).to_string_lossy().into_owned();
        let dir = std::ffi::CStr::from_ptr((*pw).pw_dir).to_string_lossy().into_owned();
        Some((name, dir))
    }
}

/// Fill in HOME, USER and LOGNAME when the app was started without them. Claude Code finds its
/// keychain sign-in by user name: with no USER it reports "not logged in" (checked on 2.1.330).
pub(crate) fn fill_env(cmd: &mut std::process::Command) {
    let missing = |k: &str| std::env::var_os(k).is_none_or(|v| v.is_empty());
    if missing("HOME") {
        cmd.env("HOME", home());
    }
    if missing("USER") || missing("LOGNAME") {
        if let Some((name, _)) = passwd() {
            if missing("USER") {
                cmd.env("USER", &name);
            }
            if missing("LOGNAME") {
                cmd.env("LOGNAME", &name);
            }
        }
    }
}

/// Folder every CLI runs in. Outside the home folder, so no CLAUDE.md, AGENTS.md or GEMINI.md is found.
/// Under the per-user temp folder (`$TMPDIR` on macOS, mode 700), not the shared `/tmp`: another account on
/// the Mac must not be able to plant files (a CLAUDE.md, a framework file) where the CLIs read them.
pub(crate) fn run_dir() -> PathBuf {
    std::env::temp_dir().join("promptmaxxer-run")
}

/// `~/.nvm/versions/node/*/bin`, newest version first.
fn nvm_bins(home: &Path) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = std::fs::read_dir(home.join(".nvm/versions/node"))
        .map(|rd| rd.filter_map(|e| e.ok()).map(|e| e.path().join("bin")).filter(|p| p.is_dir()).collect())
        .unwrap_or_default();
    v.sort_by_key(|p| std::cmp::Reverse(version_key(p)));
    v
}

fn version_key(p: &Path) -> Vec<u64> {
    let name = p.parent().and_then(|d| d.file_name()).and_then(|n| n.to_str()).unwrap_or("");
    name.trim_start_matches('v').split('.').map(|x| x.parse().unwrap_or(0)).collect()
}

/// Search order: PATH, then `~/.local/bin`, `/opt/homebrew/bin`, `/usr/local/bin`, `~/.nvm/versions/node/*/bin`.
pub(crate) fn search_dirs() -> Vec<PathBuf> {
    let home = home();
    let mut dirs: Vec<PathBuf> = std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).collect())
        .unwrap_or_default();
    dirs.push(home.join(".local/bin"));
    dirs.push(PathBuf::from("/opt/homebrew/bin"));
    dirs.push(PathBuf::from("/usr/local/bin"));
    dirs.extend(nvm_bins(&home));
    let mut seen = std::collections::HashSet::new();
    dirs.retain(|d| !d.as_os_str().is_empty() && seen.insert(d.clone()));
    dirs
}

fn is_exec(p: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(p).map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0).unwrap_or(false)
}

pub(crate) fn find_cli(name: &str) -> Option<PathBuf> {
    search_dirs().into_iter().map(|d| d.join(name)).find(|p| is_exec(p))
}

pub(crate) fn is_script(p: &Path) -> bool {
    let mut b = [0u8; 2];
    std::fs::File::open(p).and_then(|mut f| f.read_exact(&mut b)).map(|_| &b == b"#!").unwrap_or(false)
}

/// The npm `codex` is a node launcher (`bin/codex.js`) for a native binary. Run the native binary
/// directly: no node on a bare PATH needed and one process less.
pub(crate) fn codex_native(launcher: &Path) -> Option<PathBuf> {
    if !is_script(launcher) {
        return Some(launcher.to_path_buf());
    }
    let real = std::fs::canonicalize(launcher).ok()?;
    let pkg = real.parent()?.parent()?; // .../@openai/codex
    let (triple, plat) = if cfg!(target_arch = "aarch64") {
        ("aarch64-apple-darwin", "codex-darwin-arm64")
    } else {
        ("x86_64-apple-darwin", "codex-darwin-x64")
    };
    [
        pkg.join("node_modules/@openai").join(plat).join("vendor").join(triple).join("bin/codex"),
        pkg.join("vendor").join(triple).join("bin/codex"),
        pkg.parent()?.join(plat).join("vendor").join(triple).join("bin/codex"),
    ]
    .into_iter()
    .find(|p| is_exec(p))
}

/// PATH for child processes: the CLI's own folder first (node sits next to nvm CLIs), then the search dirs.
pub(crate) fn child_path(cli: &Path) -> std::ffi::OsString {
    let mut dirs = Vec::new();
    if let Some(d) = cli.parent() {
        dirs.push(d.to_path_buf());
    }
    dirs.extend(search_dirs());
    for d in ["/usr/bin", "/bin", "/usr/sbin", "/sbin"] {
        dirs.push(PathBuf::from(d));
    }
    let mut seen = std::collections::HashSet::new();
    dirs.retain(|d| seen.insert(d.clone()));
    std::env::join_paths(dirs).unwrap_or_default()
}

/// Write the framework (system prompt) once per content to the run folder and return its path.
/// The CLIs take the system prompt as a file (`--system-prompt-file`, `model_instructions_file`, `GEMINI_SYSTEM_MD`).
pub(crate) fn framework_file(framework: &str) -> std::io::Result<PathBuf> {
    let dir = run_dir();
    std::fs::create_dir_all(&dir)?;
    let mut h = DefaultHasher::new();
    framework.hash(&mut h);
    let path = dir.join(format!("framework-{:016x}.md", h.finish()));
    if std::fs::read_to_string(&path).map(|s| s == framework).unwrap_or(false) {
        return Ok(path);
    }
    // Unique temp name per writer: parallel calls with the same framework must not share one.
    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let seq = SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let tmp = dir.join(format!("framework-{:016x}.{}-{seq}.tmp", h.finish(), std::process::id()));
    std::fs::write(&tmp, framework)?;
    std::fs::rename(&tmp, &path)?;
    Ok(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn search_dirs_include_fallbacks_even_with_bare_path() {
        let d = search_dirs();
        let h = home();
        assert!(d.contains(&h.join(".local/bin")));
        assert!(d.contains(&PathBuf::from("/opt/homebrew/bin")));
        assert!(d.contains(&PathBuf::from("/usr/local/bin")));
    }

    #[test]
    fn fill_env_sets_user_when_missing() {
        let (name, dir) = passwd().unwrap();
        assert!(!name.is_empty());
        assert!(dir.starts_with('/'));
        let mut c = std::process::Command::new("/usr/bin/true");
        fill_env(&mut c);
        if std::env::var_os("USER").is_some() {
            assert!(!c.get_envs().any(|(k, _)| k == "USER"));
        }
    }

    #[test]
    fn version_sort_newest_first() {
        let a = PathBuf::from("/x/.nvm/versions/node/v9.1.0/bin");
        let b = PathBuf::from("/x/.nvm/versions/node/v24.13.1/bin");
        assert!(version_key(&b) > version_key(&a));
    }

    #[test]
    fn framework_file_parallel_writers() {
        let text = format!("parallel framework {}", std::process::id());
        let hs: Vec<_> = (0..8).map(|_| { let t = text.clone(); std::thread::spawn(move || framework_file(&t).unwrap()) }).collect();
        let paths: Vec<PathBuf> = hs.into_iter().map(|h| h.join().unwrap()).collect();
        assert!(paths.windows(2).all(|w| w[0] == w[1]));
        assert_eq!(std::fs::read_to_string(&paths[0]).unwrap(), text);
        let _ = std::fs::remove_file(&paths[0]);
    }

    #[test]
    fn framework_file_is_reused_and_exact() {
        let text = format!("test framework {}", std::process::id());
        let p1 = framework_file(&text).unwrap();
        let p2 = framework_file(&text).unwrap();
        assert_eq!(p1, p2);
        assert_eq!(std::fs::read_to_string(&p1).unwrap(), text);
        let _ = std::fs::remove_file(p1);
    }
}
