// promptmaxxer: menu bar app. `promptmaxxer --rewrite ...` runs one rewrite headless (no UI, no Accessibility).
fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "--rewrite") {
        std::process::exit(promptmaxxer_lib::headless::run(&args));
    }
    #[cfg(target_os = "macos")]
    promptmaxxer_lib::run();
}
