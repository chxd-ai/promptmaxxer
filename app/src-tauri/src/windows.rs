//! The three normal windows (settings, history, onboarding): one each, reused, brought to the front.
//! Windows: normal title bars; the app shows in the taskbar while one of these is open (Windows does that itself).
use tauri::{AppHandle, Manager, WebviewUrl, WebviewWindowBuilder};
#[cfg(target_os = "macos")]
use tauri::{ActivationPolicy, TitleBarStyle, WindowEvent};

#[cfg(target_os = "macos")]
const LABELS: [&str; 3] = ["settings", "history", "onboarding"];

fn spec(label: &str) -> Option<(&'static str, f64, f64)> {
    Some(match label {
        "settings" => ("promptmaxxer settings", 820.0, 760.0),
        "history" => ("History", 1180.0, 820.0),
        "onboarding" => ("Welcome to promptmaxxer", 600.0, 760.0),
        _ => return None,
    })
}

pub fn open(app: &AppHandle, label: &str) -> tauri::Result<()> {
    open_with(app, label, true)
}

/// `focus: false` shows the window without taking the keyboard from the app in front (PMX_OPEN screenshots).
pub fn open_with(app: &AppHandle, label: &str, focus: bool) -> tauri::Result<()> {
    let Some((title, w, h)) = spec(label) else { return Ok(()) };
    let win = match app.get_webview_window(label) {
        Some(win) => win,
        None => {
            let b = WebviewWindowBuilder::new(app, label, WebviewUrl::App(format!("{label}.html").into()))
                .title(title)
                .inner_size(w, h)
                .min_inner_size(w.min(600.0), 520.0);
            #[cfg(target_os = "macos")]
            let b = b.title_bar_style(TitleBarStyle::Overlay).hidden_title(true);
            b.center().visible(true).focused(focus).build()?
        }
    };
    #[cfg(target_os = "macos")]
    mac_policy(app, &win, label, focus);
    let _ = win.unminimize();
    // Put it in front of the person: on the screen under the mouse and on the desktop (Space) they are on now.
    // Without this, with two screens or a full-screen app, the window opens somewhere out of view and the
    // menu looks dead.
    center_on_mouse_screen(app, &win);
    let _ = win.show();
    #[cfg(target_os = "macos")]
    mac_front(app, &win, focus);
    if focus {
        let _ = win.set_focus();
    }
    Ok(())
}

/// Dock icon while a normal window is open; menu bar only after the last one closes.
#[cfg(target_os = "macos")]
fn mac_policy(app: &AppHandle, win: &tauri::WebviewWindow, label: &str, focus: bool) {
    if focus {
        // A normal window is open: show promptmaxxer in the Dock and app switcher, like any app.
        let _ = app.set_activation_policy(ActivationPolicy::Regular);
    }
    let handle = app.clone();
    let me = label.to_string();
    win.on_window_event(move |e| {
        if let WindowEvent::Destroyed = e {
            // Last normal window closed: back to a menu bar only app (no Dock icon).
            let others = LABELS.iter().any(|l| *l != me && handle.get_webview_window(l).is_some());
            if !others {
                let _ = handle.set_activation_policy(ActivationPolicy::Accessory);
            }
        }
    });
}

/// On the Space in view and in front, even though a menu bar app is never the active app.
#[cfg(target_os = "macos")]
fn mac_front(app: &AppHandle, win: &tauri::WebviewWindow, focus: bool) {
    let w2 = win.clone();
    let _ = app.run_on_main_thread(move || {
        if let Ok(ptr) = w2.ns_window() {
            // SAFETY: a live NSWindow owned by tao, used on the main thread.
            unsafe {
                let ns = ptr as *mut objc2::runtime::AnyObject;
                // moveToActiveSpace (1 << 1) | fullScreenAuxiliary (1 << 8): the window comes to the Space in view
                // when promptmaxxer is activated, instead of pulling the person to another desktop.
                let behavior: usize = (1 << 1) | (1 << 8);
                let _: () = objc2::msg_send![ns, setCollectionBehavior: behavior];
                if focus {
                    // A menu bar app is never the active app: activate it, or the window opens behind everything.
                    let ns_app: *mut objc2::runtime::AnyObject =
                        objc2::msg_send![objc2::class!(NSApplication), sharedApplication];
                    let _: () = objc2::msg_send![ns_app, activateIgnoringOtherApps: true];
                    let _: () = objc2::msg_send![ns, makeKeyAndOrderFront: std::ptr::null::<objc2::runtime::AnyObject>()];
                } else {
                    let _: () = objc2::msg_send![ns, orderFrontRegardless];
                }
            }
        }
    });
}

/// Centre the window on the monitor that holds the mouse pointer.
fn center_on_mouse_screen(app: &AppHandle, win: &tauri::WebviewWindow) {
    let Some((mx, my)) = crate::mac::mouse_location() else { return };
    let Ok(monitors) = app.available_monitors() else { return };
    for m in monitors {
        let sf = m.scale_factor();
        let pos = m.position().to_logical::<f64>(sf);
        let size = m.size().to_logical::<f64>(sf);
        if mx >= pos.x && mx < pos.x + size.width && my >= pos.y && my < pos.y + size.height {
            let Ok(outer) = win.outer_size() else { return };
            let ws = outer.to_logical::<f64>(win.scale_factor().unwrap_or(sf));
            let x = pos.x + ((size.width - ws.width) / 2.0).max(0.0);
            let y = pos.y + ((size.height - ws.height) / 2.0).max(0.0);
            let _ = win.set_position(tauri::LogicalPosition::new(x, y));
            return;
        }
    }
}

pub fn close(app: &AppHandle, label: &str) {
    if let Some(w) = app.get_webview_window(label) {
        let _ = w.close();
    }
}
