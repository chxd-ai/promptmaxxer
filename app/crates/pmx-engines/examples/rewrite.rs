//! Rewrite a draft from stdin with one engine and print timings.
//!
//!   cargo run --example rewrite -- --engine claude-plan --level standard --app Ghostty < draft.txt
//!   cargo run --example rewrite -- --detect
//!
//! Flags: --engine <id|auto>  --level standard|max  --app NAME  --target TEXT  --brief TEXT
//!        --framework FILE (default: framework/system-prompt.md of this repo)  --quiet (no live deltas)
//! The rewritten prompt goes to stdout; live deltas and timings go to stderr.

use pmx_engines::{detect_all, pick, run, EngineId, Event, Level, Request};
use std::io::{Read, Write};
use std::time::Instant;
use tokio_util::sync::CancellationToken;

#[tokio::main]
async fn main() {
    let mut args = std::env::args().skip(1);
    let mut engine = String::from("auto");
    let mut level = Level::Standard;
    let mut app = String::from("unknown");
    let mut target = None;
    let mut brief = None;
    let mut framework = format!("{}/../../../framework/system-prompt.md", env!("CARGO_MANIFEST_DIR"));
    let mut detect = false;
    let mut quiet = false;
    while let Some(a) = args.next() {
        match a.as_str() {
            "--engine" => engine = args.next().expect("--engine needs a value"),
            "--level" => {
                level = match args.next().as_deref() {
                    Some("max") => Level::Max,
                    _ => Level::Standard,
                }
            }
            "--app" => app = args.next().expect("--app needs a value"),
            "--target" => target = args.next(),
            "--brief" => brief = args.next(),
            "--framework" => framework = args.next().expect("--framework needs a value"),
            "--detect" => detect = true,
            "--quiet" => quiet = true,
            other => {
                eprintln!("unknown flag {other}");
                std::process::exit(2);
            }
        }
    }

    let t = Instant::now();
    let statuses = detect_all();
    let detect_ms = t.elapsed().as_millis();
    for s in &statuses {
        eprintln!("detect {:<14} ready={:<5} {} ({})", s.id.as_str(), s.ready, s.detail, s.label);
    }
    eprintln!("detect_all took {detect_ms} ms");
    if detect {
        return;
    }

    let id = if engine == "auto" {
        pick(None, &statuses).unwrap_or_else(|| {
            eprintln!("no engine is ready");
            std::process::exit(1)
        })
    } else {
        engine.parse::<EngineId>().unwrap_or_else(|e| {
            eprintln!("{e}");
            std::process::exit(2)
        })
    };

    let mut draft = String::new();
    std::io::stdin().read_to_string(&mut draft).unwrap();
    let framework = std::fs::read_to_string(&framework).unwrap_or_else(|e| {
        eprintln!("cannot read framework {framework}: {e}");
        std::process::exit(2)
    });
    let req = Request { draft, level, app_name: app, target, context_brief: brief, framework };

    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let cancel = CancellationToken::new();
    let c2 = cancel.clone();
    tokio::spawn(async move {
        if tokio::signal::ctrl_c().await.is_ok() {
            c2.cancel();
        }
    });
    let t0 = Instant::now();
    let task = tokio::spawn(run(id, req, tx, cancel));
    let mut first_token = None;
    while let Some(ev) = rx.recv().await {
        match ev {
            Event::Started { engine, model } => eprintln!("[started engine={engine} model={model} at {} ms]", t0.elapsed().as_millis()),
            Event::Delta(d) => {
                first_token.get_or_insert(t0.elapsed().as_millis());
                if !quiet {
                    eprint!("{d}");
                    let _ = std::io::stderr().flush();
                }
            }
            Event::Done(_) | Event::Failed(_) => {}
        }
    }
    let total = t0.elapsed().as_millis();
    match task.await.unwrap() {
        Ok(o) => {
            if !quiet {
                eprintln!();
            }
            println!("{}", o.text);
            eprintln!(
                "RESULT engine={} model={} first_token_ms={} total_ms={} outcome_ms={} retried={} chars={}",
                o.engine,
                o.model,
                first_token.map(|v| v.to_string()).unwrap_or("-".into()),
                total,
                o.ms,
                o.retried,
                o.text.chars().count()
            );
        }
        Err(e) => {
            eprintln!("\nFAILED engine={id} after {total} ms: {e}");
            std::process::exit(1);
        }
    }
}
