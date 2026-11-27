//! Finding CLIs from a menu bar app (bare PATH) and the scratch folders the engines run in.

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};

#[cfg(unix)]
pub(crate) fn home() -> PathBuf {
    std::env::var_os("HOME")
        .filter(|h| !h.is_empty())
        .map(PathBuf::from)
        .or_else(|| passwd().map(|(_, dir)| PathBuf::from(dir)))
        .unwrap_or_else(|| PathBuf::from("/"))
}

/// Windows: the user profile folder (`%USERPROFILE%`, `C:\Users\<name>`), where the CLIs keep `.claude`,
/// `.codex` and `.gemini`.
#[cfg(windows)]
pub(crate) fn home() -> PathBuf {
    ["USERPROFILE", "HOME"]
        .iter()
        .filter_map(std::env::var_os)
        .find(|h| !h.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
}

/// (user name, home folder) of the current user from the password database.
#[cfg(unix)]
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
/// Windows apps always get USERPROFILE and USERNAME from the system: nothing to fill in.
#[cfg(windows)]
pub(crate) fn fill_env(_cmd: &mut std::process::Command) {}

#[cfg(unix)]
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
#[cfg_attr(windows, allow(dead_code))]
fn nvm_bins(home: &Path) -> Vec<PathBuf> {
    let mut v: Vec<PathBuf> = std::fs::read_dir(home.join(".nvm/versions/node"))
        .map(|rd| rd.filter_map(|e| e.ok()).map(|e| e.path().join("bin")).filter(|p| p.is_dir()).collect())
        .unwrap_or_default();
    v.sort_by_key(|p| std::cmp::Reverse(version_key(p)));
    v
}

#[cfg_attr(windows, allow(dead_code))]
fn version_key(p: &Path) -> Vec<u64> {
    let name = p.parent().and_then(|d| d.file_name()).and_then(|n| n.to_str()).unwrap_or("");
    name.trim_start_matches('v').split('.').map(|x| x.parse().unwrap_or(0)).collect()
}

/// Search order: PATH, then `~/.local/bin`, `/opt/homebrew/bin`, `/usr/local/bin`, `~/.nvm/versions/node/*/bin`.
/// Windows: PATH, then `~\.local\bin` (Claude Code's own installer), `%APPDATA%\npm` (npm global installs),
/// `%LOCALAPPDATA%\Programs` style folders are reached through PATH.
pub(crate) fn search_dirs() -> Vec<PathBuf> {
    let home = home();
    let mut dirs: Vec<PathBuf> = std::env::var_os("PATH")
        .map(|p| std::env::split_paths(&p).collect())
        .unwrap_or_default();
    dirs.push(home.join(".local").join("bin"));
    #[cfg(windows)]
    {
        if let Some(appdata) = std::env::var_os("APPDATA").filter(|a| !a.is_empty()) {
            dirs.push(PathBuf::from(appdata).join("npm"));
        }
        // nvm for Windows links the active node version here.
        if let Some(link) = std::env::var_os("NVM_SYMLINK").filter(|a| !a.is_empty()) {
            dirs.push(PathBuf::from(link));
        }
    }
    #[cfg(unix)]
    {
        dirs.push(PathBuf::from("/opt/homebrew/bin"));
        dirs.push(PathBuf::from("/usr/local/bin"));
        dirs.extend(nvm_bins(&home));
    }
    let mut seen = std::collections::HashSet::new();
    dirs.retain(|d| !d.as_os_str().is_empty() && seen.insert(d.clone()));
    dirs
}

#[cfg(unix)]
fn is_exec(p: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(p).map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0).unwrap_or(false)
}

#[cfg(windows)]
fn is_exec(p: &Path) -> bool {
    std::fs::metadata(p).map(|m| m.is_file()).unwrap_or(false)
}

#[cfg(unix)]
pub(crate) fn find_cli(name: &str) -> Option<PathBuf> {
    search_dirs().into_iter().map(|d| d.join(name)).find(|p| is_exec(p))
}

/// Windows: `name` plus each PATHEXT extension in each folder (`claude.exe`, `claude.cmd`, ...). The
/// extensionless `claude` that npm also writes is a shell script for Git Bash, never started from here.
#[cfg(windows)]
pub(crate) fn find_cli(name: &str) -> Option<PathBuf> {
    let exts = path_exts();
    search_dirs()
        .into_iter()
        .flat_map(|d| exts.iter().map(move |e| d.join(format!("{name}{e}"))))
        .find(|p| is_exec(p))
}

/// PATHEXT, limited to what can be started without a shell or interpreter guess.
#[cfg(windows)]
fn path_exts() -> Vec<String> {
    const OK: [&str; 4] = [".com", ".exe", ".bat", ".cmd"];
    let from_env: Vec<String> = std::env::var("PATHEXT")
        .unwrap_or_default()
        .split(';')
        .map(|e| e.trim().to_ascii_lowercase())
        .filter(|e| OK.contains(&e.as_str()))
        .collect();
    if from_env.is_empty() {
        OK.iter().map(|e| e.to_string()).collect()
    } else {
        from_env
    }
}

#[cfg(windows)]
fn is_batch(p: &Path) -> bool {
    p.extension().and_then(|e| e.to_str()).is_some_and(|e| e.eq_ignore_ascii_case("cmd") || e.eq_ignore_ascii_case("bat"))
}

/// The script an npm `.cmd` shim starts (`"%dp0%\node_modules\@openai\codex\bin\codex.js" %*`), if it is one.
#[cfg(windows)]
pub(crate) fn npm_shim_script(shim: &Path) -> Option<PathBuf> {
    if !is_batch(shim) {
        return None;
    }
    let text = std::fs::read_to_string(shim).ok()?;
    let dir = shim.parent()?;
    for marker in ["\"%dp0%\\", "\"%~dp0\\"] {
        let Some(i) = text.find(marker) else { continue };
        let rest = &text[i + marker.len()..];
        let rel = &rest[..rest.find('"')?];
        if rel.to_ascii_lowercase().ends_with(".js") || rel.to_ascii_lowercase().ends_with(".mjs") || rel.to_ascii_lowercase().ends_with(".cjs") {
            let p = dir.join(rel);
            return p.is_file().then_some(p);
        }
    }
    None
}

/// How to start a CLI file: (program, arguments that go before the CLI's own).
/// Windows npm shims (`.cmd`) are started as `node <script>`: no cmd.exe, so the draft and other arguments
/// are never read by the cmd.exe parser. A `.cmd` that is not an npm shim is started directly; Rust then
/// quotes for cmd.exe and refuses arguments it cannot quote safely (a line break, for example).
#[cfg(windows)]
pub(crate) fn launch(cli: &Path) -> (PathBuf, Vec<std::ffi::OsString>) {
    if let Some(script) = npm_shim_script(cli) {
        let local = cli.with_file_name("node.exe");
        let node = if local.is_file() { Some(local) } else { find_cli("node").filter(|n| !is_batch(n)) };
        if let Some(node) = node {
            return (node, vec![script.into_os_string()]);
        }
    }
    (cli.to_path_buf(), vec![])
}

#[cfg(unix)]
pub(crate) fn launch(cli: &Path) -> (PathBuf, Vec<std::ffi::OsString>) {
    (cli.to_path_buf(), vec![])
}

/// A command for a CLI file (see [`launch`]). Windows: no console window flashes up.
pub(crate) fn std_command(cli: &Path) -> std::process::Command {
    let (prog, pre) = launch(cli);
    let mut c = std::process::Command::new(prog);
    c.args(pre);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        c.creation_flags(windows_sys::Win32::System::Threading::CREATE_NO_WINDOW);
    }
    c
}

/// Async form of [`std_command`].
pub(crate) fn cli_command(cli: &Path) -> tokio::process::Command {
    tokio::process::Command::from(std_command(cli))
}

/// Windows: give a scratch home the CLI's sign-in file. A symlink when Windows allows one (Developer Mode),
/// else a copy. A copy the CLI rewrote since (a refreshed sign-in token) is carried back to the real file
/// first, so a refresh is never lost.
#[cfg(windows)]
pub(crate) fn share_file(target: &Path, link: &Path) -> std::io::Result<()> {
    if std::fs::read_link(link).ok().as_deref() == Some(target) {
        return Ok(());
    }
    if let (Ok(lm), Ok(tm)) = (std::fs::symlink_metadata(link), std::fs::metadata(target)) {
        if lm.is_file() && lm.modified()? > tm.modified()? && std::fs::read(link)? != std::fs::read(target)? {
            std::fs::copy(link, target)?;
        }
    }
    let _ = std::fs::remove_file(link);
    if std::os::windows::fs::symlink_file(target, link).is_ok() {
        return Ok(());
    }
    std::fs::copy(target, link).map(|_| ())
}

/// Unix: link `link` to `target`. Two runs that set up the same scratch home at once can both remove the
/// old link and both create it; the second then finds the right link already there, which is fine.
#[cfg(unix)]
pub(crate) fn symlink_shared(target: &Path, link: &Path) -> std::io::Result<()> {
    match std::os::unix::fs::symlink(target, link) {
        Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists && std::fs::read_link(link).ok().as_deref() == Some(target) => Ok(()),
        r => r,
    }
}

/// Folder the quick status checks run in: `/tmp` on macOS, the per-user temp folder on Windows.
pub(crate) fn status_dir() -> PathBuf {
    if cfg!(windows) {
        std::env::temp_dir()
    } else {
        PathBuf::from("/tmp")
    }
}

#[cfg(unix)]
pub(crate) fn is_script(p: &Path) -> bool {
    use std::io::Read;
    let mut b = [0u8; 2];
    std::fs::File::open(p).and_then(|mut f| f.read_exact(&mut b)).map(|_| &b == b"#!").unwrap_or(false)
}

/// The npm `codex` is a node launcher (`bin/codex.js`) for a native binary. Run the native binary
/// directly: no node on a bare PATH needed and one process less.
#[cfg(unix)]
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

/// Windows: `codex.exe` is used as is. An npm `codex.cmd` shim starts `@openai/codex/bin/codex.js`, which runs
/// `vendor/<triple>/codex/codex.exe` from the platform package (`@openai/codex-win32-x64` or `-arm64`).
#[cfg(windows)]
pub(crate) fn codex_native(launcher: &Path) -> Option<PathBuf> {
    if !is_batch(launcher) {
        return Some(launcher.to_path_buf());
    }
    let script = npm_shim_script(launcher)
        .or_else(|| Some(launcher.parent()?.join("node_modules").join("@openai").join("codex").join("bin").join("codex.js")))?;
    let pkg = script.parent()?.parent()?; // ...\node_modules\@openai\codex
    let (triple, plat) = if cfg!(target_arch = "aarch64") {
        ("aarch64-pc-windows-msvc", "codex-win32-arm64")
    } else {
        ("x86_64-pc-windows-msvc", "codex-win32-x64")
    };
    let mut cands = vec![];
    for base in [pkg.join("node_modules").join("@openai").join(plat), pkg.to_path_buf(), pkg.parent()?.join(plat)] {
        let v = base.join("vendor").join(triple);
        cands.push(v.join("codex").join("codex.exe"));
        cands.push(v.join("bin").join("codex.exe"));
    }
    cands.into_iter().find(|p| is_exec(p))
}

/// PATH for child processes: the CLI's own folder first (node sits next to nvm CLIs), then the search dirs.
pub(crate) fn child_path(cli: &Path) -> std::ffi::OsString {
    let mut dirs = Vec::new();
    if let Some(d) = cli.parent() {
        dirs.push(d.to_path_buf());
    }
    dirs.extend(search_dirs());
    #[cfg(unix)]
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

    #[cfg(unix)]
    #[test]
    fn search_dirs_include_fallbacks_even_with_bare_path() {
        let d = search_dirs();
        let h = home();
        assert!(d.contains(&h.join(".local/bin")));
        assert!(d.contains(&PathBuf::from("/opt/homebrew/bin")));
        assert!(d.contains(&PathBuf::from("/usr/local/bin")));
    }

    #[cfg(unix)]
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

    #[cfg(windows)]
    #[test]
    fn npm_shims_start_through_node() {
        let dir = std::env::temp_dir().join(format!("pmx-shim-{}", std::process::id()));
        let pkg = dir.join("node_modules").join("@openai").join("codex").join("bin");
        std::fs::create_dir_all(&pkg).unwrap();
        std::fs::write(pkg.join("codex.js"), "").unwrap();
        let shim = dir.join("codex.cmd");
        std::fs::write(&shim, "@ECHO off\r\nGOTO start\r\n:start\r\nendLocal & goto #_undefined_# 2>NUL || title %COMSPEC% & \"%_prog%\"  \"%dp0%\\node_modules\\@openai\\codex\\bin\\codex.js\" %*\r\n").unwrap();
        assert_eq!(npm_shim_script(&shim), Some(pkg.join("codex.js")));
        std::fs::write(dir.join("node.exe"), "").unwrap();
        let (prog, pre) = launch(&shim);
        assert_eq!(prog, dir.join("node.exe"));
        assert_eq!(pre, vec![std::ffi::OsString::from(pkg.join("codex.js"))]);
        let exe = dir.join("tool.exe");
        assert_eq!(launch(&exe).0, exe);
        let _ = std::fs::remove_dir_all(&dir);
    }
}

/// One scratch-home setup at a time in this process (the background brief and a key press can start
/// together). On Windows a file another process holds open cannot be replaced (error 32, sometimes 5):
/// such a setup is tried again a few times before it fails.
pub(crate) fn setup_once(f: impl Fn() -> std::io::Result<()>) -> std::io::Result<()> {
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    let _g = LOCK.lock().unwrap_or_else(|e| e.into_inner());
    let mut tries = 0;
    loop {
        match f() {
            Err(e) if cfg!(windows) && matches!(e.raw_os_error(), Some(32) | Some(5)) && tries < 8 => {
                tries += 1;
                std::thread::sleep(std::time::Duration::from_millis(40 * tries));
            }
            r => return r,
        }
    }
}
