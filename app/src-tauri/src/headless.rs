//! `promptmaxxer --rewrite [--level standard|max] [--app NAME] [--engine ID] [--project DIR] [--target HINT] [--no-brief] < draft.txt`
//! One rewrite with the same engines, target hint and brief as the shortcut, printed to stdout. No UI, no
//! Accessibility. The target comes from `--app` (or `--target`); the project only from `--project`. Unlike a key
//! press, a changed brief is built first and waited for. Settings come from the settings file (`PMX_SETTINGS`).
//! On failure: the draft is printed unchanged, the reason goes to stderr, exit code 1.
use crate::context;
use pmx_engines::{EngineId, Level, Request};
use std::io::{Read, Write};
use std::path::PathBuf;

fn flag(args: &[String], name: &str) -> Option<String> {
    args.iter().position(|a| a == name).and_then(|i| args.get(i + 1)).cloned()
}

pub fn run(args: &[String]) -> i32 {
    let level = match flag(args, "--level").as_deref() {
        None | Some("standard") | Some("fast") => Level::Standard,
        Some("max") => Level::Max,
        Some(_) => {
            eprintln!("promptmaxxer: --level must be standard or max");
            return 2;
        }
    };
    let app = flag(args, "--app").unwrap_or_else(|| "unknown".into());
    let mut raw = String::new();
    if let Err(e) = std::io::stdin().read_to_string(&mut raw) {
        eprintln!("promptmaxxer: cannot read stdin: {e}");
        return 2;
    }
    let fail = |msg: String, raw: &str| {
        eprintln!("promptmaxxer: {msg}");
        print!("{raw}");
        1
    };
    let s = crate::settings::load();
    let engine = match flag(args, "--engine") {
        Some(e) => match e.parse::<EngineId>() {
            Ok(id) => id,
            Err(_) => return fail(format!("unknown engine '{e}' (custom-api, anthropic-key, claude-plan, chatgpt-plan, gemini-plan)"), &raw),
        },
        None => match s.pick_engine(level, &pmx_engines::detect_all()) {
            Some(id) => id,
            None => return fail("no AI is ready. Pick one in promptmaxxer settings.".into(), &raw),
        },
    };
    let framework = match crate::paths::framework() {
        Ok(f) => f,
        Err(e) => return fail(e, &raw),
    };
    let project_dir = flag(args, "--project").map(PathBuf::from);
    let ctx = context::for_app_name(&app, project_dir.as_deref(), &s);
    let target = flag(args, "--target").or_else(|| ctx.target.hint());
    let rt = tokio::runtime::Builder::new_multi_thread().worker_threads(2).enable_all().build().expect("tokio runtime");
    let brief = if args.iter().any(|a| a == "--no-brief") {
        None
    } else {
        let fast = s.pick_engine(Level::Standard, &pmx_engines::detect_all()).unwrap_or(engine);
        rt.block_on(context::brief_wait(&ctx, fast))
    };
    eprintln!(
        "promptmaxxer: engine {} ({}), level {}, app {app}, target {}, chip \"{}\", brief {}",
        engine.label(),
        engine.as_str(),
        if level == Level::Max { "max" } else { "standard" },
        target.as_deref().unwrap_or("none"),
        ctx.chip,
        brief.as_ref().map(|b| format!("{} words", b.split_whitespace().count())).unwrap_or_else(|| "none".into())
    );
    let req = Request { draft: pmx_context::scrub_secrets(&raw), level, app_name: app.clone(), target, context_brief: brief, framework };
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let result = rt.block_on(pmx_engines::run(engine, req, tx, tokio_util::sync::CancellationToken::new()));
    let mut stdout = std::io::stdout();
    match result {
        Ok(o) => {
            crate::log::log(&format!("headless: ok engine={} model={} level={level:?} app={app} ms={} chars={}", o.engine.as_str(), o.model, o.ms, o.text.chars().count()));
            let _ = writeln!(stdout, "{}", o.text);
            eprintln!("promptmaxxer: done in {} ms ({}{})", o.ms, o.model, if o.retried { ", retried once" } else { "" });
            0
        }
        Err(e) => {
            crate::log::log(&format!("headless: failed engine={} level={level:?} app={app}: {e}", engine.as_str()));
            fail(format!("couldn't maxx this one, text unchanged: {e}"), &raw)
        }
    }
}
