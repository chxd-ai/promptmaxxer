//! The three normal windows (settings, history, onboarding): one each, reused, brought to the front.
use tauri::{ActivationPolicy, AppHandle, Manager, TitleBarStyle, WebviewUrl, WebviewWindowBuilder, WindowEvent};

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
        None => WebviewWindowBuilder::new(app, label, WebviewUrl::App(format!("{label}.html").into()))
            .title(title)
            .inner_size(w, h)
            .min_inner_size(w.min(600.0), 520.0)
            .title_bar_style(TitleBarStyle::Overlay)
            .hidden_title(true)
            .center()
            .visible(true)
            .focused(focus)
            .build()?,
    };
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
    let _ = win.unminimize();
    let _ = win.show();
    if focus {
        let _ = win.set_focus();
    }
    Ok(())
}

pub fn close(app: &AppHandle, label: &str) {
    if let Some(w) = app.get_webview_window(label) {
        let _ = w.close();
    }
}
