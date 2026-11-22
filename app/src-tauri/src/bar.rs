//! The floating bar: a transparent, always-on-top 560 x 220 window (`ui/bar.html`) that never takes the focus
//! from the app being edited. It is shown with `orderFrontRegardless` (no activation), placed under the selection
//! (Accessibility bounds) or near the mouse, kept on screen, and fed `pmx:bar` events.
use serde::Serialize;
use std::sync::atomic::{AtomicU64, Ordering};
use tauri::{AppHandle, Emitter, LogicalPosition, Manager, WebviewUrl, WebviewWindow, WebviewWindowBuilder};

pub const LABEL: &str = "bar";
const W: f64 = 560.0;
const H: f64 = 220.0;
/// Where the card starts inside the window (bar.html body padding).
const PAD_LEFT: f64 = 14.0;
const PAD_TOP: f64 = 14.0;
pub const HIDE_AFTER_MS: u64 = 2500;

#[derive(Debug, Clone, Serialize, Default)]
pub struct Payload {
    pub phase: &'static str,
    pub level: &'static str,
    pub engine: String,
    pub model: String,
    pub target: String,
    pub chip: String,
    /// Full text so far.
    pub text: String,
    pub message: String,
    pub ms: u64,
}

pub fn create(app: &AppHandle) -> tauri::Result<WebviewWindow> {
    WebviewWindowBuilder::new(app, LABEL, WebviewUrl::App("bar.html".into()))
        .title("promptmaxxer")
        .inner_size(W, H)
        .resizable(false)
        .decorations(false)
        .transparent(true)
        .shadow(false)
        .always_on_top(true)
        .visible_on_all_workspaces(true)
        .skip_taskbar(true)
        .visible(false)
        .focused(false)
        .focusable(false)
        .accept_first_mouse(true)
        .build()
}

fn window(app: &AppHandle) -> Option<WebviewWindow> {
    app.get_webview_window(LABEL)
}

/// Each show bumps this; a delayed hide only runs if no newer show happened.
static SHOWN: AtomicU64 = AtomicU64::new(0);

/// Work areas of all screens in logical points, origin top-left (same space as Accessibility and CGEvent).
fn screens(app: &AppHandle) -> Vec<(f64, f64, f64, f64)> {
    app.available_monitors()
        .unwrap_or_default()
        .iter()
        .map(|m| {
            let s = m.scale_factor();
            let wa = m.work_area();
            (wa.position.x as f64 / s, wa.position.y as f64 / s, wa.size.width as f64 / s, wa.size.height as f64 / s)
        })
        .collect()
}

/// Window position for an anchor: under the selection rectangle (above it when there is no room), or beside
/// the mouse; clamped into the work area of the screen that holds the anchor.
pub fn place(anchor: Anchor, screens: &[(f64, f64, f64, f64)]) -> (f64, f64) {
    let (ax, ay, ah) = match anchor {
        Anchor::Selection { x, y, w: _, h } => (x, y, h),
        Anchor::Mouse { x, y } => (x + 12.0, y + 6.0, 12.0),
    };
    let scr = screens
        .iter()
        .copied()
        .find(|(sx, sy, sw, sh)| ax >= *sx && ax < sx + sw && ay >= *sy && ay < sy + sh)
        .or_else(|| screens.first().copied())
        .unwrap_or((0.0, 0.0, 1440.0, 900.0));
    let (sx, sy, sw, sh) = scr;
    let mut x = ax - PAD_LEFT;
    let mut y = ay + ah + 8.0 - PAD_TOP;
    if y + H > sy + sh {
        y = ay - H - 4.0 + PAD_TOP; // above the selection
    }
    x = x.clamp(sx, (sx + sw - W).max(sx));
    y = y.clamp(sy, (sy + sh - H).max(sy));
    (x.round(), y.round())
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Anchor {
    Selection { x: f64, y: f64, w: f64, h: f64 },
    Mouse { x: f64, y: f64 },
}

/// Show without activating promptmaxxer: the app being edited keeps the keyboard focus.
pub fn show(app: &AppHandle, anchor: Anchor) {
    let Some(w) = window(app) else { return };
    SHOWN.fetch_add(1, Ordering::SeqCst);
    let (x, y) = place(anchor, &screens(app));
    let _ = w.set_position(LogicalPosition::new(x, y));
    let w2 = w.clone();
    let _ = app.run_on_main_thread(move || {
        if let Ok(ptr) = w2.ns_window() {
            // SAFETY: a live NSWindow owned by tao, used on the main thread.
            unsafe {
                let ns = ptr as *mut objc2::runtime::AnyObject;
                // On every Space and over full-screen apps (terminals often run full screen):
                // canJoinAllSpaces (1 << 0) | stationary (1 << 4) | ignoresCycle (1 << 6) | fullScreenAuxiliary (1 << 8).
                let behavior: usize = (1 << 0) | (1 << 4) | (1 << 6) | (1 << 8);
                let _: () = objc2::msg_send![ns, setCollectionBehavior: behavior];
                // NSStatusWindowLevel (25): above normal and floating windows, like a menu bar popover.
                let _: () = objc2::msg_send![ns, setLevel: 25isize];
                let _: () = objc2::msg_send![ns, orderFrontRegardless];
            }
        }
    });
}

pub fn emit(app: &AppHandle, p: &Payload) {
    let _ = app.emit("pmx:bar", p);
}

pub fn hide_now(app: &AppHandle) {
    if let Some(w) = window(app) {
        let _ = w.hide();
    }
}

pub fn hide_later(app: &AppHandle, ms: u64) {
    let gen = SHOWN.load(Ordering::SeqCst);
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(ms)).await;
        if SHOWN.load(Ordering::SeqCst) == gen && !crate::press::is_busy() {
            hide_now(&app);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    const SCR: [(f64, f64, f64, f64); 1] = [(0.0, 25.0, 1440.0, 875.0)];

    #[test]
    fn goes_under_the_selection() {
        let (x, y) = place(Anchor::Selection { x: 300.0, y: 200.0, w: 120.0, h: 18.0 }, &SCR);
        assert_eq!((x, y), (286.0, 212.0));
    }

    #[test]
    fn flips_above_near_the_bottom_and_stays_on_screen() {
        let (x, y) = place(Anchor::Selection { x: 1400.0, y: 820.0, w: 30.0, h: 18.0 }, &SCR);
        assert!(y + H <= 900.0 && y < 820.0, "above the selection: {y}");
        assert_eq!(x, 1440.0 - W);
        let (x, y) = place(Anchor::Mouse { x: -50.0, y: 0.0 }, &SCR);
        assert_eq!((x, y), (0.0, 25.0));
    }

    #[test]
    fn picks_the_screen_that_holds_the_anchor() {
        let two = [(0.0, 0.0, 1440.0, 900.0), (1440.0, 0.0, 1920.0, 1080.0)];
        let (x, _) = place(Anchor::Mouse { x: 3300.0, y: 500.0 }, &two);
        assert_eq!(x, 1440.0 + 1920.0 - W);
    }
}
