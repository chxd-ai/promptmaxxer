//! One press, start to end: copy the selection while the context is read, pick the engine, stream the rewrite into
//! the floating bar, paste it back (or leave it on the clipboard), write history.
//! - A second press of the fast key while busy upgrades the run to max (cancel, restart at max; same draft).
//! - Esc is a global key only while busy: it cancels, and the text in the app stays as it was.
//! - Fail-safe: any error or cancel puts the old clipboard back and leaves the app's text alone.
use crate::bar::{self, Anchor, Payload};
use crate::context::{self, PressContext, Preview};
use crate::log::log;
use crate::selection::{self, CaptureError, Delivery};
use crate::{history, keys, mac, notify, settings};
use pmx_engines::{EngineError, EngineId, EngineStatus, Event, Level, Outcome, Request};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};
use tauri::AppHandle;
use tokio_util::sync::CancellationToken;

static APP: OnceLock<AppHandle> = OnceLock::new();

pub fn set_app(app: AppHandle) {
    let _ = APP.set(app);
}

pub fn app() -> Option<&'static AppHandle> {
    APP.get()
}

struct Active {
    level: Level,
    cancel: CancellationToken,
    upgrade: bool,
}

static ACTIVE: Mutex<Option<Active>> = Mutex::new(None);

pub fn is_busy() -> bool {
    ACTIVE.lock().unwrap().is_some()
}

/// Clears the busy state and turns Esc off again, on every way out of a run.
struct BusyGuard;
impl Drop for BusyGuard {
    fn drop(&mut self) {
        *ACTIVE.lock().unwrap() = None;
        keys::esc(false);
    }
}

fn begin(level: Level) -> Option<BusyGuard> {
    let mut g = ACTIVE.lock().unwrap();
    if g.is_some() {
        return None;
    }
    *g = Some(Active { level, cancel: CancellationToken::new(), upgrade: false });
    drop(g);
    keys::esc(true);
    Some(BusyGuard)
}

fn current_cancel() -> CancellationToken {
    ACTIVE.lock().unwrap().as_ref().map(|a| a.cancel.clone()).unwrap_or_default()
}

/// Esc or `bar_cancel`.
pub fn cancel() {
    if let Some(a) = ACTIVE.lock().unwrap().as_mut() {
        a.upgrade = false;
        a.cancel.cancel();
        log("press: cancelled by the person");
    }
}

/// After a cancel: true (and a fresh token, level max) when the cancel was an upgrade.
fn take_upgrade() -> bool {
    let mut g = ACTIVE.lock().unwrap();
    match g.as_mut() {
        Some(a) if a.upgrade => {
            a.upgrade = false;
            a.level = Level::Max;
            a.cancel = CancellationToken::new();
            true
        }
        _ => false,
    }
}

// ---------------------------------------------------------------------------------------------
// Engine status cache (detect_all spawns two small CLI checks; keep it off the hot path).

static STATUSES: Mutex<Option<(Instant, Vec<EngineStatus>)>> = Mutex::new(None);
const STATUS_MAX_AGE: Duration = Duration::from_secs(300);

/// Blocking: runs `detect_all`, stores it, updates the tray's engine line.
pub fn refresh_statuses() -> Vec<EngineStatus> {
    let st = pmx_engines::detect_all();
    *STATUSES.lock().unwrap() = Some((Instant::now(), st.clone()));
    let s = settings::load();
    let label = match s.pick_engine(Level::Standard, &st) {
        Some(id) => format!("Engine: {}", id.label()),
        None => "Engine: none ready".to_string(),
    };
    crate::tray::set_engine_label(&label);
    st
}

fn statuses() -> Vec<EngineStatus> {
    if let Some((t, st)) = STATUSES.lock().unwrap().as_ref() {
        if t.elapsed() < STATUS_MAX_AGE {
            return st.clone();
        }
    }
    refresh_statuses()
}

// ---------------------------------------------------------------------------------------------
// What the bar and the commands read after a press.

#[derive(Clone)]
pub struct Last {
    pub history_id: Option<String>,
    pub app: mac::AppInfo,
    pub draft: String,
    pub delivery: Delivery,
}

static LAST: Mutex<Option<Last>> = Mutex::new(None);
static PREVIEW: Mutex<Option<Preview>> = Mutex::new(None);

pub fn preview() -> Preview {
    PREVIEW.lock().unwrap().clone().unwrap_or_else(|| Preview { chip: "No project".into(), ..Default::default() })
}

pub fn last() -> Option<Last> {
    LAST.lock().unwrap().clone()
}

// ---------------------------------------------------------------------------------------------

/// A shortcut went down. `fast`: it was the fast key (a second fast press while busy upgrades to max).
pub fn on_key(level: Level) {
    {
        let mut g = ACTIVE.lock().unwrap();
        if let Some(a) = g.as_mut() {
            if a.level == Level::Standard && !a.upgrade {
                a.upgrade = true;
                a.cancel.cancel();
                log("press: second press while busy, upgrading to max");
            } else {
                log("press: ignored, already busy");
            }
            return;
        }
    }
    let Some(guard) = begin(level) else { return };
    tauri::async_runtime::spawn(async move {
        let _guard = guard;
        run_press(level).await;
    });
}

fn level_str(l: Level) -> &'static str {
    match l {
        Level::Standard => "standard",
        Level::Max => "max",
    }
}

fn payload(phase: &'static str, level: Level, engine: EngineId, target: &str, chip: &str) -> Payload {
    Payload {
        phase,
        level: level_str(level),
        engine: engine.as_str().into(),
        model: engine.model(level).into(),
        target: target.into(),
        chip: chip.into(),
        ..Default::default()
    }
}

/// Run the engine once, streaming into the bar. The text shown restarts when the engine retries.
async fn stream(app: &AppHandle, engine: EngineId, req: Request, base: &Payload) -> Result<Outcome, EngineError> {
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    let cancel = current_cancel();
    let run = pmx_engines::run(engine, req, tx, cancel);
    tokio::pin!(run);
    let mut text = String::new();
    let mut started = 0;
    let mut first_delta: Option<Instant> = None;
    let t0 = Instant::now();
    let mut handle = |ev: Event, text: &mut String| match ev {
        Event::Started { .. } => {
            started += 1;
            if started > 1 {
                text.clear();
                bar::emit(app, &Payload { phase: "delta", text: String::new(), ..base.clone() });
            }
        }
        Event::Delta(d) => {
            if first_delta.is_none() {
                first_delta = Some(Instant::now());
                log(&format!("press: first words after {} ms", t0.elapsed().as_millis()));
            }
            text.push_str(&d);
            bar::emit(app, &Payload { phase: "delta", text: text.clone(), ..base.clone() });
        }
        Event::Done(_) | Event::Failed(_) => {}
    };
    let res = loop {
        tokio::select! {
            r = &mut run => break r,
            Some(ev) = rx.recv() => handle(ev, &mut text),
        }
    };
    while let Ok(ev) = rx.try_recv() {
        handle(ev, &mut text);
    }
    res
}

async fn run_press(level0: Level) {
    let Some(app) = APP.get().cloned() else { return };
    let t0 = Instant::now();
    let s = settings::load();
    let Some(front) = mac::front_app() else {
        notify(&app, selection::MSG_NOTHING);
        return;
    };
    // Context and engine checks run while the selection is copied.
    let (f2, s2) = (front.clone(), s.clone());
    let ctx_task = tokio::task::spawn_blocking(move || {
        let bounds = mac::selection_bounds(f2.pid);
        (context::gather(&f2, &s2), bounds)
    });
    let st_task = tokio::task::spawn_blocking(statuses);

    let captured = match selection::capture().await {
        Ok(c) => c,
        Err(CaptureError::NoAccessibility) => {
            log("press: no Accessibility permission yet");
            notify(&app, selection::MSG_ACCESSIBILITY);
            return;
        }
        Err(e) => {
            log(&format!("press: {e:?}"));
            notify(&app, selection::MSG_NOTHING);
            return;
        }
    };
    let copy_ms = t0.elapsed().as_millis();
    let (ctx, bounds): (PressContext, _) = match tokio::time::timeout(Duration::from_secs(2), ctx_task).await {
        Ok(Ok(v)) => v,
        _ => {
            log("press: context took over 2 s, going on without a project");
            (context::for_app_name(&front.name, None, &s), None)
        }
    };
    let statuses = st_task.await.unwrap_or_default();
    let pick = |l: Level| s.pick_engine(l, &statuses);
    let mut level = level0;
    let Some(mut engine) = pick(level) else {
        selection::abandon(captured);
        log("press: no engine ready");
        notify(&app, selection::MSG_NO_ENGINE);
        let _ = crate::windows::open(&app, "settings");
        return;
    };
    let target_label = ctx.target_label(&front.name);
    let (brief, fresh) = context::brief_now(&ctx, pick(Level::Standard)).await;
    *PREVIEW.lock().unwrap() = Some(context::preview(&ctx, brief.clone()));
    log(&format!(
        "press: level={} app={} ({}) chars={} copy_ms={copy_ms} ctx_ms={} target={:?} project={} via={} sources={} brief={} engine={}",
        level_str(level), front.name, front.bundle_id, captured.draft.chars().count(), ctx.ms, ctx.target.hint(),
        ctx.project_name().unwrap_or_else(|| "-".into()), ctx.project_via, ctx.sources.len(),
        match (&brief, fresh) { (None, _) => "none", (Some(_), true) => "fresh", (Some(_), false) => "stale" },
        engine.as_str()
    ));

    let anchor = match bounds {
        Some((x, y, w, h)) => Anchor::Selection { x, y, w, h },
        None => {
            let (x, y) = mac::mouse_location().unwrap_or((200.0, 200.0));
            Anchor::Mouse { x, y }
        }
    };
    let mut base = payload("start", level, engine, &target_label, &ctx.chip);
    bar::emit(&app, &base);
    bar::show(&app, anchor);

    let framework = match crate::paths::framework() {
        Ok(f) => f,
        Err(e) => {
            selection::abandon(captured);
            log(&format!("press: {e}"));
            bar::emit(&app, &Payload { phase: "error", message: selection::MSG_FAILED.into(), ..base });
            bar::hide_later(&app, 4000);
            return;
        }
    };
    let result = loop {
        let req = Request {
            // Keys and passwords in the selection never leave the Mac (SPEC section 9).
            draft: pmx_context::scrub_secrets(&captured.draft),
            level,
            app_name: front.name.clone(),
            target: ctx.target.hint(),
            context_brief: brief.clone(),
            framework: framework.clone(),
        };
        match stream(&app, engine, req, &base).await {
            Err(EngineError::Cancelled) if take_upgrade() => {
                level = Level::Max;
                engine = pick(Level::Max).unwrap_or(engine);
                base = payload("start", level, engine, &target_label, &ctx.chip);
                bar::emit(&app, &base);
                log(&format!("press: restarted at max with {}", engine.as_str()));
            }
            other => break other,
        }
    };
    let total = t0.elapsed().as_millis() as u64;
    let project = ctx.project_name();
    let hint = ctx.target.hint();
    match result {
        Ok(out) => {
            let draft = captured.draft.clone();
            let app_info = captured.app.clone();
            let delivery = selection::deliver(captured, &out.text).await;
            let id = s.history.then(|| {
                history::append(&history::Row {
                    app: &front.name, level: level_str(level), engine: out.engine.as_str(), model: Some(&out.model), ms: total,
                    ok: true, retried: Some(out.retried), draft: &draft, output: Some(&out.text), error: None,
                    target: Some(&target_label), target_hint: hint.as_deref(), project: project.as_deref(),
                })
            }).flatten();
            let (phase, message) = match delivery {
                Delivery::Pasted => ("done", selection::MSG_PASTED),
                Delivery::Clipboard { .. } => ("clipboard", selection::MSG_CLIPBOARD),
            };
            bar::emit(&app, &Payload { phase, text: out.text.clone(), message: message.into(), ms: total, model: out.model.clone(), ..base });
            if s.sound {
                mac::play_done_sound();
            }
            log(&format!("press: ok engine={} model={} engine_ms={} total_ms={total} retried={} {delivery:?}", out.engine.as_str(), out.model, out.ms, out.retried));
            *LAST.lock().unwrap() = Some(Last { history_id: id, app: app_info, draft, delivery });
            bar::hide_later(&app, bar::HIDE_AFTER_MS);
        }
        Err(EngineError::Cancelled) => {
            selection::abandon(captured);
            log(&format!("press: cancelled after {total} ms, text unchanged"));
            bar::emit(&app, &Payload { phase: "cancelled", ..base });
            bar::hide_later(&app, 1200);
        }
        Err(e) => {
            let draft = captured.draft.clone();
            selection::abandon(captured);
            log(&format!("press: failed after {total} ms with {}: {e}", engine.as_str()));
            if s.history {
                history::append(&history::Row {
                    app: &front.name, level: level_str(level), engine: engine.as_str(), model: Some(&engine.model(level)), ms: total,
                    ok: false, retried: None, draft: &draft, output: None, error: Some(&e.to_string()),
                    target: Some(&target_label), target_hint: hint.as_deref(), project: project.as_deref(),
                });
            }
            bar::emit(&app, &Payload { phase: "error", message: format!("{e}. Nothing was pasted."), ..base });
            bar::hide_later(&app, 5000);
        }
    }
}

/// History "re-run": same draft, the chosen level, result to the clipboard (the original app may be gone).
/// False when a press is already running (nothing is started).
pub fn rerun(item: serde_json::Value, level: Level) -> bool {
    let Some(guard) = begin(level) else { return false };
    tauri::async_runtime::spawn(async move {
        let _guard = guard;
        let Some(app) = APP.get().cloned() else { return };
        let s = settings::load();
        let str_of = |k: &str| item.get(k).and_then(|v| v.as_str()).unwrap_or("").to_string();
        let draft = str_of("draft");
        if draft.trim().is_empty() {
            return;
        }
        let statuses = tokio::task::spawn_blocking(statuses).await.unwrap_or_default();
        let Some(engine) = s.pick_engine(level, &statuses) else {
            notify(&app, selection::MSG_NO_ENGINE);
            return;
        };
        let app_name = str_of("app");
        let target = Some(str_of("target_hint")).filter(|t| !t.is_empty());
        let label = Some(str_of("target")).filter(|t| !t.is_empty()).unwrap_or_else(|| app_name.clone());
        let base = payload("start", level, engine, &label, "History");
        let (x, y) = mac::mouse_location().unwrap_or((200.0, 200.0));
        bar::emit(&app, &base);
        bar::show(&app, Anchor::Mouse { x, y });
        let framework = match crate::paths::framework() {
            Ok(f) => f,
            Err(e) => {
                log(&format!("rerun: {e}"));
                bar::emit(&app, &Payload { phase: "error", message: selection::MSG_FAILED.into(), ..base });
                bar::hide_later(&app, 4000);
                return;
            }
        };
        let t0 = Instant::now();
        let req = Request { draft: pmx_context::scrub_secrets(&draft), level, app_name: app_name.clone(), target: target.clone(), context_brief: None, framework };
        match stream(&app, engine, req, &base).await {
            Ok(out) => {
                mac::set_clipboard_text(&out.text);
                let ms = t0.elapsed().as_millis() as u64;
                if s.history {
                    let project = str_of("project");
                    let id = history::append(&history::Row {
                        app: &app_name, level: level_str(level), engine: out.engine.as_str(), model: Some(&out.model), ms, ok: true,
                        retried: Some(out.retried), draft: &draft, output: Some(&out.text), error: None, target: Some(&label),
                        target_hint: target.as_deref(), project: Some(project.as_str()).filter(|p| !p.is_empty()),
                    });
                    // Bar thumbs rate the last row: point them at this re-run, not the press before it.
                    if let Some(front) = mac::front_app() {
                        *LAST.lock().unwrap() =
                            Some(Last { history_id: id, app: front, draft: draft.clone(), delivery: Delivery::Clipboard { reason: "re-run" } });
                    }
                }
                log(&format!("rerun: ok engine={} level={} ms={ms}", out.engine.as_str(), level_str(level)));
                bar::emit(&app, &Payload { phase: "clipboard", text: out.text, message: selection::MSG_CLIPBOARD.into(), ms, ..base });
                bar::hide_later(&app, bar::HIDE_AFTER_MS);
            }
            Err(e) => {
                log(&format!("rerun: failed: {e}"));
                let phase = if e == EngineError::Cancelled { "cancelled" } else { "error" };
                bar::emit(&app, &Payload { phase, message: e.to_string(), ..base });
                bar::hide_later(&app, 4000);
            }
        }
    });
    true
}

/// `bar_undo`: the paste was one edit, so Cmd+Z in the same app puts the original back. Done only when that app
/// is in front (or our own bar took the focus); a clipboard result puts the original text on the clipboard.
pub async fn undo() -> bool {
    let Some(last) = last() else { return false };
    match last.delivery {
        Delivery::Pasted => {
            let me = std::process::id() as i32;
            let front = mac::front_app().map(|a| a.pid);
            if front == Some(me) {
                mac::activate(last.app.pid);
                tokio::time::sleep(Duration::from_millis(250)).await;
            } else if front != Some(last.app.pid) {
                log("undo: another app is in front, nothing done");
                return false;
            }
            let ok = mac::cmd_key(mac::KEY_Z).is_ok();
            log(&format!("undo: Cmd+Z sent to {}", last.app.name));
            ok
        }
        Delivery::Clipboard { .. } => mac::set_clipboard_text(&last.draft),
    }
}
