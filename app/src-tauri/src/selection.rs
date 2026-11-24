//! The press's two ends: copy the selection, and put the result back.
//! - `capture()`: remember the front app, save the whole clipboard, Cmd+C, wait for the copy.
//! - `deliver(captured, text)`: paste into the same app (then restore the old clipboard), or leave the text on the
//!   clipboard for terminals and when the front app changed.
//! - `abandon(captured)`: failure path, the old clipboard goes back and the text in the app is untouched.
//! The middle (context, engine, floating bar) is `press.rs`.
use crate::log::log;
use crate::mac::{self, AppInfo, SavedClipboard};
use std::time::{Duration, Instant};

pub const COPY_TIMEOUT: Duration = Duration::from_millis(600);
pub const PASTE_SETTLE: Duration = Duration::from_millis(300);
const MODIFIER_WAIT: Duration = Duration::from_secs(3);
const POLL: Duration = Duration::from_millis(15);

/// A copy-on-select copy this recent counts as the selection.
const COPY_ON_SELECT_MAX_AGE: Duration = Duration::from_secs(60);

/// Terminal panes may run Claude Code's fullscreen view, which copies on select. Orca chat boxes do not count:
/// there a press with nothing selected must not paste an old clipboard at the cursor.
fn copy_on_select_app(app: &AppInfo) -> bool {
    mac::is_terminal(app) || (mac::is_mixed_terminal_app(app) && mac::focused_is_terminal(app.pid))
}

pub const MSG_NOTHING: &str = "Highlight some text first.";
pub const MSG_FAILED: &str = "Couldn't maxx this one. Your text is unchanged.";
pub const MSG_CLIPBOARD: &str = "Maxxed. It's on your clipboard.";
pub const MSG_ACCESSIBILITY: &str = "Allow promptmaxxer in System Settings > Privacy & Security > Accessibility, then press again.";
pub const MSG_PASTED: &str = "Maxxed. Pasted in place.";
pub const MSG_NO_ENGINE: &str = "Pick an AI in promptmaxxer settings.";

pub struct Captured {
    pub app: AppInfo,
    pub draft: String,
    saved: SavedClipboard,
    /// Clipboard change count right after our Cmd+C. A different count later means the user copied something.
    copied_count: isize,
}

#[derive(Debug, PartialEq, Eq)]
pub enum CaptureError {
    NoAccessibility,
    NoFrontApp,
    NothingSelected,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Delivery {
    Pasted,
    /// Left on the clipboard. `reason`: "terminal" or "app changed".
    Clipboard { reason: &'static str },
}

async fn wait_until(timeout: Duration, mut check: impl FnMut() -> bool) -> bool {
    let t0 = Instant::now();
    loop {
        if check() {
            return true;
        }
        if t0.elapsed() >= timeout {
            return false;
        }
        tokio::time::sleep(POLL).await;
    }
}

pub async fn capture() -> Result<Captured, CaptureError> {
    if !mac::accessibility_trusted() {
        mac::ask_accessibility();
        return Err(CaptureError::NoAccessibility);
    }
    let app = mac::front_app().ok_or(CaptureError::NoFrontApp)?;
    // Wait until the shortcut's keys are up, so Cmd+C is not read as Cmd+Option+C.
    if !wait_until(MODIFIER_WAIT, || !mac::modifiers_held()).await {
        log("capture: modifiers still held after 3 s, copying anyway");
    }
    let saved = mac::save_clipboard();
    let before = mac::change_count();
    if let Err(e) = mac::cmd_key(mac::KEY_C) {
        log(&format!("capture: Cmd+C failed: {e}"));
    }
    let copied = wait_until(COPY_TIMEOUT, || mac::change_count() != before).await;
    let mut draft = if copied { mac::clipboard_text().unwrap_or_default() } else { String::new() };
    if !copied && copy_on_select_app(&app) && mac::copied_elsewhere_within(COPY_ON_SELECT_MAX_AGE) {
        // Claude Code's fullscreen view copies on select and leaves no terminal selection for Cmd+C:
        // the selection is the text it just put on the clipboard.
        draft = mac::clipboard_text().unwrap_or_default();
        log(&format!("capture: Cmd+C copied nothing, using the copy-on-select clipboard ({} chars)", draft.chars().count()));
    }
    if draft.trim().is_empty() {
        if copied {
            mac::restore_clipboard(&saved);
        }
        log(&format!("capture: nothing selected (Cmd+C changed clipboard: {copied}, app: {})", app.name));
        return Err(CaptureError::NothingSelected);
    }
    Ok(Captured { app, draft, saved, copied_count: mac::change_count() })
}

/// Put the old clipboard back, unless the user copied something new since the press.
pub fn abandon(c: Captured) {
    if mac::change_count() == c.copied_count {
        mac::restore_clipboard(&c.saved);
    } else {
        log("abandon: clipboard changed during the run, keeping the new copy");
    }
}

pub async fn deliver(c: Captured, text: &str) -> Delivery {
    let mut now = mac::front_app();
    // A click on the floating bar makes promptmaxxer the front app: give the focus back first.
    if now.as_ref().is_some_and(|n| n.pid == std::process::id() as i32) && !mac::is_terminal(&c.app) {
        mac::activate(c.app.pid);
        wait_until(Duration::from_millis(600), || mac::front_app().is_some_and(|n| n.pid == c.app.pid)).await;
        tokio::time::sleep(Duration::from_millis(80)).await;
        now = mac::front_app();
    }
    let same_app = now.as_ref().is_some_and(|n| n.pid == c.app.pid);
    let terminal = mac::is_terminal(&c.app) || (mac::is_mixed_terminal_app(&c.app) && mac::focused_is_terminal(c.app.pid));
    if terminal || !same_app {
        mac::set_clipboard_text(text);
        return Delivery::Clipboard { reason: if same_app { "terminal" } else { "app changed" } };
    }
    wait_until(Duration::from_secs(1), || !mac::modifiers_held()).await;
    // Something copied during the run is the clipboard to give back, not the one from before the press.
    let user_copied = mac::change_count() != c.copied_count;
    let restore = if user_copied { mac::save_clipboard() } else { c.saved };
    mac::set_clipboard_text(text);
    if let Err(e) = mac::cmd_key(mac::KEY_V) {
        // The result is on the clipboard; keep it there rather than restoring over it.
        log(&format!("deliver: Cmd+V failed: {e}"));
        return Delivery::Clipboard { reason: "paste failed" };
    }
    // Let the app read the pasteboard before the old contents go back.
    tokio::time::sleep(PASTE_SETTLE).await;
    mac::restore_clipboard(&restore);
    Delivery::Pasted
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn terminals_are_clipboard_only() {
        let app = |name: &str, id: &str| AppInfo { name: name.into(), bundle_id: id.into(), pid: 1 };
        assert!(mac::is_terminal(&app("Ghostty", "com.mitchellh.ghostty")));
        // Orca: chat boxes take a paste; only its terminal panes are clipboard-only (checked at paste time).
        assert!(!mac::is_terminal(&app("Orca", "com.stablyai.orca")));
        assert!(mac::is_mixed_terminal_app(&app("Orca", "com.stablyai.orca")));
        assert!(mac::is_terminal(&app("Terminal", "com.apple.Terminal")));
        assert!(mac::is_terminal(&app("iTerm2", "com.googlecode.iterm2")));
        assert!(mac::is_terminal(&app("Warp", "dev.warp.Warp-Stable")));
        assert!(!mac::is_terminal(&app("Notes", "com.apple.Notes")));
        assert!(!mac::is_terminal(&app("Code", "com.microsoft.VSCode")));
    }
}
