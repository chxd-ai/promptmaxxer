//! promptmaxxer: the menu bar app. Library half, so tests and the headless mode share the code.
//! - `press`: one press, start to end (copy, context, engine, floating bar, paste, history).
//! - `selection`: copy the selection and put the result back; `mac`: macOS calls.
//! - `context`: target, project, sources, brief (pmx-context); engines come from pmx-engines.
//! - `bar`, `windows`, `tray`, `keys`, `commands`: the UI side.
//! - `settings`, `history`, `log`, `paths`: files on disk.

#[cfg(target_os = "macos")]
pub mod bar;
#[cfg(target_os = "macos")]
pub mod commands;
pub mod context;
pub mod headless;
pub mod history;
#[cfg(target_os = "macos")]
pub mod keys;
pub mod log;
#[cfg(target_os = "macos")]
pub mod mac;
pub mod paths;
#[cfg(target_os = "macos")]
pub mod press;
#[cfg(target_os = "macos")]
pub mod selection;
pub mod settings;
#[cfg(target_os = "macos")]
pub mod tray;
#[cfg(target_os = "macos")]
pub mod windows;

/// A notification from the app itself (BRAND.md messages).
#[cfg(target_os = "macos")]
pub fn notify(app: &tauri::AppHandle, msg: &str) {
    use tauri_plugin_notification::NotificationExt;
    if let Err(e) = app.notification().builder().title("promptmaxxer").body(msg).show() {
        log::log(&format!("notify: failed ({e})"));
    }
}

#[cfg(target_os = "macos")]
pub fn run() {
    use tauri::Manager;
    log::log(&format!("start: promptmaxxer {} launching (settings {})", env!("CARGO_PKG_VERSION"), settings::path().display()));
    let app = tauri::Builder::default()
        .plugin(
            tauri_plugin_global_shortcut::Builder::new()
                .with_handler(|_app, shortcut, event| keys::handle(shortcut, event.state()))
                .build(),
        )
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_autostart::init(tauri_plugin_autostart::MacosLauncher::LaunchAgent, Some(vec!["--background"])))
        .invoke_handler(tauri::generate_handler![
            commands::get_settings,
            commands::save_settings,
            commands::list_engines,
            commands::test_engine,
            commands::save_api_key,
            commands::delete_api_key,
            commands::history_list,
            commands::history_delete,
            commands::history_copy,
            commands::history_rerun,
            commands::history_thumb,
            commands::check_accessibility,
            commands::open_accessibility_settings,
            commands::finish_onboarding,
            commands::bar_cancel,
            commands::bar_undo,
            commands::bar_thumb,
            commands::open_settings,
            commands::open_history,
            commands::context_preview,
        ])
        .setup(|app| {
            app.set_activation_policy(tauri::ActivationPolicy::Accessory);
            press::set_app(app.handle().clone());
            tray::build(app)?;
            bar::create(app.handle())?;
            let s = settings::load();
            if let Err(e) = keys::apply(app.handle(), &s.fast_key, &s.max_key) {
                log::log(&format!("start: keys: {e}"));
            }
            // Which AI is ready: in the background, so the menu bar icon shows at once.
            tauri::async_runtime::spawn_blocking(|| {
                let st = press::refresh_statuses();
                let ready: Vec<_> = st.iter().filter(|s| s.ready).map(|s| s.id.as_str()).collect();
                log::log(&format!("start: engines ready: {ready:?}"));
            });
            if !s.onboarded {
                log::log("start: not onboarded yet, opening the welcome window");
                windows::open(app.handle(), "onboarding")?;
            } else if !std::env::args().any(|a| a == "--background") && std::env::var("PMX_OPEN").is_err() {
                // Opened by the person (Applications, Spotlight, Dock): show the app. Start at login stays quiet.
                windows::open(app.handle(), "settings")?;
            }
            // PMX_OPEN=settings,history: open these windows at launch without taking the focus (screenshots).
            if let Ok(list) = std::env::var("PMX_OPEN") {
                for label in list.split(',').map(str::trim).filter(|l| !l.is_empty()) {
                    let _ = windows::open_with(app.handle(), label, false);
                }
            }
            if !mac::accessibility_trusted() {
                log::log("start: Accessibility not granted yet");
            }
            let _ = app.handle().get_webview_window(bar::LABEL);
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("tauri app");
    app.run(|app, event| match event {
        // Menu bar app: closing the last window never quits.
        tauri::RunEvent::ExitRequested { api, code: None, .. } => api.prevent_exit(),
        // Opened again from Applications, Spotlight or the Dock while running: show the app window.
        tauri::RunEvent::Reopen { .. } => {
            let label = if settings::load().onboarded { "settings" } else { "onboarding" };
            let _ = windows::open(app, label);
        }
        _ => {}
    });
}
