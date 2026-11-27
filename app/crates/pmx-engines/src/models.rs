//! Model choice: which model each engine uses, and the "any model" engine's endpoint.
//!
//! The app keeps a [`ModelConfig`] in its settings and hands it over with [`configure`]. Every call
//! reads it through [`current`], so a changed model name works on the next key press. Empty fields
//! mean "the default": a person who never opens the model settings gets the same models as before.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::sync::RwLock;

/// Model names for one engine. Empty = the engine's default.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct LevelModels {
    pub fast: String,
    pub max: String,
}

/// Any service that speaks the OpenAI Chat Completions API: OpenAI, OpenRouter, Groq, DeepSeek,
/// Mistral, Together, Gemini's OpenAI endpoint, or a model on this Mac through Ollama or LM Studio.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct CustomApi {
    /// "https://openrouter.ai/api/v1", "http://localhost:11434/v1". `/chat/completions` is added.
    pub base_url: String,
    /// Model for the fast key. Required.
    pub fast_model: String,
    /// Model for the max key. Empty = the fast model.
    pub max_model: String,
}

impl CustomApi {
    pub fn is_set(&self) -> bool {
        !self.base_url.trim().is_empty() && !self.fast_model.trim().is_empty()
    }

    /// The chat completions URL. Accepts a base with or without `/chat/completions` on the end.
    pub fn endpoint(&self) -> String {
        let b = self.base_url.trim().trim_end_matches('/');
        let b = b.strip_suffix("/chat/completions").unwrap_or(b);
        format!("{b}/chat/completions")
    }

    fn url(&self) -> Option<reqwest::Url> {
        reqwest::Url::parse(&self.endpoint()).ok()
    }

    /// The host (and port) the calls go to, lowercase: "openrouter.ai", "localhost:11434". Never the
    /// path or a user name or key. Empty when the address cannot be read.
    pub fn host(&self) -> String {
        let Some(u) = self.url() else { return String::new() };
        let h = u.host_str().unwrap_or("").to_lowercase();
        match u.port() {
            Some(p) if !h.is_empty() => format!("{h}:{p}"),
            _ => h,
        }
    }

    /// True for a server on this computer: no key is needed, and nothing leaves the machine.
    pub fn is_local(&self) -> bool {
        let Some(u) = self.url() else { return false };
        matches!(u.host_str().map(str::to_lowercase).as_deref(), Some("localhost" | "127.0.0.1" | "[::1]"))
    }

    pub fn is_https(&self) -> bool {
        self.url().is_some_and(|u| u.scheme() == "https")
    }

    /// A key may be sent only over https, or to a server on this computer.
    pub fn key_allowed(&self) -> bool {
        self.is_https() || self.is_local()
    }

    /// Keychain account for this address's key: `custom-api:<host>`. The key is only ever sent to
    /// the host it was saved for, so a later address change never takes the key along.
    pub fn key_account(&self) -> String {
        format!("{}:{}", crate::CUSTOM_KEY, self.host())
    }
}

/// Everything about model choice that the person can change.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ModelConfig {
    /// Per engine id ("claude-plan", ...): model names that replace the defaults.
    pub models: BTreeMap<String, LevelModels>,
    pub custom: CustomApi,
}

static CONFIG: RwLock<Option<ModelConfig>> = RwLock::new(None);

/// Set the model choice for every later call.
pub fn configure(cfg: ModelConfig) {
    *CONFIG.write().unwrap_or_else(|e| e.into_inner()) = Some(cfg);
}

/// The model choice now (defaults when [`configure`] was never called).
pub fn current() -> ModelConfig {
    CONFIG.read().unwrap_or_else(|e| e.into_inner()).clone().unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn endpoint_and_host() {
        let c = |u: &str| CustomApi { base_url: u.into(), fast_model: "m".into(), max_model: String::new() };
        assert_eq!(c("https://openrouter.ai/api/v1").endpoint(), "https://openrouter.ai/api/v1/chat/completions");
        assert_eq!(c("http://localhost:11434/v1/").endpoint(), "http://localhost:11434/v1/chat/completions");
        assert_eq!(c("https://x.ai/v1/chat/completions").endpoint(), "https://x.ai/v1/chat/completions");
        assert_eq!(c("https://openrouter.ai/api/v1").host(), "openrouter.ai");
        assert_eq!(c("http://user:pw@localhost:1234/v1").host(), "localhost:1234");
        assert!(c("http://localhost:11434/v1").is_local());
        assert!(c("http://127.0.0.1:1234/v1").is_local());
        assert!(!c("https://api.openai.com/v1").is_local());
        assert!(!CustomApi::default().is_set());
        assert!(c("http://localhost:11434/v1").is_set());
        assert!(c("http://[::1]:8080/v1").is_local());
        assert_eq!(c("HTTPS://API.OpenAI.com/v1").host(), "api.openai.com");
        assert_eq!(c("not a url").host(), "");
    }

    #[test]
    fn key_rules() {
        let c = |u: &str| CustomApi { base_url: u.into(), fast_model: "m".into(), max_model: String::new() };
        assert!(c("https://openrouter.ai/api/v1").key_allowed());
        assert!(c("http://localhost:11434/v1").key_allowed());
        assert!(!c("http://openrouter.ai/api/v1").key_allowed());
        assert!(!c("http://192.168.1.5:1234/v1").key_allowed());
        assert_eq!(c("https://openrouter.ai/api/v1").key_account(), "custom-api:openrouter.ai");
        assert_eq!(c("http://localhost:1234/v1").key_account(), "custom-api:localhost:1234");
        // A user name in the address does not change where the key goes.
        assert_eq!(c("https://openrouter.ai@evil.example/v1").key_account(), "custom-api:evil.example");
        assert_ne!(c("https://evil.example/v1").key_account(), c("https://openrouter.ai/api/v1").key_account());
    }

    #[test]
    fn missing_fields_fill_in() {
        let m: ModelConfig = serde_json::from_str(r#"{"models":{"claude-plan":{"max":"opus"}}}"#).unwrap();
        assert_eq!(m.models["claude-plan"].max, "opus");
        assert_eq!(m.models["claude-plan"].fast, "");
        assert!(!m.custom.is_set());
    }
}
