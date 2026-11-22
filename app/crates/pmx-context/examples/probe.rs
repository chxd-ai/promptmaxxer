//! Print what pmx-context sees for the app in front.
//!
//!   cargo run --example probe                 # now
//!   cargo run --example probe -- --wait 3     # after 3 seconds (time to switch apps)
//!   cargo run --example probe -- --pin DIR    # with a pinned project
//!
//! Never prints file contents, only paths, so no secret can show.

use pmx_context::{chip_label, detect_project, diag, fingerprint, front_app, sources, target_for, SourcePrefs};
use std::path::PathBuf;
use std::time::Instant;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let mut wait = 0.0f64;
    let mut pin: Option<PathBuf> = None;
    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--wait" => {
                wait = args.get(i + 1).and_then(|s| s.parse().ok()).unwrap_or(0.0);
                i += 1;
            }
            "--pin" => {
                pin = args.get(i + 1).map(PathBuf::from);
                i += 1;
            }
            _ => {}
        }
        i += 1;
    }
    if wait > 0.0 {
        std::thread::sleep(std::time::Duration::from_secs_f64(wait));
    }

    println!("Permissions: accessibility={} screen_recording={}", diag::accessibility_trusted(), diag::screen_recording_allowed());
    let t0 = Instant::now();
    let Some(app) = front_app() else {
        println!("AppInfo: None");
        return;
    };
    println!("AppInfo: {app:?}");
    let (title, title_src) = diag::window_title_with_source(&app);
    println!("Window title: {title:?} (via {title_src})");
    let target = target_for(&app, title.as_deref());
    println!("Target: {target:?} hint={:?}", target.hint());
    let (project, proj_src) = diag::detect_project_with_source(&app, title.as_deref(), pin.as_deref());
    match &project {
        Some(p) => println!("Project: root={} cwd={} name={} (via {proj_src})", p.root.display(), p.cwd.display(), p.name),
        None => println!("Project: None (via {proj_src})"),
    }
    let srcs = sources(project.as_ref(), &SourcePrefs::all_on());
    println!("Sources ({}):", srcs.len());
    for s in &srcs {
        println!("  {:<8} {:<16} {}", format!("{:?}", s.level), s.label, s.path.display());
    }
    println!("Fingerprint: {}", fingerprint(&srcs));
    println!("Chip: {}", chip_label(project.as_ref(), &srcs));
    println!("Local work: {} ms", t0.elapsed().as_millis());
    // Same answer through the plain API.
    assert_eq!(project, detect_project(&app, title.as_deref(), pin.as_deref()));
}
