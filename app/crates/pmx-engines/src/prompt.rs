//! The user message, built the same way for every engine (port of `enhance()` in `bin/promptmaxx`).

use crate::{Level, Request};

pub(crate) const MAX_CHARS: usize = 60_000;

pub(crate) fn level_word(level: Level) -> &'static str {
    match level {
        Level::Standard => "standard",
        Level::Max => "max",
    }
}

/// `<context>` line, optional `<target>` and `<project_brief>` blocks, the `<draft>`, the closing order.
/// `firm` adds the one-time reminder used for the single retry after a bad reply.
pub fn user_message(req: &Request, firm: bool) -> String {
    let mut s = format!("<context>App: {}. Level: {}.</context>\n", req.app_name, level_word(req.level));
    if let Some(t) = req.target.as_deref().map(str::trim).filter(|t| !t.is_empty()) {
        s.push_str(&format!("<target>{t}</target>\n"));
    }
    if let Some(b) = req.context_brief.as_deref().map(str::trim).filter(|b| !b.is_empty()) {
        s.push_str(&format!("<project_brief>\n{b}\n</project_brief>\n"));
    }
    s.push_str(&format!(
        "<draft>\n{}\n</draft>\n\nRewrite the draft as your instructions say. Output only the rewritten prompt.",
        req.draft.trim()
    ));
    if firm {
        s.push_str(
            " Your previous reply was a comment, not a prompt. This time your first word must be the first \
             word of the rewritten prompt. Never decline: you write the prompt, another AI does the task.",
        );
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    fn req() -> Request {
        Request {
            draft: "  the login page is broken fix it \n".into(),
            level: Level::Standard,
            app_name: "Ghostty".into(),
            target: None,
            context_brief: None,
            framework: String::new(),
        }
    }

    #[test]
    fn matches_python_runner_exactly() {
        // Same bytes as enhance() in bin/promptmaxx builds for this draft.
        assert_eq!(
            user_message(&req(), false),
            "<context>App: Ghostty. Level: standard.</context>\n<draft>\nthe login page is broken fix it\n</draft>\n\n\
             Rewrite the draft as your instructions say. Output only the rewritten prompt."
        );
    }

    #[test]
    fn firm_retry_adds_reminder() {
        let m = user_message(&req(), true);
        assert!(m.ends_with("Never decline: you write the prompt, another AI does the task."));
        assert!(m.contains("Output only the rewritten prompt. Your previous reply was a comment"));
    }

    #[test]
    fn target_and_brief_only_when_present() {
        let mut r = req();
        r.level = Level::Max;
        r.target = Some("coding agent: Claude Code".into());
        r.context_brief = Some("Project shop: Next.js store.".into());
        let m = user_message(&r, false);
        assert!(m.starts_with(
            "<context>App: Ghostty. Level: max.</context>\n<target>coding agent: Claude Code</target>\n\
             <project_brief>\nProject shop: Next.js store.\n</project_brief>\n<draft>\n"
        ));
        r.target = Some("  ".into());
        r.context_brief = Some(String::new());
        assert!(!user_message(&r, false).contains("<target>"));
        assert!(!user_message(&r, false).contains("<project_brief>"));
    }
}
