//! pmx-engines: turn a rough prompt into a strong one with whichever AI the person already has.
//!
//! Engines, in default pick order: `custom-api` (any OpenAI-compatible API, cloud or local),
//! `anthropic-key` (API key in the keychain), `claude-plan` (Claude Code CLI), `chatgpt-plan` (Codex CLI),
//! `gemini-plan` (Gemini CLI). The model each engine uses can be changed: see [`models`]. [`detect_all`] says which are ready,
//! [`pick`] chooses one, [`run`] streams the rewrite as [`Event`]s and returns the [`Outcome`].
//!
//! Event order for one [`run`]: `Started`, `Delta`*, then `Done` or `Failed`. If the first reply is a
//! comment or refusal, the engine retries once with a firmer reminder: a second `Started` is sent, and the
//! caller drops the text streamed so far. `Done.text` is the cleaned final text; deltas are raw.

pub mod guard;
pub mod keychain;
pub mod models;
mod paths;
mod proc;
mod prompt;
mod engines {
    pub(crate) mod anthropic;
    pub(crate) mod claude;
    pub(crate) mod codex;
    pub(crate) mod gemini;
    pub(crate) mod openai;
}

pub use models::{configure, CustomApi, LevelModels, ModelConfig};
pub use prompt::user_message;

use engines::{anthropic, claude, codex, gemini, openai};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::process::{Command as StdCommand, Stdio};
use std::time::{Duration, Instant};
use tokio::sync::mpsc::UnboundedSender;
use tokio_util::sync::CancellationToken;

/// Fast key = Standard.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Level {
    Standard,
    Max,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum EngineId {
    #[serde(rename = "custom-api")]
    CustomApi,
    #[serde(rename = "anthropic-key")]
    AnthropicKey,
    #[serde(rename = "claude-plan")]
    ClaudePlan,
    #[serde(rename = "chatgpt-plan")]
    ChatGptPlan,
    #[serde(rename = "gemini-plan")]
    GeminiPlan,
}

impl EngineId {
    /// Default pick order. `custom-api` is ready only once someone set it up, so it comes first.
    pub const ALL: [EngineId; 5] = [EngineId::CustomApi, EngineId::AnthropicKey, EngineId::ClaudePlan, EngineId::ChatGptPlan, EngineId::GeminiPlan];

    pub fn as_str(self) -> &'static str {
        match self {
            EngineId::CustomApi => "custom-api",
            EngineId::AnthropicKey => "anthropic-key",
            EngineId::ClaudePlan => "claude-plan",
            EngineId::ChatGptPlan => "chatgpt-plan",
            EngineId::GeminiPlan => "gemini-plan",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            EngineId::CustomApi => "Any model (OpenAI-compatible API)",
            EngineId::AnthropicKey => "Anthropic API key",
            EngineId::ClaudePlan => "Claude plan",
            EngineId::ChatGptPlan => "ChatGPT plan",
            EngineId::GeminiPlan => "Google account",
        }
    }

    /// The model this engine uses for this level: the person's choice (settings), else the default.
    pub fn model(self, level: Level) -> String {
        self.model_in(level, &models::current())
    }

    pub fn model_in(self, level: Level, cfg: &ModelConfig) -> String {
        let pick = |s: &str| Some(s.trim().to_string()).filter(|s| !s.is_empty());
        if self == EngineId::CustomApi {
            let fast = cfg.custom.fast_model.trim().to_string();
            return match level {
                Level::Standard => fast,
                Level::Max => pick(&cfg.custom.max_model).unwrap_or(fast),
            };
        }
        let chosen = cfg.models.get(self.as_str()).and_then(|m| match level {
            Level::Standard => pick(&m.fast),
            Level::Max => pick(&m.max),
        });
        chosen.unwrap_or_else(|| self.default_model(level).to_string())
    }

    /// The built-in model name per level. CLI engines take their CLI's alias or model id.
    /// `custom-api` has no default: the person names the model.
    pub fn default_model(self, level: Level) -> &'static str {
        match (self, level) {
            (EngineId::CustomApi, _) => "",
            (EngineId::AnthropicKey, Level::Standard) => "claude-haiku-5-5",
            (EngineId::AnthropicKey, Level::Max) => "claude-sonnet-5-5",
            (EngineId::ClaudePlan, Level::Standard) => "haiku",
            (EngineId::ClaudePlan, Level::Max) => "sonnet",
            (EngineId::ChatGptPlan, Level::Standard) => "gpt-6.2-sol",
            (EngineId::ChatGptPlan, Level::Max) => "gpt-6.2-astra",
            (EngineId::GeminiPlan, Level::Standard) => "flash",
            (EngineId::GeminiPlan, Level::Max) => "pro",
        }
    }
}

impl std::fmt::Display for EngineId {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl std::str::FromStr for EngineId {
    type Err = EngineError;
    fn from_str(s: &str) -> Result<Self, EngineError> {
        EngineId::ALL
            .into_iter()
            .find(|e| e.as_str() == s.trim())
            .ok_or_else(|| EngineError::NotReady(format!("unknown engine '{s}'")))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EngineStatus {
    pub id: EngineId,
    /// "Claude plan"
    pub label: String,
    pub ready: bool,
    /// "Signed in", "CLI not found", "Not signed in"
    pub detail: String,
    /// The models in use now, and the built-in ones (empty for `custom-api`).
    pub fast_model: String,
    pub max_model: String,
    pub default_fast: String,
    pub default_max: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Request {
    pub draft: String,
    pub level: Level,
    pub app_name: String,
    /// "coding agent: Claude Code"
    pub target: Option<String>,
    pub context_brief: Option<String>,
    /// System prompt text (framework/system-prompt.md).
    pub framework: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Event {
    Started { engine: EngineId, model: String },
    Delta(String),
    Done(Outcome),
    Failed(String),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Outcome {
    pub text: String,
    pub engine: EngineId,
    pub model: String,
    pub ms: u64,
    pub retried: bool,
}

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum EngineError {
    #[error("{0}")]
    NotReady(String),
    #[error("Cancelled")]
    Cancelled,
    #[error("No reply after {0} seconds")]
    Timeout(u64),
    #[error("{0}")]
    Cli(String),
    #[error("{0}")]
    Http(String),
    #[error("{0}")]
    BadOutput(String),
    #[error("Nothing to rewrite")]
    EmptyDraft,
    #[error("Selection too long ({0} characters)")]
    TooLong(usize),
    #[error("Keychain: {0}")]
    Keychain(String),
}

/// Timeout for one model call (the retry gets its own).
pub const CALL_TIMEOUT: Duration = Duration::from_secs(60);

// ---------------------------------------------------------------------------------------------
// Detection

const STATUS_TIMEOUT: Duration = Duration::from_millis(1200);

/// Run a status command with a hard timeout; None on timeout or start failure.
fn quick_output(mut cmd: StdCommand) -> Option<(bool, String)> {
    use std::io::Read;
    paths::fill_env(&mut cmd);
    let mut child = cmd.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().ok()?;
    let t0 = Instant::now();
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                let mut s = String::new();
                if let Some(mut o) = child.stdout.take() {
                    let _ = o.read_to_string(&mut s);
                }
                if let Some(mut e) = child.stderr.take() {
                    let _ = e.read_to_string(&mut s);
                }
                return Some((status.success(), s));
            }
            Ok(None) if t0.elapsed() < STATUS_TIMEOUT => std::thread::sleep(Duration::from_millis(15)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }
}

fn status(id: EngineId, ready: bool, detail: impl Into<String>) -> EngineStatus {
    EngineStatus {
        id,
        label: id.label().to_string(),
        ready,
        detail: detail.into(),
        fast_model: id.model(Level::Standard),
        max_model: id.model(Level::Max),
        default_fast: id.default_model(Level::Standard).into(),
        default_max: id.default_model(Level::Max).into(),
    }
}

/// Keychain account for the `custom-api` key (optional: local servers need none).
pub const CUSTOM_KEY: &str = "custom-api";

fn detect_custom() -> EngineStatus {
    let c = models::current().custom;
    if !c.is_set() {
        return status(EngineId::CustomApi, false, "Not set up");
    }
    let key = if keychain::get(CUSTOM_KEY).is_some() { ", key saved" } else { "" };
    status(EngineId::CustomApi, true, format!("{}{key}", c.host()))
}

fn detect_anthropic() -> EngineStatus {
    match keychain::get("anthropic") {
        Some(_) => status(EngineId::AnthropicKey, true, "Key saved"),
        None => status(EngineId::AnthropicKey, false, "No key saved"),
    }
}

fn detect_claude() -> EngineStatus {
    let id = EngineId::ClaudePlan;
    let Some(cli) = paths::find_cli("claude") else { return status(id, false, "CLI not found") };
    let mut c = StdCommand::new(&cli);
    c.args(["auth", "status", "--json"]).current_dir("/tmp").env("PATH", paths::child_path(&cli));
    match quick_output(c).and_then(|(_, out)| claude::parse_auth_status(&out)) {
        Some(true) => status(id, true, "Signed in"),
        Some(false) => status(id, false, "Not signed in"),
        None => status(id, true, "Signed in: not checked"),
    }
}

fn detect_codex() -> EngineStatus {
    let id = EngineId::ChatGptPlan;
    let Some(launcher) = paths::find_cli("codex") else { return status(id, false, "CLI not found") };
    let Some(cli) = paths::codex_native(&launcher) else { return status(id, false, "CLI found, but its program file is missing") };
    let mut c = StdCommand::new(&cli);
    c.args(["login", "status"]).current_dir("/tmp").env("PATH", paths::child_path(&cli));
    match quick_output(c) {
        Some((_, out)) => {
            let (ready, detail) = codex::parse_login_status(&out);
            status(id, ready, detail)
        }
        None => status(id, true, "Signed in: not checked"),
    }
}

fn detect_gemini() -> EngineStatus {
    let id = EngineId::GeminiPlan;
    if paths::find_cli("gemini").is_none() {
        return status(id, false, "CLI not found");
    }
    let (ready, detail) = gemini::signed_in(&gemini::real_dir());
    status(id, ready, detail)
}

/// Which engines are ready. No model calls; the checks run in parallel (about 0.2 s here).
pub fn detect_all() -> Vec<EngineStatus> {
    std::thread::scope(|s| {
        let hs = [
            s.spawn(detect_custom),
            s.spawn(detect_anthropic),
            s.spawn(detect_claude),
            s.spawn(detect_codex),
            s.spawn(detect_gemini),
        ];
        hs.into_iter()
            .zip(EngineId::ALL)
            .map(|(h, id)| h.join().unwrap_or_else(|_| status(id, false, "Check failed")))
            .collect()
    })
}

/// A pinned engine is used only when it is ready (None otherwise: never bill another account silently).
/// Auto (`None`) takes the first ready engine in the default order.
pub fn pick(pref: Option<EngineId>, statuses: &[EngineStatus]) -> Option<EngineId> {
    let ready = |id: EngineId| statuses.iter().any(|s| s.id == id && s.ready);
    match pref {
        Some(id) => ready(id).then_some(id),
        None => EngineId::ALL.into_iter().find(|&id| ready(id)),
    }
}

// ---------------------------------------------------------------------------------------------
// Run

/// Test and integration hooks. `Default` is what [`run`] uses.
#[doc(hidden)]
#[derive(Debug, Clone, Default)]
pub struct RunOptions {
    pub anthropic_base_url: Option<String>,
    /// Use this key instead of the keychain one.
    pub anthropic_key: Option<String>,
    /// Use this program instead of the CLI found on disk.
    pub cli: Option<PathBuf>,
    /// `custom-api`: use this setup and key instead of the saved ones.
    pub custom: Option<CustomApi>,
    pub custom_key: Option<String>,
    pub timeout: Option<Duration>,
}

/// Stream one rewrite. Sends `Started`, `Delta`s, then `Done` or `Failed`; returns the same result.
pub async fn run(
    engine: EngineId,
    req: Request,
    tx: UnboundedSender<Event>,
    cancel: CancellationToken,
) -> Result<Outcome, EngineError> {
    run_with(engine, req, tx, cancel, RunOptions::default()).await
}

#[doc(hidden)]
pub async fn run_with(
    engine: EngineId,
    req: Request,
    tx: UnboundedSender<Event>,
    cancel: CancellationToken,
    opts: RunOptions,
) -> Result<Outcome, EngineError> {
    let t0 = Instant::now();
    let model = match (&opts.custom, engine) {
        (Some(c), EngineId::CustomApi) => EngineId::CustomApi.model_in(req.level, &ModelConfig { custom: c.clone(), ..Default::default() }),
        _ => engine.model(req.level),
    };
    let res = run_inner(engine, &req, &model, &tx, &cancel, &opts, t0).await;
    match &res {
        Ok(o) => {
            let _ = tx.send(Event::Done(o.clone()));
        }
        Err(e) => {
            let _ = tx.send(Event::Failed(e.to_string()));
        }
    }
    res
}

async fn run_inner(
    engine: EngineId,
    req: &Request,
    model: &str,
    tx: &UnboundedSender<Event>,
    cancel: &CancellationToken,
    opts: &RunOptions,
    t0: Instant,
) -> Result<Outcome, EngineError> {
    let draft = req.draft.trim();
    if draft.is_empty() {
        return Err(EngineError::EmptyDraft);
    }
    let n = draft.chars().count();
    if n > prompt::MAX_CHARS {
        return Err(EngineError::TooLong(n));
    }
    let _ = tx.send(Event::Started { engine, model: model.to_string() });
    let first = guard::clean(&attempt(engine, req, model, false, tx, cancel, opts).await?);
    let (text, retried) = if guard::looks_bad(&first, draft) {
        let _ = tx.send(Event::Started { engine, model: model.to_string() });
        let second = guard::clean(&attempt(engine, req, model, true, tx, cancel, opts).await?);
        if guard::looks_bad(&second, draft) {
            return Err(EngineError::BadOutput("The AI returned a comment instead of a prompt".into()));
        }
        (second, true)
    } else {
        (first, false)
    };
    Ok(Outcome { text, engine, model: model.to_string(), ms: t0.elapsed().as_millis() as u64, retried })
}

fn not_found(name: &str) -> EngineError {
    EngineError::NotReady(format!("{name} CLI not found"))
}

async fn attempt(
    engine: EngineId,
    req: &Request,
    model: &str,
    firm: bool,
    tx: &UnboundedSender<Event>,
    cancel: &CancellationToken,
    opts: &RunOptions,
) -> Result<String, EngineError> {
    let user = prompt::user_message(req, firm);
    call_raw(engine, req.level, model, &req.framework, user, tx, cancel, opts).await
}

/// One model call with this system prompt and user message, no wrapping and no guard.
#[allow(clippy::too_many_arguments)]
async fn call_raw(
    engine: EngineId,
    level: Level,
    model: &str,
    system: &str,
    user: String,
    tx: &UnboundedSender<Event>,
    cancel: &CancellationToken,
    opts: &RunOptions,
) -> Result<String, EngineError> {
    let timeout = opts.timeout.unwrap_or(CALL_TIMEOUT);
    if cancel.is_cancelled() {
        return Err(EngineError::Cancelled);
    }
    let fw_file = || paths::framework_file(system).map_err(|e| EngineError::Cli(format!("could not write the framework file: {e}")));
    match engine {
        EngineId::CustomApi => {
            let cfg = opts.custom.clone().unwrap_or_else(|| models::current().custom);
            if !cfg.is_set() {
                return Err(EngineError::NotReady("Any model: set the API address and model name in Settings".into()));
            }
            let key = opts.custom_key.clone().or_else(|| keychain::get(CUSTOM_KEY));
            openai::call(&cfg.endpoint(), key.as_deref(), model, system, &user, tx, cancel, timeout).await
        }
        EngineId::AnthropicKey => {
            let key = opts
                .anthropic_key
                .clone()
                .or_else(|| keychain::get("anthropic"))
                .ok_or_else(|| EngineError::NotReady("No Anthropic API key saved".into()))?;
            let base = opts.anthropic_base_url.as_deref().unwrap_or(anthropic::DEFAULT_BASE_URL);
            anthropic::call(base, &key, model, level, system, &user, tx, cancel, timeout).await
        }
        EngineId::ClaudePlan => {
            let cli = opts.cli.clone().or_else(|| paths::find_cli("claude")).ok_or_else(|| not_found("Claude Code"))?;
            let cmd = claude::command(&cli, model, &fw_file()?);
            proc::run_cli(cmd, Some(user), &mut claude::Parser::default(), tx, cancel, timeout).await
        }
        EngineId::ChatGptPlan => {
            let cli = match opts.cli.clone() {
                Some(c) => c,
                None => paths::find_cli("codex").and_then(|l| paths::codex_native(&l)).ok_or_else(|| not_found("Codex"))?,
            };
            let home = codex::scratch_home();
            codex::prepare_home(&codex::real_home(), &home).map_err(|e| EngineError::Cli(format!("could not set up the Codex folder: {e}")))?;
            let cmd = codex::command(&cli, model, &fw_file()?, &home);
            proc::run_cli(cmd, Some(user), &mut codex::Parser::default(), tx, cancel, timeout).await
        }
        EngineId::GeminiPlan => {
            let cli = opts.cli.clone().or_else(|| paths::find_cli("gemini")).ok_or_else(|| not_found("Gemini"))?;
            let home = gemini::scratch_home();
            gemini::prepare_home(&gemini::real_dir(), &home).map_err(|e| EngineError::Cli(format!("could not set up the Gemini folder: {e}")))?;
            let cmd = gemini::command(&cli, model, &fw_file()?, &home, &user);
            proc::run_cli(cmd, None, &mut gemini::Parser::default(), tx, cancel, timeout).await
        }
    }
}

/// One plain model call: `system` as the system prompt, `user` as the message, the text back
/// (trimmed). No draft wrapper, no guard, no retry, no events. Used for the project brief
/// (pmx-context `BRIEF_PROMPT`).
pub async fn complete(engine: EngineId, level: Level, system: &str, user: String, cancel: CancellationToken) -> Result<String, EngineError> {
    complete_with(engine, level, system, user, cancel, RunOptions::default()).await
}

#[doc(hidden)]
pub async fn complete_with(
    engine: EngineId,
    level: Level,
    system: &str,
    user: String,
    cancel: CancellationToken,
    opts: RunOptions,
) -> Result<String, EngineError> {
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let model = engine.model(level);
    let out = call_raw(engine, level, &model, system, user, &tx, &cancel, &opts).await?;
    let out = out.trim().to_string();
    if out.is_empty() {
        return Err(EngineError::BadOutput("The AI returned nothing".into()));
    }
    Ok(out)
}

/// One tiny real call for the settings "Test" button.
pub async fn test_engine(engine: EngineId, framework: &str) -> Result<Outcome, EngineError> {
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let req = Request {
        draft: "fix the typo in the readme".into(),
        level: Level::Standard,
        app_name: "Promptmaxxer settings".into(),
        target: None,
        context_brief: None,
        framework: framework.to_string(),
    };
    run(engine, req, tx, CancellationToken::new()).await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn st(id: EngineId, ready: bool) -> EngineStatus {
        status(id, ready, "")
    }

    #[test]
    fn serde_names() {
        assert_eq!(serde_json::to_string(&EngineId::ChatGptPlan).unwrap(), "\"chatgpt-plan\"");
        assert_eq!(serde_json::from_str::<EngineId>("\"anthropic-key\"").unwrap(), EngineId::AnthropicKey);
        assert_eq!(serde_json::to_string(&Level::Standard).unwrap(), "\"standard\"");
        for id in EngineId::ALL {
            assert_eq!(id.as_str().parse::<EngineId>().unwrap(), id);
            assert_eq!(serde_json::to_string(&id).unwrap(), format!("\"{id}\""));
        }
        assert!("nope".parse::<EngineId>().is_err());
    }

    #[test]
    fn pick_auto_and_pinned() {
        let s = [st(EngineId::CustomApi, false), st(EngineId::AnthropicKey, false), st(EngineId::ClaudePlan, true), st(EngineId::ChatGptPlan, true), st(EngineId::GeminiPlan, false)];
        assert_eq!(pick(None, &s), Some(EngineId::ClaudePlan));
        assert_eq!(pick(Some(EngineId::ChatGptPlan), &s), Some(EngineId::ChatGptPlan));
        assert_eq!(pick(Some(EngineId::GeminiPlan), &s), None);
        let none = [st(EngineId::ClaudePlan, false)];
        assert_eq!(pick(None, &none), None);
        // Order comes from the default order, not the slice order.
        let rev = [st(EngineId::GeminiPlan, true), st(EngineId::AnthropicKey, true)];
        assert_eq!(pick(None, &rev), Some(EngineId::AnthropicKey));
    }

    #[test]
    fn models_default_and_override() {
        let mut cfg = ModelConfig::default();
        assert_eq!(EngineId::ClaudePlan.model_in(Level::Standard, &cfg), "haiku");
        assert_eq!(EngineId::CustomApi.model_in(Level::Max, &cfg), "");
        cfg.models.insert("claude-plan".into(), LevelModels { fast: " ".into(), max: "opus".into() });
        assert_eq!(EngineId::ClaudePlan.model_in(Level::Standard, &cfg), "haiku");
        assert_eq!(EngineId::ClaudePlan.model_in(Level::Max, &cfg), "opus");
        cfg.custom = CustomApi { base_url: "http://localhost:11434/v1".into(), fast_model: "qwen3:8b".into(), max_model: String::new() };
        assert_eq!(EngineId::CustomApi.model_in(Level::Standard, &cfg), "qwen3:8b");
        assert_eq!(EngineId::CustomApi.model_in(Level::Max, &cfg), "qwen3:8b");
        cfg.custom.max_model = "qwen3:32b".into();
        assert_eq!(EngineId::CustomApi.model_in(Level::Max, &cfg), "qwen3:32b");
    }

    #[test]
    fn quick_output_times_out() {
        let mut c = StdCommand::new("/bin/sleep");
        c.arg("5");
        let t = Instant::now();
        assert!(quick_output(c).is_none());
        assert!(t.elapsed() < Duration::from_secs(2));
    }
}
