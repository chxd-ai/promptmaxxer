//! pmx-context: the part of promptmaxxer that knows which app and AI the person is
//! writing into, which project they are in, and what that project's instruction files say.
//!
//! No Tauri dependency.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;

mod app;
mod brief;
mod procs;
mod project;
mod secrets;
mod sources;
mod target;

pub use app::{front_app, window_title};
pub use brief::{brief, brief_fingerprint, brief_in, cached_brief_text, cached_brief_text_in, default_cache_dir, BRIEF_MAX_WORDS, BRIEF_PROMPT, SOURCE_TEXT_CAP};
pub use project::detect_project;
pub use secrets::{scrub_secrets, SECRET_MARK};
pub use sources::{chip_label, fingerprint, sources};
pub use target::target_for;

/// The app in front.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AppInfo {
    pub name: String,
    pub bundle_id: String,
    pub pid: i32,
}

/// The kind of AI that receives the prompt.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TargetKind {
    CodingAgent,
    Chat,
    Research,
    Writing,
    Image,
    Video,
    Data,
    BotBuilding,
    Unknown,
}

impl TargetKind {
    /// Plain words for the prompt, e.g. "coding agent".
    pub fn as_str(&self) -> &'static str {
        match self {
            TargetKind::CodingAgent => "coding agent",
            TargetKind::Chat => "chat",
            TargetKind::Research => "research",
            TargetKind::Writing => "writing",
            TargetKind::Image => "image",
            TargetKind::Video => "video",
            TargetKind::Data => "data",
            TargetKind::BotBuilding => "bot building",
            TargetKind::Unknown => "unknown",
        }
    }
}

/// The receiving AI.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Target {
    pub kind: TargetKind,
    /// "Claude Code", "ChatGPT", "Midjourney", ... "Unknown" when not known.
    pub label: String,
}

impl Target {
    /// The `target` string for `pmx-engines::Request`, e.g. "coding agent: Claude Code".
    /// None when the target is unknown (the model decides from the text).
    pub fn hint(&self) -> Option<String> {
        match self.kind {
            TargetKind::Unknown => None,
            k => Some(format!("{}: {}", k.as_str(), self.label)),
        }
    }
}

/// The project the person works in.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Project {
    /// Git root (or the working folder when it is not in a Git repo).
    pub root: PathBuf,
    /// The working folder (shell cwd, editor folder). Always inside `root`.
    pub cwd: PathBuf,
    /// Folder name of `root`.
    pub name: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SourceLevel {
    User,
    Project,
}

/// One instruction file that feeds the brief.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Source {
    pub path: PathBuf,
    /// "CLAUDE.md", "CLAUDE.local.md", "AGENTS.md" or "memory".
    pub label: String,
    pub level: SourceLevel,
}

/// Which sources the person allows (from settings).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct SourcePrefs {
    /// The app's own "About me" notes (text, not a file).
    pub about_me: String,
    /// Read `~/.claude/CLAUDE.md`.
    pub claude_user: bool,
    /// Read `~/.codex/AGENTS.md`.
    pub codex_user: bool,
    /// Read the project's own files and Claude's project memory index.
    pub project_files: bool,
    /// Projects the person said "never read".
    pub blocked_projects: Vec<PathBuf>,
}

impl SourcePrefs {
    /// Everything on, no About me text, nothing blocked.
    pub fn all_on() -> Self {
        SourcePrefs { about_me: String::new(), claude_user: true, codex_user: true, project_files: true, blocked_projects: vec![] }
    }
}

/// A short project brief (max 300 words).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Brief {
    pub text: String,
    pub sources: Vec<Source>,
    pub fingerprint: String,
    /// True when it came from the cache and `summarize` was not called.
    pub cached: bool,
}

/// Diagnostics: which method found a window title or a project, and the permission state.
/// For the probe and the settings screen; never prompts for a permission.
pub mod diag {
    pub use crate::app::{accessibility_trusted, screen_recording_allowed, window_title_with_source};
    pub use crate::project::detect_project_with_source;
}

pub(crate) fn home_dir() -> PathBuf {
    if let Some(h) = std::env::var_os("HOME") {
        if !h.is_empty() {
            return PathBuf::from(h);
        }
    }
    // Fall back to the password database.
    unsafe {
        let pw = libc::getpwuid(libc::getuid());
        if !pw.is_null() && !(*pw).pw_dir.is_null() {
            let s = std::ffi::CStr::from_ptr((*pw).pw_dir).to_string_lossy().into_owned();
            return PathBuf::from(s);
        }
    }
    PathBuf::from("/")
}
