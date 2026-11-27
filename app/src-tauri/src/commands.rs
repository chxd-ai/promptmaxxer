//! Tauri commands for the UI.
use crate::log::log;
use crate::settings::{self, Settings};
use crate::{history, mac, press, windows};
use pmx_engines::{EngineId, EngineStatus, Level, Request};
use serde_json::{json, Value};
use tauri::AppHandle;
use tauri_plugin_autostart::ManagerExt;
use tokio_util::sync::CancellationToken;

#[tauri::command]
pub fn get_settings() -> Settings {
    settings::load()
}

#[tauri::command]
pub fn save_settings(app: AppHandle, settings: Settings) -> Result<(), String> {
    let old = settings::load();
    // Set the keys first: a pair that cannot be set is never saved, and the old keys stay on.
    if old.fast_key != settings.fast_key || old.max_key != settings.max_key {
        crate::keys::apply(&app, &settings.fast_key, &settings.max_key)
            .map_err(|e| format!("Not saved. That shortcut could not be set: {e}"))?;
    }
    settings::save(&settings).map_err(|e| format!("Couldn't save the settings: {e}"))?;
    if old.start_at_login != settings.start_at_login {
        let al = app.autolaunch();
        let r = if settings.start_at_login { al.enable() } else { al.disable() };
        log(&format!("settings: start at login {} ({})", settings.start_at_login, if r.is_ok() { "ok" } else { "failed" }));
    }
    if old.engine != settings.engine
        || old.engine_fast != settings.engine_fast
        || old.engine_max != settings.engine_max
        || old.models != settings.models
        || old.custom_api != settings.custom_api
    {
        tauri::async_runtime::spawn_blocking(press::refresh_statuses);
    }
    Ok(())
}

#[tauri::command]
pub async fn list_engines() -> Vec<EngineStatus> {
    tauri::async_runtime::spawn_blocking(press::refresh_statuses).await.unwrap_or_default()
}

#[tauri::command]
pub async fn test_engine(engine: String) -> Value {
    let Ok(id) = engine.parse::<EngineId>() else { return json!({"ok": false, "ms": 0, "model": "", "error": "Unknown engine"}) };
    let fw = match crate::paths::framework() {
        Ok(f) => f,
        Err(e) => return json!({"ok": false, "ms": 0, "model": "", "error": e}),
    };
    match pmx_engines::test_engine(id, &fw).await {
        Ok(o) => {
            log(&format!("test_engine: {} ok in {} ms", id.as_str(), o.ms));
            json!({"ok": true, "ms": o.ms, "model": o.model, "text": o.text.chars().take(200).collect::<String>()})
        }
        Err(e) => {
            log(&format!("test_engine: {} failed: {e}", id.as_str()));
            json!({"ok": false, "ms": 0, "model": id.model(Level::Standard), "error": e.to_string()})
        }
    }
}

/// Checks the key with one real call before it is saved; a key that fails is never stored.
/// `provider`: "anthropic", or "custom-api" for the any-model engine (any key format).
#[tauri::command]
pub async fn save_api_key(provider: String, key: String) -> Value {
    if provider != "anthropic" && provider != pmx_engines::CUSTOM_KEY {
        return json!({"ok": false, "error": "Unknown key type."});
    }
    let key = key.trim().to_string();
    let custom = provider == pmx_engines::CUSTOM_KEY;
    if custom && !settings::load().custom_api.is_set() {
        return json!({"ok": false, "error": "Fill in the API address and the model first."});
    }
    if !custom && !key.starts_with("sk-ant-") {
        return json!({"ok": false, "error": "That doesn't look like an Anthropic key. It starts with sk-ant-."});
    }
    let fw = crate::paths::framework().unwrap_or_default();
    let req = Request {
        draft: "fix the typo in the readme".into(),
        level: Level::Standard,
        app_name: "promptmaxxer settings".into(),
        target: None,
        context_brief: None,
        framework: fw,
    };
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let (engine, opts) = if custom {
        (EngineId::CustomApi, pmx_engines::RunOptions { custom_key: Some(key.clone()), ..Default::default() })
    } else {
        (EngineId::AnthropicKey, pmx_engines::RunOptions { anthropic_key: Some(key.clone()), ..Default::default() })
    };
    match pmx_engines::run_with(engine, req, tx, CancellationToken::new(), opts).await {
        Ok(_) => match pmx_engines::keychain::set(&provider, &key) {
            Ok(()) => {
                log("save_api_key: key checked and saved in the Keychain");
                tauri::async_runtime::spawn_blocking(press::refresh_statuses);
                json!({"ok": true})
            }
            Err(e) => json!({"ok": false, "error": e.to_string()}),
        },
        Err(e) => {
            log(&format!("save_api_key: check failed: {e}"));
            json!({"ok": false, "error": format!("The key didn't work: {e}")})
        }
    }
}

#[tauri::command]
pub fn delete_api_key(provider: String) -> Result<(), String> {
    if provider != "anthropic" && provider != pmx_engines::CUSTOM_KEY {
        return Ok(());
    }
    pmx_engines::keychain::delete(&provider).map_err(|e| e.to_string())?;
    log("delete_api_key: removed from the Keychain");
    tauri::async_runtime::spawn_blocking(press::refresh_statuses);
    Ok(())
}

#[tauri::command]
pub fn history_list(query: String) -> Vec<Value> {
    history::list(&history::path(), &query)
}

#[tauri::command]
pub fn history_delete(id: Option<String>) -> Result<(), String> {
    history::delete(&history::path(), id.as_deref()).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn history_copy(id: String) -> bool {
    match history::get(&history::path(), &id).and_then(|v| v.get("output").and_then(Value::as_str).map(str::to_string)) {
        Some(out) if !out.is_empty() => mac::set_clipboard_text(&out),
        _ => false,
    }
}

#[tauri::command]
pub fn history_rerun(id: String, level: String) -> bool {
    let level = if level == "max" { Level::Max } else { Level::Standard };
    match history::get(&history::path(), &id) {
        Some(item) => {
            press::rerun(item, level);
            true
        }
        None => false,
    }
}

#[tauri::command]
pub fn history_thumb(id: String, up: Option<bool>) -> Result<bool, String> {
    history::thumb(&history::path(), &id, up).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn check_accessibility() -> bool {
    mac::accessibility_trusted()
}

#[tauri::command]
pub fn open_accessibility_settings() {
    if !mac::accessibility_trusted() {
        // Puts promptmaxxer in the list (and shows the system prompt); the switch is the person's click.
        mac::ask_accessibility();
    }
    mac::open_accessibility_pane();
}

#[tauri::command]
pub fn finish_onboarding(app: AppHandle) -> Result<(), String> {
    let mut s = settings::load();
    s.onboarded = true;
    settings::save(&s).map_err(|e| e.to_string())?;
    log("onboarding: finished");
    windows::close(&app, "onboarding");
    Ok(())
}

#[tauri::command]
pub fn bar_cancel(app: AppHandle) {
    if press::is_busy() {
        press::cancel();
    } else {
        crate::bar::hide_now(&app);
    }
}

#[tauri::command]
pub async fn bar_undo(app: AppHandle) -> bool {
    let ok = press::undo().await;
    if ok {
        crate::bar::hide_now(&app);
    }
    ok
}

#[tauri::command]
pub fn bar_thumb(up: bool) -> bool {
    let Some(id) = press::last().and_then(|l| l.history_id) else { return false };
    history::thumb(&history::path(), &id, Some(up)).unwrap_or(false)
}

#[tauri::command]
pub fn open_settings(app: AppHandle) -> Result<(), String> {
    windows::open(&app, "settings").map_err(|e| e.to_string())
}

#[tauri::command]
pub fn open_history(app: AppHandle) -> Result<(), String> {
    windows::open(&app, "history").map_err(|e| e.to_string())
}

#[tauri::command]
pub fn context_preview() -> crate::context::Preview {
    press::preview()
}

/// "macos" or "windows". The UI skips the Accessibility step and shows Windows key names (Ctrl, Alt) on Windows.
#[tauri::command]
pub fn app_platform() -> &'static str {
    if cfg!(windows) {
        "windows"
    } else {
        "macos"
    }
}
