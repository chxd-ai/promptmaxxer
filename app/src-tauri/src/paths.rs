//! Fixed file locations. Nothing here points into Documents at runtime: macOS blocks background apps there.
use std::path::PathBuf;

pub fn home() -> PathBuf {
    std::env::var_os("HOME").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("/tmp"))
}

/// `~/.local/share/promptmaxxer` (shared with the runner and later the settings file).
pub fn share_dir() -> PathBuf {
    home().join(".local/share/promptmaxxer")
}

pub fn history_file() -> PathBuf {
    share_dir().join("history.jsonl")
}

pub fn log_file() -> PathBuf {
    home().join("Library/Logs/promptmaxxer.log")
}

/// The framework (system prompt) bundled with the app: `<app>.app/Contents/Resources/framework/system-prompt.md`.
/// A dev build run from `target/` falls back to the repo copy.
pub fn framework_file() -> PathBuf {
    if let Some(dir) = std::env::current_exe().ok().and_then(|e| e.parent().map(|p| p.to_path_buf())) {
        let bundled = dir.join("../Resources/framework/system-prompt.md");
        if bundled.exists() {
            return bundled;
        }
    }
    PathBuf::from(concat!(env!("CARGO_MANIFEST_DIR"), "/../../framework/system-prompt.md"))
}

/// The framework text, or a short error.
pub fn framework() -> Result<String, String> {
    let p = framework_file();
    std::fs::read_to_string(&p).map_err(|e| format!("cannot read the framework {}: {e}", p.display()))
}
