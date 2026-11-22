//! Per-press context: the receiving AI (target), the project, its instruction files, the chip and the brief.
//! The brief never makes the person wait: a fresh cached brief is used at once; when the files changed, the
//! last saved brief is used for this press and a new one is built in the background with the fast engine
//! (pmx-context `BRIEF_PROMPT`), ready for the next press.
use crate::log::log;
use crate::settings::Settings;
use pmx_context::{Project, Source, SourcePrefs, Target, TargetKind};
use pmx_engines::{EngineId, Level};
use serde::Serialize;
use std::collections::HashSet;
use std::sync::Mutex;
use std::time::Instant;
use tokio_util::sync::CancellationToken;

#[derive(Debug, Clone)]
pub struct PressContext {
    pub target: Target,
    pub project: Option<Project>,
    pub sources: Vec<Source>,
    pub chip: String,
    pub prefs: SourcePrefs,
    /// How the project was found ("applescript-tty", "editor-title", "process-tree-unsure", ...).
    pub project_via: &'static str,
    pub ms: u128,
}

impl PressContext {
    /// "Claude Code", "ChatGPT", or the app's own name when the target is unknown.
    pub fn target_label(&self, app_name: &str) -> String {
        match self.target.hint() {
            Some(_) => self.target.label.clone(),
            None => app_name.to_string(),
        }
    }
    pub fn project_name(&self) -> Option<String> {
        self.project.as_ref().map(|p| p.name.clone())
    }
}

/// User-level files (~/.claude/CLAUDE.md, ~/.codex/AGENTS.md) are rules for coding agents: they only go into
/// prompts for coding-agent targets. About me always applies.
fn prefs_for(target: &Target, s: &Settings) -> SourcePrefs {
    let mut prefs = s.source_prefs();
    if target.kind != TargetKind::CodingAgent {
        prefs.claude_user = false;
        prefs.codex_user = false;
    }
    prefs
}

/// Front app → window title → target and project → sources → chip. Blocking (Accessibility, process table,
/// AppleScript for terminals): run it off the async threads, in parallel with the copy.
pub fn gather(app: &crate::mac::AppInfo, s: &Settings) -> PressContext {
    let t0 = Instant::now();
    let info = pmx_context::AppInfo { name: app.name.clone(), bundle_id: app.bundle_id.clone(), pid: app.pid };
    let title = pmx_context::window_title(&info);
    let target = pmx_context::target_for(&info, title.as_deref());
    let pinned = s.pinned();
    let (project, via) = pmx_context::diag::detect_project_with_source(&info, title.as_deref(), pinned.as_deref());
    let prefs = prefs_for(&target, s);
    let sources = pmx_context::sources(project.as_ref(), &prefs);
    let chip = pmx_context::chip_label(project.as_ref(), &sources);
    PressContext { target, project, sources, chip, prefs, project_via: via, ms: t0.elapsed().as_millis() }
}

/// For headless runs: the target from an app name, the project from a folder (or none).
pub fn for_app_name(app_name: &str, project_dir: Option<&std::path::Path>, s: &Settings) -> PressContext {
    let info = pmx_context::AppInfo { name: app_name.to_string(), bundle_id: String::new(), pid: 0 };
    let target = pmx_context::target_for(&info, None);
    let project = project_dir.and_then(|d| pmx_context::detect_project(&info, None, Some(d)));
    let prefs = prefs_for(&target, s);
    let sources = pmx_context::sources(project.as_ref(), &prefs);
    let chip = pmx_context::chip_label(project.as_ref(), &sources);
    PressContext { target, project, sources, chip, prefs, project_via: if project_dir.is_some() { "pinned" } else { "none" }, ms: 0 }
}

/// Projects whose brief is being rebuilt now (by root; "" = no project).
static REFRESHING: Mutex<Option<HashSet<String>>> = Mutex::new(None);

fn key(p: Option<&Project>) -> String {
    p.map(|p| p.root.to_string_lossy().into_owned()).unwrap_or_default()
}

async fn summarize(engine: EngineId, text: String) -> Option<String> {
    let t0 = Instant::now();
    match pmx_engines::complete(engine, Level::Standard, pmx_context::BRIEF_PROMPT, text, CancellationToken::new()).await {
        Ok(t) => {
            log(&format!("brief: built with {} in {} ms ({} words)", engine.as_str(), t0.elapsed().as_millis(), t.split_whitespace().count()));
            Some(t)
        }
        Err(e) => {
            log(&format!("brief: {} failed after {} ms: {e}", engine.as_str(), t0.elapsed().as_millis()));
            None
        }
    }
}

/// The brief for this press, without waiting for a model. `(text, fresh)`.
/// Stale: starts one background rebuild (with `engine`) and returns the last saved brief, if any.
pub async fn brief_now(ctx: &PressContext, engine: Option<EngineId>) -> (Option<String>, bool) {
    if ctx.sources.is_empty() && ctx.prefs.about_me.trim().is_empty() {
        return (None, true);
    }
    if let Some(b) = pmx_context::brief(ctx.project.as_ref(), &ctx.prefs, |_| async { None }).await {
        return (Some(b.text), true);
    }
    let Some(engine) = engine else { return (None, false) };
    let k = key(ctx.project.as_ref());
    let start = {
        let mut g = REFRESHING.lock().unwrap();
        g.get_or_insert_with(HashSet::new).insert(k.clone())
    };
    if start {
        let (project, prefs) = (ctx.project.clone(), ctx.prefs.clone());
        tauri::async_runtime::spawn(async move {
            log(&format!("brief: sources changed for {}, rebuilding in the background", if k.is_empty() { "user level" } else { "a project" }));
            let _ = pmx_context::brief(project.as_ref(), &prefs, move |text| summarize(engine, text)).await;
            if let Some(g) = REFRESHING.lock().unwrap().as_mut() {
                g.remove(&k);
            }
        });
    }
    // The last saved brief, but only while the project may be read at all.
    let allowed = ctx.project.is_none() || ctx.sources.iter().any(|s| s.level == pmx_context::SourceLevel::Project);
    let stale = if allowed { pmx_context::cached_brief_text(ctx.project.as_ref()) } else { None };
    (stale, false)
}

/// Build the brief now and wait for it (headless runs only).
pub async fn brief_wait(ctx: &PressContext, engine: EngineId) -> Option<String> {
    pmx_context::brief(ctx.project.as_ref(), &ctx.prefs, move |text| summarize(engine, text)).await.map(|b| b.text)
}

/// `context_preview` for the bar and settings.
#[derive(Debug, Clone, Serialize, Default)]
pub struct Preview {
    pub chip: String,
    pub brief: Option<String>,
    pub sources: Vec<PreviewSource>,
}

#[derive(Debug, Clone, Serialize)]
pub struct PreviewSource {
    pub path: String,
    pub label: String,
    pub level: String,
}

pub fn preview(ctx: &PressContext, brief: Option<String>) -> Preview {
    let home = crate::paths::home();
    Preview {
        chip: ctx.chip.clone(),
        brief,
        sources: ctx
            .sources
            .iter()
            .map(|s| PreviewSource {
                path: match s.path.strip_prefix(&home) {
                    Ok(rel) => format!("~/{}", rel.display()),
                    Err(_) => s.path.display().to_string(),
                },
                label: s.label.clone(),
                level: match s.level {
                    pmx_context::SourceLevel::User => "User".into(),
                    pmx_context::SourceLevel::Project => "Project".into(),
                },
            })
            .collect(),
    }
}
