//! Global keys: the fast and max shortcuts from settings (re-registered live when settings change), and Esc,
//! which is registered only while a press runs.
use crate::log::log;
use pmx_engines::Level;
use std::str::FromStr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;
use tauri::AppHandle;
use tauri_plugin_global_shortcut::{Code, GlobalShortcutExt, Shortcut, ShortcutState};

/// (fast, max) as registered now.
static KEYS: Mutex<Option<(Shortcut, Shortcut)>> = Mutex::new(None);
static FAST_HELD: AtomicBool = AtomicBool::new(false);
static MAX_HELD: AtomicBool = AtomicBool::new(false);
static ESC_ON: AtomicBool = AtomicBool::new(false);

fn esc_key() -> Shortcut {
    Shortcut::new(None, Code::Escape)
}

/// "Alt+Space", "Ctrl+Shift+Space", "Ctrl+Alt+Cmd+P" (the settings format) to a shortcut.
pub fn parse(s: &str) -> Result<Shortcut, String> {
    let norm: Vec<String> = s
        .split('+')
        .map(|p| p.trim())
        .filter(|p| !p.is_empty())
        .map(|p| match p.to_lowercase().as_str() {
            "cmd" | "command" | "super" | "meta" => "Super".to_string(),
            "ctrl" | "control" => "Control".to_string(),
            "alt" | "option" => "Alt".to_string(),
            _ => p.to_string(),
        })
        .collect();
    Shortcut::from_str(&norm.join("+")).map_err(|e| format!("'{s}': {e}"))
}

/// The plugin's one handler: fast, max or Esc.
pub fn handle(shortcut: &Shortcut, state: ShortcutState) {
    if *shortcut == esc_key() {
        if state == ShortcutState::Pressed {
            crate::press::cancel();
        }
        return;
    }
    let Some((fast, max)) = *KEYS.lock().unwrap() else { return };
    let (held, level) = if *shortcut == fast {
        (&FAST_HELD, Level::Standard)
    } else if *shortcut == max {
        (&MAX_HELD, Level::Max)
    } else {
        return;
    };
    match state {
        ShortcutState::Pressed => {
            if held.swap(true, Ordering::SeqCst) {
                return; // key repeat
            }
            crate::press::on_key(level);
        }
        ShortcutState::Released => held.store(false, Ordering::SeqCst),
    }
}

/// Register the two keys from settings, replacing the old ones. Errors are logged and returned.
pub fn apply(app: &AppHandle, fast: &str, max: &str) -> Result<(), String> {
    let (f, m) = (parse(fast)?, parse(max)?);
    if f == m {
        return Err("The fast and max keys must differ.".into());
    }
    let gs = app.global_shortcut();
    let old = KEYS.lock().unwrap().take();
    if let Some((of, om)) = old {
        let _ = gs.unregister(of);
        let _ = gs.unregister(om);
    }
    let mut errs = vec![];
    let mut done = vec![];
    for (name, k) in [(fast, f), (max, m)] {
        match gs.register(k) {
            Ok(()) => {
                log(&format!("keys: registered {name}"));
                done.push(k);
            }
            Err(e) => {
                log(&format!("keys: could not register {name}: {e}"));
                errs.push(format!("{name}: {e}"));
            }
        }
    }
    if errs.is_empty() {
        *KEYS.lock().unwrap() = Some((f, m));
        return Ok(());
    }
    // Roll back: drop the half-set new pair and put the old pair back on.
    for k in done {
        let _ = gs.unregister(k);
    }
    if let Some((of, om)) = old {
        let ok = gs.register(of).is_ok() & gs.register(om).is_ok();
        log(&format!("keys: rolled back to the old keys ({})", if ok { "ok" } else { "failed" }));
        *KEYS.lock().unwrap() = Some((of, om));
    }
    Err(errs.join("; "))
}

/// Esc on while busy, off after.
pub fn esc(on: bool) {
    let Some(app) = crate::press::app() else { return };
    if ESC_ON.swap(on, Ordering::SeqCst) == on {
        return;
    }
    let gs = app.global_shortcut();
    let r = if on { gs.register(esc_key()) } else { gs.unregister(esc_key()) };
    if let Err(e) = r {
        log(&format!("keys: Esc {}: {e}", if on { "register" } else { "unregister" }));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tauri_plugin_global_shortcut::Modifiers;

    #[test]
    fn parses_the_settings_format() {
        assert_eq!(parse("Alt+Space").unwrap(), Shortcut::new(Some(Modifiers::ALT), Code::Space));
        assert_eq!(parse("Ctrl+Shift+Space").unwrap(), Shortcut::new(Some(Modifiers::CONTROL | Modifiers::SHIFT), Code::Space));
        assert_eq!(
            parse("Ctrl+Alt+Cmd+P").unwrap(),
            Shortcut::new(Some(Modifiers::CONTROL | Modifiers::ALT | Modifiers::SUPER), Code::KeyP)
        );
        assert_eq!(parse("Ctrl+Option+Space").unwrap(), parse("Ctrl+Alt+Space").unwrap());
        assert!(parse("Ctrl+Nope").is_err());
    }
}
