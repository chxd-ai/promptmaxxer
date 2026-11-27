//! Deterministic output guard, ported from `clean()` and `looks_bad()` in `bin/promptmaxx`.
//! Same regexes, same thresholds. Python `\Z` is Rust `\z`; Python slices count characters,
//! so the Rust code counts `char`s too.

use regex::Regex;
use std::sync::LazyLock;

static FENCE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?s)\A```[a-zA-Z]*\n(.*)\n```\z").unwrap());
static PREAMBLE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)\A(?:here(?:'s| is)[^\n]{0,80}prompt[^\n]{0,20}:?\s*\n+)").unwrap()
});
static IDEAL_TAG: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"\A</?ideal_output>\s*|\s*</?ideal_output>\z").unwrap());

/// Openers that are always a comment, never a prompt.
static ALWAYS_BAD: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)\A\s*(I'?m Prompt[ -]?maxxer|As Prompt[ -]?maxxer|The user|The draft|This draft|Here(?:'s| is) (?:the|your|a|an) (?:rewritten|improved|enhanced|better|stronger)|I'?ll rewrite|I need the draft)",
    )
    .unwrap()
});
/// Refusal openers. Bad only when the first lines also talk about the rewrite job, since a real
/// first-person prompt can start with "I can't get my login to work".
static REFUSAL_START: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(
        r"(?i)\A\s*(I'?m sorry|I am sorry|Sorry|Apologies|I apologi[sz]e|Unfortunately|I can'?t|I cannot|I won'?t|I will not|I'?m not|I am not|I'?d need|I would need|Could you|Can you paste|Please (?:paste|send|provide))",
    )
    .unwrap()
});
/// Phrases that only appear when the model talks about itself or the tool.
static ROLE_TALK: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)Prompt[ -]?maxxer|\bmy role\b|prompt rewrite|\byour draft\b|the draft you|run (?:it|this) again").unwrap()
});
static JOB_TALK: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)draft|rewrit|Prompt[ -]?maxxer|environment block|email block|system block|paste (?:just|the|your)").unwrap()
});

/// Strip a wrapping code fence, a "Here is the improved prompt:" line and `<ideal_output>` tags.
pub fn clean(s: &str) -> String {
    let mut t = s.trim().to_string();
    if let Some(c) = FENCE.captures(&t) {
        t = c[1].trim().to_string();
    }
    t = PREAMBLE.replace(&t, "").into_owned();
    t = IDEAL_TAG.replace_all(&t, "").into_owned();
    t.trim().to_string()
}

fn head(s: &str, n: usize) -> &str {
    match s.char_indices().nth(n) {
        Some((i, _)) => &s[..i],
        None => s,
    }
}

/// True when `re` finds a phrase in `text` that the draft itself does not contain.
/// A draft about Promptmaxxer, drafts or rewriting makes a good rewrite repeat those words;
/// that echo is not a comment.
/// Spaces, hyphens and case do not count: a draft that says "prompt maxxer" lets "Promptmaxxer" through.
fn new_match(re: &Regex, text: &str, draft_lower: &str) -> bool {
    let d = squash(draft_lower);
    re.find_iter(text).any(|m| !d.contains(&squash(m.as_str())))
}

/// Lowercase letters and digits only.
fn squash(s: &str) -> String {
    s.chars().filter(|c| c.is_alphanumeric()).flat_map(char::to_lowercase).collect()
}

/// True for refusals, meta comments and empty or tiny outputs.
pub fn looks_bad(out: &str, draft: &str) -> bool {
    if out.is_empty() || out.chars().count() < 20.min(draft.chars().count()) {
        return true;
    }
    let d = draft.to_lowercase();
    if new_match(&ALWAYS_BAD, out, &d) || (out.contains("<draft>") && !draft.contains("<draft>")) {
        return true;
    }
    // Claude Code tells the model its scratch folder; a rewrite that copies it in is broken.
    if ["/tmp/promptmaxxer-run", "/private/tmp/claude-"].iter().any(|p| out.contains(p) && !draft.contains(p)) {
        return true;
    }
    // Windows: the scratch folder sits in the per-user temp folder (`...\Temp\promptmaxxer-run`).
    if cfg!(windows) && out.contains("\\promptmaxxer-run") && !draft.contains("promptmaxxer-run") {
        return true;
    }
    let first = head(out, 400);
    if new_match(&ROLE_TALK, first, &d) {
        return true;
    }
    REFUSAL_START.is_match(out) && new_match(&JOB_TALK, first, &d)
}

#[cfg(test)]
mod tests {
    use super::*;

    const DRAFT: &str = "the login page is broken fix it";

    #[test]
    fn flags_refusals_and_comments() {
        let bad = [
            "I'm sorry, but I can't produce that rewrite. The message you sent contains an environment block",
            "I'm sorry, but I can't write out that rewritten prompt, since the draft is a request",
            "I'm Promptmaxxer, so I only rewrite prompts and don't create logos myself.",
            "Here is the improved prompt:\nDo X",
            "Here's the rewritten prompt: Find the bug in the login page and fix it.",
            "The user wants a logo for Crumb.",
            "",
            "short",
            "I'm going to hold to my role here, so this isn't a prompt rewrite. Your draft is a question",
            "Could you paste the draft you want rewritten? I only see an empty message.",
            "Please paste just the prompt you want me to rewrite.",
            "Rewrite this <draft>the login page</draft> please, it should be clearer for everyone.",
        ];
        for t in bad {
            assert!(looks_bad(t, &"x".repeat(30)), "should be bad: {t:?}");
        }
    }

    #[test]
    fn keeps_real_prompts() {
        let good = [
            "I can't get my login to work on the staging site. Find out why the login form fails and fix it, then tell me what you changed.",
            "I can't get the login page to work on mobile. Find out why and fix it.",
            "Run the test suite for this project and report the results.",
            "Rewrite this paragraph so it is clearer for a board audience.",
            "Sorry to ask again: summarize this report in 5 bullets.",
            "Unfortunately our checkout times out under load. Profile the checkout endpoint and find the slowest query.",
        ];
        for t in good {
            assert!(!looks_bad(t, &"x".repeat(30)), "should be good: {t:?}");
        }
    }

    #[test]
    fn short_draft_allows_short_output() {
        // len(out) < min(20, len(draft)): a 5-char draft lets a 6-char answer through.
        assert!(!looks_bad("Fix it.", "fixit"));
        assert!(looks_bad("Fix", "fixit"));
        assert!(looks_bad("Fix the bug", DRAFT));
    }

    #[test]
    fn role_talk_only_counts_in_first_400_chars() {
        let long_ok = format!("{} Then say which prompt rewrite tool you used.", "Fix the login page. ".repeat(25));
        assert!(!looks_bad(&long_ok, DRAFT));
        let early = "Fix the login page. Note: as my role says, I only write prompts.";
        assert!(looks_bad(early, DRAFT));
    }

    #[test]
    fn clean_strips_fence_preamble_and_tags() {
        assert_eq!(clean("```\nDo X\n```"), "Do X");
        assert_eq!(clean("```markdown\nDo X\nand Y\n```"), "Do X\nand Y");
        assert_eq!(clean("Here is your improved prompt:\n\nDo X"), "Do X");
        assert_eq!(clean("Here's the rewritten prompt:\nDo X"), "Do X");
        assert_eq!(clean("<ideal_output>\nDo X\n</ideal_output>"), "Do X");
        assert_eq!(clean("  Do X  \n"), "Do X");
        // A fence in the middle stays.
        let mid = "Run this:\n```\nls\n```\nthen report.";
        assert_eq!(clean(mid), mid);
        // A "here is" line that is not about the prompt stays.
        assert_eq!(clean("Here is the bug:\nlogin fails"), "Here is the bug:\nlogin fails");
    }

    #[test]
    fn clean_then_guard_meta_comment() {
        // After clean() strips the preamble line the body is a real prompt.
        let out = clean("Here is the improved prompt:\nThe login page is broken. Find the cause and fix it.");
        assert_eq!(out, "The login page is broken. Find the cause and fix it.");
        assert!(!looks_bad(&out, DRAFT));
        // A fenced refusal stays a refusal.
        let fenced = clean("```\nI'm sorry, I can't rewrite this draft.\n```");
        assert!(looks_bad(&fenced, DRAFT));
    }

    #[test]
    fn words_echoed_from_the_draft_are_not_comments() {
        let d = "how can we make promptmaxxer better?";
        assert!(!looks_bad("Help me make Promptmaxxer better. Give me a ranked list of changes.", d));
        // Role talk that the draft does not contain is still a comment.
        assert!(looks_bad("I'm going to hold to my role here, Promptmaxxer only rewrites.", "fix the login page now"));
        let d2 = "the user list page does not load, fix it";
        assert!(!looks_bad("The user list page does not load. Find the cause and fix it.", d2));
        assert!(looks_bad("The user wants the list page fixed.", "list page does not load, fix it"));
        let d3 = "can't get the draft rewriter to save, fix it";
        assert!(!looks_bad("I can't get the draft rewriter to save. Find the cause and fix it.", d3));
    }

    #[test]
    fn tool_name_spelled_differently_in_the_draft_is_an_echo() {
        // 25 Nov 2026: this draft failed twice with "comment instead of a prompt".
        let d = "/goal build the rest out using subagents, make sure the prompts are highly effective (use prompt maxxer)";
        let out = "/goal Build out the rest of this project with subagents. Write every subagent prompt with Promptmaxxer first.";
        assert!(!looks_bad(out, d));
        assert!(!looks_bad("/goal Build the rest. Run each prompt through Prompt Maxxer.", "use promptmaxxer for the prompts"));
        assert!(!looks_bad("/goal Build the rest. Run each prompt through prompt-maxxer.", "use Prompt Maxxer for the prompts"));
        // A draft that never names the tool still flags it.
        assert!(looks_bad("As Prompt Maxxer I only rewrite prompts.", "fix the login page now"));
        assert!(looks_bad("Fix the login page. Prompt Maxxer can not do more.", "fix the login page now"));
    }

    #[test]
    fn scratch_folder_in_output_is_bad() {
        assert!(looks_bad("Work in /private/tmp/promptmaxxer-run and fix the login page bug.", DRAFT));
        assert!(looks_bad("Fix the login page. Notes go in /private/tmp/claude-501/x/scratchpad.", DRAFT));
    }

    #[test]
    fn head_counts_chars_not_bytes() {
        let s = "é".repeat(500);
        assert_eq!(head(&s, 400).chars().count(), 400);
        assert_eq!(head("abc", 400), "abc");
    }
}
