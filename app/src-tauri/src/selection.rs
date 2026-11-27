//! The press's two ends: copy the selection, and put the result back.
//! - `capture()`: remember the front app, save the whole clipboard, Cmd+C, wait for the copy.
//! - `deliver(captured, text)`: paste into the same app and window (then restore the old clipboard about 1 s
//!   later), or leave the text on the clipboard for terminals and when the front app or window changed.
//! - `abandon(captured)`: failure path, the old clipboard goes back and the text in the app is untouched.
//! The middle (context, engine, floating bar) is `press.rs`.
use crate::log::log;
use crate::mac::{self, AppInfo, SavedClipboard};
use std::sync::Mutex;
use std::time::{Duration, Instant};

pub const COPY_TIMEOUT: Duration = Duration::from_millis(600);
/// Time an app gets to read the pasted text before the old clipboard goes back. Slow apps (Electron, big
/// documents) read the clipboard several hundred ms after Cmd+V; at 300 ms some pasted the old clipboard.
pub const PASTE_SETTLE: Duration = Duration::from_millis(1000);
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
    /// The app's front window at the press (macOS window number, Windows HWND).
    window: Option<u64>,
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

/// A clipboard to put back after a paste: (sequence, clipboard change count right after our text went on, old clipboard).
static PENDING_RESTORE: Mutex<Option<(u64, isize, SavedClipboard)>> = Mutex::new(None);

/// Put the old clipboard back now if a restore is pending (`seq`: only that one), and only if the clipboard
/// still holds our text: a copy made since is the person's and stays.
fn finish_restore(seq: Option<u64>) {
    let pending = {
        let mut g = PENDING_RESTORE.lock().unwrap_or_else(|e| e.into_inner());
        if seq.is_some() && g.as_ref().map(|p| p.0) != seq {
            return;
        }
        g.take()
    };
    if let Some((_, ours, saved)) = pending {
        if restore_allowed(ours, mac::change_count()) {
            mac::restore_clipboard(&saved);
        } else {
            log("deliver: clipboard changed after the paste, keeping the new copy");
        }
    }
}

/// The old clipboard goes back only while the clipboard still holds our text (no change since we set it).
fn restore_allowed(ours: isize, now: isize) -> bool {
    ours == now
}

/// The paste may go ahead only into the window the press came from. Unknown at the press: no check.
fn same_window(at_press: Option<u64>, now: Option<u64>) -> bool {
    at_press.is_none() || at_press == now
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
    // A restore still waiting from the last paste goes first, so its old clipboard is the one saved now.
    finish_restore(None);
    if !mac::accessibility_trusted() {
        mac::ask_accessibility();
        return Err(CaptureError::NoAccessibility);
    }
    let app = mac::front_app().ok_or(CaptureError::NoFrontApp)?;
    let window = mac::front_window(app.pid);
    // Wait until the shortcut's keys are up, so Cmd+C is not read as Cmd+Option+C.
    if !wait_until(MODIFIER_WAIT, || !mac::modifiers_held()).await {
        log("capture: modifiers still held after 3 s, copying anyway");
    }
    let saved = mac::save_clipboard();
    let before = mac::change_count();
    #[cfg(target_os = "macos")]
    let copy = mac::cmd_key(mac::KEY_C);
    #[cfg(windows)]
    let copy = mac::copy_selection(&app);
    if let Err(e) = copy {
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
    Ok(Captured { app, window, draft, saved, copied_count: mac::change_count() })
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
        mac::activate_to(c.app.pid, c.window);
        wait_until(Duration::from_millis(600), || mac::front_app().is_some_and(|n| n.pid == c.app.pid)).await;
        tokio::time::sleep(Duration::from_millis(80)).await;
        now = mac::front_app();
    }
    let same_app = now.as_ref().is_some_and(|n| n.pid == c.app.pid);
    let terminal = mac::is_terminal(&c.app) || (mac::is_mixed_terminal_app(&c.app) && mac::focused_is_terminal(c.app.pid));
    // Same app but another window (a new document, another chat): the paste would land in the wrong field.
    let window_ok = same_app && same_window(c.window, mac::front_window(c.app.pid));
    if terminal || !same_app || !window_ok {
        mac::set_clipboard_text(text);
        let reason = if !same_app {
            "app changed"
        } else if !window_ok {
            "window changed"
        } else {
            "terminal"
        };
        return Delivery::Clipboard { reason };
    }
    wait_until(Duration::from_secs(1), || !mac::modifiers_held()).await;
    // Something copied during the run is the clipboard to give back, not the one from before the press.
    let user_copied = mac::change_count() != c.copied_count;
    let restore = if user_copied { mac::save_clipboard() } else { c.saved };
    mac::set_clipboard_text(text);
    let ours = mac::change_count();
    if let Err(e) = mac::cmd_key(mac::KEY_V) {
        // The result is on the clipboard; keep it there rather than restoring over it.
        log(&format!("deliver: Cmd+V failed: {e}"));
        return Delivery::Clipboard { reason: "paste failed" };
    }
    // Let the app read the pasteboard before the old contents go back. The wait runs on its own so the bar
    // says "Pasted" now; the next press finishes it early.
    use std::sync::atomic::{AtomicU64, Ordering};
    static SEQ: AtomicU64 = AtomicU64::new(0);
    let seq = SEQ.fetch_add(1, Ordering::Relaxed);
    *PENDING_RESTORE.lock().unwrap_or_else(|e| e.into_inner()) = Some((seq, ours, restore));
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(PASTE_SETTLE).await;
        finish_restore(Some(seq));
    });
    Delivery::Pasted
}

#[cfg(test)]
mod rule_tests {
    use super::*;

    #[test]
    fn restore_and_window_rules() {
        assert!(restore_allowed(7, 7));
        assert!(!restore_allowed(7, 8), "a copy made after the paste stays");
        assert!(same_window(Some(42), Some(42)));
        assert!(!same_window(Some(42), Some(43)));
        assert!(!same_window(Some(42), None));
        assert!(same_window(None, Some(43)), "unknown at the press: no check");
        assert!(PASTE_SETTLE >= Duration::from_millis(900));
    }
}

// The app lists checked here are the macOS ones; win.rs tests its own.
#[cfg(all(test, target_os = "macos"))]
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
