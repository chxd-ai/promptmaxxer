//! Settings: `~/.local/share/promptmaxxer/settings.json`, file mode 600 (Windows: `%APPDATA%\promptmaxxer\settings.json`).
//! `PMX_SETTINGS=/path/settings.json` points the app at another file (tests).
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Default keys. Max is Ctrl+Shift+Space, not Ctrl+Option+Space: Control+Option is a common
/// push-to-talk chord in voice tools, and Ctrl+Option+Space is macOS's own "next input source" key.
/// Ctrl+Shift+Space is free by default on macOS.
#[cfg(not(windows))]
pub const DEFAULT_FAST_KEY: &str = "Alt+Space";
#[cfg(not(windows))]
pub const DEFAULT_MAX_KEY: &str = "Ctrl+Shift+Space";

/// Windows default keys. Alt+Space is taken on Windows: it opens every window's system menu (and PowerToys Run
/// uses it). Ctrl+Space switches the input language with Chinese, Japanese and Korean keyboards, and Win+Space
/// switches keyboard layouts. Ctrl+Alt+Space is free on a standard Windows install; Ctrl+Shift+Space stays the
/// max key, as on macOS.
#[cfg(windows)]
pub const DEFAULT_FAST_KEY: &str = "Ctrl+Alt+Space";
#[cfg(windows)]
pub const DEFAULT_MAX_KEY: &str = "Ctrl+Shift+Space";

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Sources {
    pub claude_user: bool,
    pub codex_user: bool,
    pub project_files: bool,
}

impl Default for Sources {
    fn default() -> Self {
        Sources { claude_user: true, codex_user: true, project_files: true }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub fast_key: String,
    pub max_key: String,
    /// "auto" or an engine id ("claude-plan", ...).
    pub engine: String,
    pub engine_fast: Option<String>,
    pub engine_max: Option<String>,
    /// Model names per engine id that replace the built-in ones. Empty = built-in.
    pub models: std::collections::BTreeMap<String, pmx_engines::LevelModels>,
    /// The "any model" engine: an OpenAI-compatible API address and model names.
    pub custom_api: pmx_engines::CustomApi,
    pub about_me: String,
    pub sources: Sources,
    pub blocked_projects: Vec<String>,
    pub pinned_project: Option<String>,
    pub history: bool,
    pub start_at_login: bool,
    pub sound: bool,
    pub onboarded: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            fast_key: DEFAULT_FAST_KEY.into(),
            max_key: DEFAULT_MAX_KEY.into(),
            engine: "auto".into(),
            engine_fast: None,
            engine_max: None,
            models: Default::default(),
            custom_api: Default::default(),
            about_me: String::new(),
            sources: Sources::default(),
            blocked_projects: vec![],
            pinned_project: None,
            history: true,
            start_at_login: false,
            sound: true,
            onboarded: false,
        }
    }
}

fn expand(p: &str) -> PathBuf {
    match p.strip_prefix("~/") {
        Some(rest) => crate::paths::home().join(rest),
        None => PathBuf::from(p),
    }
}

impl Settings {
    /// The engine preference for a level: the per-key pin, else the overall one. `None` = auto.
    pub fn engine_pref(&self, level: pmx_engines::Level) -> Option<pmx_engines::EngineId> {
        let per_key = match level {
            pmx_engines::Level::Standard => self.engine_fast.as_deref(),
            pmx_engines::Level::Max => self.engine_max.as_deref(),
        };
        let s = per_key.filter(|s| !s.trim().is_empty()).unwrap_or(self.engine.as_str());
        if s.trim().is_empty() || s == "auto" {
            return None;
        }
        s.parse().ok()
    }

    pub fn model_config(&self) -> pmx_engines::ModelConfig {
        pmx_engines::ModelConfig { models: self.models.clone(), custom: self.custom_api.clone() }
    }

    pub fn source_prefs(&self) -> pmx_context::SourcePrefs {
        pmx_context::SourcePrefs {
            about_me: self.about_me.clone(),
            claude_user: self.sources.claude_user,
            codex_user: self.sources.codex_user,
            project_files: self.sources.project_files,
            blocked_projects: self.blocked_projects.iter().map(|p| expand(p)).collect(),
        }
    }

    pub fn pinned(&self) -> Option<PathBuf> {
        self.pinned_project.as_deref().filter(|p| !p.trim().is_empty()).map(expand)
    }
}

pub fn path() -> PathBuf {
    match std::env::var_os("PMX_SETTINGS") {
        Some(p) if !p.is_empty() => PathBuf::from(p),
        _ => crate::paths::share_dir().join("settings.json"),
    }
}

/// Missing or unreadable file: defaults (an unreadable file is left alone, not overwritten until a save).
/// Also hands the model choice to the engines, so every reader of the settings uses the same models.
pub fn load() -> Settings {
    let s = load_from(&path());
    pmx_engines::configure(s.model_config());
    s
}

pub fn load_from(p: &Path) -> Settings {
    match std::fs::read(p) {
        Ok(raw) => serde_json::from_slice(&raw).unwrap_or_else(|e| {
            crate::log::log(&format!("settings: could not read {} ({e}); using defaults", p.display()));
            Settings::default()
        }),
        Err(_) => Settings::default(),
    }
}

pub fn save(s: &Settings) -> std::io::Result<()> {
    save_to(&path(), s)?;
    pmx_engines::configure(s.model_config());
    Ok(())
}

pub fn save_to(p: &Path, s: &Settings) -> std::io::Result<()> {
    if let Some(dir) = p.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = p.with_extension(format!("json.tmp{}", std::process::id()));
    {
        use std::io::Write;
        let mut f = crate::paths::private_file(&tmp, true)?;
        f.write_all(&serde_json::to_vec_pretty(s).map_err(std::io::Error::other)?)?;
    }
    crate::paths::set_private(&tmp)?;
    std::fs::rename(&tmp, p)
}

#[cfg(test)]
mod tests {
    use super::*;
    use pmx_engines::{EngineId, Level};

    #[test]
    fn defaults_match_the_contract_and_partial_files_fill_in() {
        let d = Settings::default();
        let v = serde_json::to_value(&d).unwrap();
        for k in ["fast_key", "max_key", "engine", "engine_fast", "engine_max", "models", "custom_api", "about_me", "sources", "blocked_projects",
            "pinned_project", "history", "start_at_login", "sound", "onboarded"] {
            assert!(v.get(k).is_some(), "missing {k}");
        }
        assert_eq!(d.engine, "auto");
        assert!(!d.start_at_login && !d.onboarded && d.history);
        let s: Settings = serde_json::from_str(r#"{"fast_key":"Ctrl+Alt+Cmd+P","onboarded":true}"#).unwrap();
        assert_eq!(s.fast_key, "Ctrl+Alt+Cmd+P");
        assert_eq!(s.max_key, DEFAULT_MAX_KEY);
        assert!(s.onboarded && s.sources.claude_user);
    }

    #[test]
    fn engine_pref_per_key_wins_over_overall() {
        let mut s = Settings::default();
        assert_eq!(s.engine_pref(Level::Standard), None);
        s.engine = "chatgpt-plan".into();
        assert_eq!(s.engine_pref(Level::Max), Some(EngineId::ChatGptPlan));
        s.engine_fast = Some("claude-plan".into());
        assert_eq!(s.engine_pref(Level::Standard), Some(EngineId::ClaudePlan));
        assert_eq!(s.engine_pref(Level::Max), Some(EngineId::ChatGptPlan));
        s.engine_max = Some(String::new());
        assert_eq!(s.engine_pref(Level::Max), Some(EngineId::ChatGptPlan));
    }

    #[cfg(unix)]
    #[test]
    fn save_is_mode_600_and_round_trips() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("pmx-settings-{}", std::process::id()));
        let p = dir.join("settings.json");
        let mut s = Settings::default();
        s.blocked_projects = vec!["~/Clients/bank".into()];
        save_to(&p, &s).unwrap();
        assert_eq!(load_from(&p), s);
        assert_eq!(std::fs::metadata(&p).unwrap().permissions().mode() & 0o777, 0o600);
        assert!(s.source_prefs().blocked_projects[0].ends_with("Clients/bank"));
        assert!(!s.source_prefs().blocked_projects[0].starts_with("~"));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
