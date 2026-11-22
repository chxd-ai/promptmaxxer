//! The menu bar icon and menu: title, current engine, Settings…, History…, Quit. No Dock icon.
use crate::log::log;
use std::sync::OnceLock;
use tauri::image::Image;
use tauri::menu::{Menu, MenuItem, PredefinedMenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::{App, Wry};

static ENGINE_ITEM: OnceLock<MenuItem<Wry>> = OnceLock::new();

pub fn set_engine_label(text: &str) {
    if let Some(item) = ENGINE_ITEM.get() {
        let _ = item.set_text(text);
    }
}

pub fn build(app: &App) -> tauri::Result<()> {
    let title = MenuItem::with_id(app, "title", "promptmaxxer", false, None::<&str>)?;
    let eng = MenuItem::with_id(app, "engine", "Engine: checking…", false, None::<&str>)?;
    let settings = MenuItem::with_id(app, "settings", "Settings…", true, Some("Cmd+,"))?;
    let history = MenuItem::with_id(app, "history", "History…", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "Quit", true, Some("Cmd+Q"))?;
    let (s1, s2) = (PredefinedMenuItem::separator(app)?, PredefinedMenuItem::separator(app)?);
    let menu = Menu::with_items(app, &[&title, &eng, &s1, &settings, &history, &s2, &quit])?;
    let _ = ENGINE_ITEM.set(eng);
    TrayIconBuilder::with_id("promptmaxxer")
        .icon(Image::from_bytes(include_bytes!("../icons/tray-36.png"))?)
        .icon_as_template(true)
        .tooltip("promptmaxxer")
        .menu(&menu)
        .show_menu_on_left_click(true)
        .on_menu_event(|app, e| match e.id.as_ref() {
            id @ ("settings" | "history") => {
                log(&format!("menu: {id}"));
                if let Err(e) = crate::windows::open(app, id) {
                    log(&format!("menu: could not open {id}: {e}"));
                }
            }
            "quit" => {
                log("quit: from the menu");
                app.exit(0);
            }
            _ => {}
        })
        .build(app)?;
    Ok(())
}
