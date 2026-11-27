// promptmaxxer: menu bar app. `promptmaxxer --rewrite ...` runs one rewrite headless (no UI, no Accessibility).
// Windows release builds are GUI programs: no console window opens with the tray app. Piped stdin and stdout
// (the headless mode) still work.
#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]
fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "--rewrite") {
        std::process::exit(promptmaxxer_lib::headless::run(&args));
    }
    #[cfg(any(target_os = "macos", windows))]
    promptmaxxer_lib::run();
}
