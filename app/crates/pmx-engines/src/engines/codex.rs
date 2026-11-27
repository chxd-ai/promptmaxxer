//! `chatgpt-plan`: the person's ChatGPT subscription through the Codex CLI (`codex exec`).
//!
//! Codex always reads `$CODEX_HOME/AGENTS.md` as global instructions (checked on 0.169.0: neither
//! `--ignore-user-config` nor `project_doc_max_bytes=0` stops it). So each call runs with a throwaway
//! `CODEX_HOME` that holds only a symlink to the real `auth.json`. Codex writes a refreshed token through
//! the symlink (checked: `codex login` into a home whose `auth.json` is a symlink updates the target and
//! keeps the link), so the person's real sign-in stays current. The `auth.lock` link shares the lock.

use crate::proc::{short, LineParser};
use crate::EngineError;
use serde_json::Value;
use std::path::{Path, PathBuf};
use tokio::process::Command;

pub(crate) fn real_home() -> PathBuf {
    std::env::var_os("CODEX_HOME").map(PathBuf::from).unwrap_or_else(|| crate::paths::home().join(".codex"))
}

pub(crate) fn scratch_home() -> PathBuf {
    std::env::temp_dir().join("promptmaxxer-codex-home")
}

/// Create the throwaway home: mode 700, links to `auth.json` and the auth lock files, nothing else.
pub(crate) fn prepare_home(real: &Path, scratch: &Path) -> std::io::Result<()> {
    crate::paths::setup_once(|| prepare_home_now(real, scratch))
}

fn prepare_home_now(real: &Path, scratch: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(scratch)?;
    // Windows: the per-user temp folder is already private to the user.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(scratch, std::fs::Permissions::from_mode(0o700))?;
    }
    for name in ["auth.json", "auth.lock", "auth.json.lock"] {
        let target = real.join(name);
        let link = scratch.join(name);
        let current = std::fs::read_link(&link).ok();
        if !target.exists() {
            if current.is_some() {
                let _ = std::fs::remove_file(&link);
            }
            continue;
        }
        #[cfg(windows)]
        {
            let _ = current;
            crate::paths::share_file(&target, &link)?;
            continue;
        }
        #[cfg(unix)]
        if current.as_deref() == Some(target.as_path()) {
            continue;
        }
        // A plain file here would be a stale copy; replace it with the link.
        #[cfg(unix)]
        {
            let _ = std::fs::remove_file(&link);
            crate::paths::symlink_shared(&target, &link)?;
        }
    }
    // Never let a global instructions file sit in the scratch home.
    for name in ["AGENTS.md", "AGENTS.override.md", "config.toml"] {
        let _ = std::fs::remove_file(scratch.join(name));
    }
    Ok(())
}

/// Reasoning effort per model choice.
pub(crate) fn effort(model: &str) -> &'static str {
    if model.contains("astra") { "medium" } else { "low" }
}

pub(crate) fn command(cli: &Path, model: &str, framework_file: &Path, home: &Path) -> Command {
    let mut c = crate::paths::cli_command(cli);
    let fw = toml_string(&framework_file.to_string_lossy());
    c.arg("exec")
        .args(["--json", "--skip-git-repo-check", "--ephemeral", "--ignore-user-config", "--ignore-rules"])
        .args(["--sandbox", "read-only", "--model", model])
        .arg("--cd")
        .arg(crate::paths::run_dir())
        // The framework replaces Codex's own base instructions.
        .args(["-c", &format!("model_instructions_file={fw}")])
        .args(["-c", &format!("model_reasoning_effort=\"{}\"", effort(model))])
        .args(["-c", "model_reasoning_summary=\"none\""])
        .args(["-c", "web_search=\"disabled\""])
        // Drop the extra instruction blocks Codex adds around the conversation.
        .args(["-c", "include_permissions_instructions=false"])
        .args(["-c", "include_collaboration_mode_instructions=false"])
        .args(["-c", "include_environment_context=false"])
        .args(["-c", "include_apps_instructions=false"])
        .args(["-c", "skills.include_instructions=false"])
        .args(["-c", "skills.bundled.enabled=false"])
        .args(["-c", "project_doc_max_bytes=0"]);
    for f in ["shell_tool", "unified_exec", "multi_agent", "plugins", "apps", "hooks", "image_generation", "browser_use", "computer_use", "goals", "tool_suggest", "memories"] {
        c.args(["--disable", f]);
    }
    c.arg("-") // prompt from stdin
        .env("CODEX_HOME", home)
        .env("PATH", crate::paths::child_path(cli))
        // Force the ChatGPT sign-in, never an API key that happens to be in the environment.
        .env_remove("OPENAI_API_KEY")
        .env_remove("CODEX_API_KEY")
        .env_remove("CODEX_ACCESS_TOKEN");
    c
}

fn toml_string(s: &str) -> String {
    format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
}

/// `codex login status` text: "Logged in using ChatGPT", "Logged in using an API key ...", "Not logged in".
pub(crate) fn parse_login_status(out: &str) -> (bool, &'static str) {
    let o = out.to_lowercase();
    if o.contains("logged in using chatgpt") {
        (true, "Signed in")
    } else if o.contains("not logged in") {
        (false, "Not signed in")
    } else if o.contains("logged in using an api key") || o.contains("api key") {
        // An API key bills per call: never use it for the "ChatGPT plan" engine.
        (false, "Signed in with an API key, not a ChatGPT plan")
    } else if o.contains("logged in") {
        (true, "Signed in")
    } else {
        (false, "Not signed in")
    }
}

/// Parses `codex exec --json` events.
#[derive(Default)]
pub(crate) struct Parser {
    /// Text of the agent message being streamed, to turn `item.updated` snapshots into deltas.
    live: String,
    last_message: Option<String>,
    fatal: Option<String>,
    turn_done: bool,
}

impl LineParser for Parser {
    fn feed(&mut self, line: &str) -> Vec<String> {
        let Ok(v) = serde_json::from_str::<Value>(line) else { return vec![] };
        let ty = v["type"].as_str().unwrap_or("");
        match ty {
            "item.started" | "item.updated" | "item.completed" if v["item"]["type"] == "agent_message" => {
                let text = v["item"]["text"].as_str().unwrap_or("").to_string();
                let delta = if text.starts_with(&self.live) { text[self.live.len()..].to_string() } else { String::new() };
                if ty == "item.completed" {
                    self.last_message = Some(text.clone());
                }
                self.live = text;
                vec![delta]
            }
            // Delta events, in case this Codex version streams them.
            t if t.ends_with("agent_message.delta") || t.ends_with("agent_message_delta") => {
                let d = v["delta"].as_str().unwrap_or("").to_string();
                self.live.push_str(&d);
                vec![d]
            }
            "turn.completed" => {
                self.turn_done = true;
                vec![]
            }
            "turn.failed" => {
                self.fatal = Some(v["error"]["message"].as_str().unwrap_or("Codex turn failed").to_string());
                vec![]
            }
            "error" => {
                // "Reconnecting... n/5" lines are retries, not the end. Keep the last one in case it is.
                let m = v["message"].as_str().unwrap_or("").to_string();
                if !m.starts_with("Reconnecting") {
                    self.fatal = Some(m);
                }
                vec![]
            }
            _ => vec![],
        }
    }

    fn done(&self) -> bool {
        self.turn_done && self.last_message.is_some()
    }

    fn finish(&mut self, ok: bool, stderr: &str) -> Result<String, EngineError> {
        if let Some(m) = self.last_message.clone().filter(|m| !m.trim().is_empty()) {
            return Ok(m);
        }
        let msg = self.fatal.clone().unwrap_or_else(|| short(stderr));
        Err(EngineError::Cli(if msg.is_empty() {
            format!("Codex returned no text (exit {})", if ok { "ok" } else { "error" })
        } else {
            short(&msg)
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn login_status_texts() {
        assert_eq!(parse_login_status("Logged in using ChatGPT\n"), (true, "Signed in"));
        assert_eq!(parse_login_status("Not logged in\n"), (false, "Not signed in"));
        assert_eq!(parse_login_status("Logged in using an API key - sk-****"), (false, "Signed in with an API key, not a ChatGPT plan"));
    }

    #[test]
    fn parses_completed_message() {
        let mut p = Parser::default();
        let lines = [
            r#"{"type":"thread.started","thread_id":"t"}"#,
            r#"{"type":"turn.started"}"#,
            r#"{"type":"item.completed","item":{"id":"item_0","type":"reasoning","text":"thinking"}}"#,
            r#"{"type":"item.completed","item":{"id":"item_1","type":"agent_message","text":"Fix the login page."}}"#,
            r#"{"type":"turn.completed","usage":{"input_tokens":10,"output_tokens":5}}"#,
        ];
        let deltas: Vec<String> = lines.iter().flat_map(|l| p.feed(l)).filter(|d| !d.is_empty()).collect();
        assert_eq!(deltas, vec!["Fix the login page."]);
        assert!(p.done());
        assert_eq!(p.finish(true, "").unwrap(), "Fix the login page.");
    }

    #[test]
    fn snapshots_become_deltas() {
        let mut p = Parser::default();
        let a = p.feed(r#"{"type":"item.started","item":{"type":"agent_message","text":""}}"#);
        let b = p.feed(r#"{"type":"item.updated","item":{"type":"agent_message","text":"Fix the"}}"#);
        let c = p.feed(r#"{"type":"item.updated","item":{"type":"agent_message","text":"Fix the login"}}"#);
        let d = p.feed(r#"{"type":"item.completed","item":{"type":"agent_message","text":"Fix the login page."}}"#);
        assert_eq!([a, b, c, d].concat().concat(), "Fix the login page.");
    }

    #[test]
    fn retries_are_not_fatal_but_turn_failed_is() {
        let mut p = Parser::default();
        p.feed(r#"{"type":"error","message":"Reconnecting... 2/5 (stream disconnected)"}"#);
        p.feed(r#"{"type":"turn.failed","error":{"message":"You've hit your usage limit."}}"#);
        assert!(p.finish(false, "").unwrap_err().to_string().contains("usage limit"));
    }

    #[cfg(unix)]
    #[test]
    fn scratch_home_links_auth_only() {
        let base = std::env::temp_dir().join(format!("pmx-codex-test-{}", std::process::id()));
        let real = base.join("real");
        let scratch = base.join("scratch");
        std::fs::create_dir_all(&real).unwrap();
        std::fs::write(real.join("auth.json"), "{}").unwrap();
        std::fs::write(real.join("AGENTS.md"), "secret rules").unwrap();
        std::fs::create_dir_all(&scratch).unwrap();
        std::fs::write(scratch.join("AGENTS.md"), "stale").unwrap();
        prepare_home(&real, &scratch).unwrap();
        prepare_home(&real, &scratch).unwrap(); // idempotent
        assert_eq!(std::fs::read_link(scratch.join("auth.json")).unwrap(), real.join("auth.json"));
        assert!(!scratch.join("AGENTS.md").exists());
        assert!(!scratch.join("auth.lock").exists());
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn command_has_no_user_config_and_reads_stdin() {
        let c = command(Path::new("/x/codex"), "gpt-6.2-sol", Path::new("/tmp/promptmaxxer-run/f.md"), Path::new("/tmp/h"));
        let args: Vec<String> = c.as_std().get_args().map(|a| a.to_string_lossy().into_owned()).collect();
        assert!(args.contains(&"--ignore-user-config".to_string()));
        assert!(args.contains(&"model_instructions_file=\"/tmp/promptmaxxer-run/f.md\"".to_string()));
        assert_eq!(args.last().unwrap(), "-");
    }
}
