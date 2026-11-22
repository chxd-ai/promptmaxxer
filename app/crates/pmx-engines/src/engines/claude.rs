//! `claude-plan`: the person's Claude subscription through the Claude Code CLI (`claude -p`).

use crate::proc::{short, LineParser};
use crate::EngineError;
use serde_json::Value;
use std::path::Path;
use tokio::process::Command;

pub(crate) fn command(cli: &Path, model: &str, framework_file: &Path) -> Command {
    let mut c = Command::new(cli);
    c.args(["-p", "--model", model])
        // No user, project or local settings: no hooks, plugins, permissions or output styles.
        .args(["--setting-sources", ""])
        .arg("--strict-mcp-config")
        .args(["--tools", ""])
        .arg("--disable-slash-commands")
        .arg("--no-session-persistence")
        .arg("--system-prompt-file")
        .arg(framework_file)
        .args(["--output-format", "stream-json", "--verbose", "--include-partial-messages"])
        .current_dir(crate::paths::run_dir())
        .env("PATH", crate::paths::child_path(cli))
        // From inside the home folder Claude Code adds CLAUDE.md files and auto-memory to the call.
        .env("CLAUDE_CODE_DISABLE_CLAUDE_MDS", "1")
        .env("CLAUDE_CODE_DISABLE_AUTO_MEMORY", "1")
        // When the app itself was started from a Claude Code session, do not look nested.
        .env_remove("CLAUDECODE")
        .env_remove("CLAUDE_CODE_ENTRYPOINT")
        .env_remove("CLAUDE_CODE_SSE_PORT");
    c
}

/// `claude auth status --json`: no model call, about 0.2 s.
pub(crate) fn parse_auth_status(stdout: &str) -> Option<bool> {
    let v: Value = serde_json::from_str(stdout.trim()).ok()?;
    v.get("loggedIn")?.as_bool()
}

/// Parses `--output-format stream-json --include-partial-messages` lines.
#[derive(Default)]
pub(crate) struct Parser {
    streamed: String,
    result: Option<String>,
    is_error: bool,
}

impl LineParser for Parser {
    fn feed(&mut self, line: &str) -> Vec<String> {
        let Ok(v) = serde_json::from_str::<Value>(line) else { return vec![] };
        match v.get("type").and_then(Value::as_str) {
            Some("stream_event") => {
                let ev = &v["event"];
                if ev["type"] == "content_block_delta" && ev["delta"]["type"] == "text_delta" {
                    if let Some(t) = ev["delta"]["text"].as_str() {
                        self.streamed.push_str(t);
                        return vec![t.to_string()];
                    }
                }
                vec![]
            }
            Some("result") => {
                self.is_error = v["is_error"].as_bool().unwrap_or(false) || v["subtype"].as_str().is_some_and(|s| s != "success");
                self.result = v["result"].as_str().map(str::to_string);
                vec![]
            }
            _ => vec![],
        }
    }

    fn done(&self) -> bool {
        !self.is_error && self.result.as_deref().is_some_and(|r| !r.trim().is_empty())
    }

    fn finish(&mut self, ok: bool, stderr: &str) -> Result<String, EngineError> {
        if self.is_error || !ok {
            let msg = self.result.clone().filter(|s| !s.trim().is_empty()).unwrap_or_else(|| short(stderr));
            return Err(EngineError::Cli(if msg.is_empty() { "Claude Code failed with no output".into() } else { short(&msg) }));
        }
        match self.result.clone().filter(|s| !s.trim().is_empty()) {
            Some(r) => Ok(r),
            None if !self.streamed.trim().is_empty() => Ok(self.streamed.clone()),
            None => Err(EngineError::Cli("Claude Code returned no text".into())),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_deltas_and_result() {
        let mut p = Parser::default();
        let lines = [
            r#"{"type":"system","subtype":"init","model":"claude-haiku-5-5"}"#,
            r#"{"type":"stream_event","event":{"type":"message_start","message":{}}}"#,
            r#"{"type":"stream_event","event":{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"Fix the "}}}"#,
            r#"{"type":"stream_event","event":{"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"login page."}}}"#,
            r#"{"type":"assistant","message":{"content":[{"type":"text","text":"Fix the login page."}]}}"#,
            r#"{"type":"result","subtype":"success","is_error":false,"result":"Fix the login page."}"#,
        ];
        let deltas: Vec<String> = lines.iter().flat_map(|l| p.feed(l)).collect();
        assert_eq!(deltas, vec!["Fix the ", "login page."]);
        assert_eq!(p.finish(true, "").unwrap(), "Fix the login page.");
    }

    #[test]
    fn error_result_is_an_error() {
        let mut p = Parser::default();
        p.feed(r#"{"type":"result","subtype":"success","is_error":true,"result":"Not logged in · Please run /login"}"#);
        let e = p.finish(false, "").unwrap_err();
        assert!(e.to_string().contains("Not logged in"), "{e}");
    }

    #[test]
    fn auth_status() {
        assert_eq!(parse_auth_status(r#"{"loggedIn": true, "authMethod": "claude.ai"}"#), Some(true));
        assert_eq!(parse_auth_status(r#"{"loggedIn": false}"#), Some(false));
        assert_eq!(parse_auth_status("garbage"), None);
    }
}
