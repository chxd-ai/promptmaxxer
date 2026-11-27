//! Fixed file locations. Nothing here points into Documents at runtime: macOS blocks background apps there.
//! Windows: settings, history and the log all live in `%APPDATA%\promptmaxxer`.
use std::path::{Path, PathBuf};

#[cfg(not(windows))]
pub fn home() -> PathBuf {
    std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("/tmp"))
}

/// Windows: the user profile folder (`%USERPROFILE%`).
#[cfg(windows)]
pub fn home() -> PathBuf {
    ["USERPROFILE", "HOME"]
        .iter()
        .filter_map(std::env::var_os)
        .find(|h| !h.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
}

/// `~/.local/share/promptmaxxer` (shared with the runner and later the settings file).
#[cfg(not(windows))]
pub fn share_dir() -> PathBuf {
    home().join(".local/share/promptmaxxer")
}

/// `%APPDATA%\promptmaxxer` (roaming app data, private to the user).
#[cfg(windows)]
pub fn share_dir() -> PathBuf {
    match std::env::var_os("APPDATA").filter(|a| !a.is_empty()) {
        Some(a) => PathBuf::from(a).join("promptmaxxer"),
        None => home().join("AppData").join("Roaming").join("promptmaxxer"),
    }
}

pub fn history_file() -> PathBuf {
    share_dir().join("history.jsonl")
}

#[cfg(not(windows))]
pub fn log_file() -> PathBuf {
    home().join("Library/Logs/promptmaxxer.log")
}

#[cfg(windows)]
pub fn log_file() -> PathBuf {
    share_dir().join("promptmaxxer.log")
}

/// Open a file only the user can read (mode 600 on macOS; on Windows the files sit in the user's own
/// `%APPDATA%`, which other users cannot read). `truncate`: write from scratch, else append.
pub fn private_file(path: &Path, truncate: bool) -> std::io::Result<std::fs::File> {
    let mut o = std::fs::OpenOptions::new();
    if truncate {
        o.write(true).create(true).truncate(true);
    } else {
        o.create(true).append(true);
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        o.mode(0o600);
    }
    o.open(path)
}

/// Mode 600 (Unix); nothing to do on Windows.
pub fn set_private(path: &Path) -> std::io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))?;
    }
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

/// The framework (system prompt) bundled with the app: `<app>.app/Contents/Resources/framework/system-prompt.md`
/// (Windows: `framework\system-prompt.md` next to the installed exe). A dev build run from `target/` falls back to
/// the repo copy.
pub fn framework_file() -> PathBuf {
    if let Some(dir) = std::env::current_exe().ok().and_then(|e| e.parent().map(|p| p.to_path_buf())) {
        let bundled = dir.join("../Resources/framework/system-prompt.md");
        if bundled.exists() {
            return bundled;
        }
        #[cfg(windows)]
        {
            let installed = dir.join("framework").join("system-prompt.md");
            if installed.exists() {
                return installed;
            }
        }
    }
    PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../../framework/system-prompt.md"))
}

/// The framework text, or a short error.
pub fn framework() -> Result<String, String> {
    let p = framework_file();
    std::fs::read_to_string(&p).map_err(|e| format!("cannot read the framework {}: {e}", p.display()))
}
