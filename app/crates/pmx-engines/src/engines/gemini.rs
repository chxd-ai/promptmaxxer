//! `gemini-plan`: the person's Google account through the Gemini CLI (`gemini -p`).
//!
//! The CLI loads `~/.gemini/AGENTS.md`/`GEMINI.md` (context files) and the person's hooks from
//! `~/.gemini/settings.json`. Each call therefore runs with `GEMINI_CLI_HOME` set to a throwaway home whose
//! settings keep only the sign-in method, and with `GEMINI_SYSTEM_MD` pointing at the framework file,
//! which replaces the CLI's own system prompt. OAuth tokens stay where the CLI keeps them: the macOS
//! keychain (service `gemini-cli-oauth`), or `oauth_creds.json`, which is linked, never copied.

use crate::proc::{short, LineParser};
use crate::EngineError;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use tokio::process::Command;

pub(crate) fn real_dir() -> PathBuf {
    std::env::var_os("GEMINI_CLI_HOME").map(PathBuf::from).unwrap_or_else(crate::paths::home).join(".gemini")
}

pub(crate) fn scratch_home() -> PathBuf {
    std::env::temp_dir().join("promptmaxxer-gemini-home")
}

/// The sign-in method saved by the CLI (`security.auth.selectedType`), if any.
pub(crate) fn selected_auth(settings_json: &str) -> Option<String> {
    let v: Value = serde_json::from_str(settings_json).ok()?;
    v.pointer("/security/auth/selectedType").and_then(Value::as_str).map(str::to_string)
}

/// Ready check without a model call: a saved sign-in method (OAuth tokens live in the keychain or
/// `oauth_creds.json`), or a `GEMINI_API_KEY` in the environment.
pub(crate) fn signed_in(real: &Path) -> (bool, &'static str) {
    let auth = std::fs::read_to_string(real.join("settings.json")).ok().and_then(|s| selected_auth(&s));
    let has_key = std::env::var_os("GEMINI_API_KEY").is_some_and(|k| !k.is_empty());
    match auth.as_deref() {
        Some("gemini-api-key") if !has_key => (false, "Not signed in (API key method, no key in the app's environment)"),
        Some(_) => (true, "Signed in"),
        None if has_key => (true, "Signed in"),
        None if real.join("oauth_creds.json").exists() => (true, "Signed in"),
        None => (false, "Not signed in"),
    }
}

pub(crate) fn prepare_home(real: &Path, scratch_home: &Path) -> std::io::Result<()> {
    crate::paths::setup_once(|| prepare_home_now(real, scratch_home))
}

fn prepare_home_now(real: &Path, scratch_home: &Path) -> std::io::Result<()> {
    let dir = scratch_home.join(".gemini");
    std::fs::create_dir_all(&dir)?;
    // Windows: the per-user temp folder is already private to the user.
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(scratch_home, std::fs::Permissions::from_mode(0o700))?;
    }
    let auth = std::fs::read_to_string(real.join("settings.json")).ok().and_then(|s| selected_auth(&s));
    let mut settings = json!({
        // A context file name that never exists, so no AGENTS.md or GEMINI.md is loaded.
        "context": { "fileName": ["PROMPTMAXXER-NO-CONTEXT.md"] },
        "telemetry": { "enabled": false },
        "privacy": { "usageStatisticsEnabled": false },
    });
    if let Some(a) = auth {
        settings["security"] = json!({ "auth": { "selectedType": a } });
    }
    // Write then rename: a run that starts at the same moment never reads a half-written file.
    let tmp = dir.join(format!("settings.json.{}-{:?}.tmp", std::process::id(), std::thread::current().id()));
    std::fs::write(&tmp, serde_json::to_vec_pretty(&settings)?)?;
    std::fs::rename(&tmp, dir.join("settings.json"))?;
    for name in ["oauth_creds.json", "google_accounts.json"] {
        let target = real.join(name);
        let link = dir.join(name);
        #[cfg(windows)]
        {
            if target.exists() {
                crate::paths::share_file(&target, &link)?;
            } else {
                let _ = std::fs::remove_file(&link);
            }
        }
        #[cfg(unix)]
        {
            if !target.exists() {
                let _ = std::fs::remove_file(&link);
            } else if std::fs::read_link(&link).ok().as_deref() != Some(target.as_path()) {
                let _ = std::fs::remove_file(&link);
                crate::paths::symlink_shared(&target, &link)?;
            }
        }
    }
    for name in ["AGENTS.md", "GEMINI.md"] {
        let _ = std::fs::remove_file(dir.join(name));
    }
    Ok(())
}

/// The user message goes on stdin, never in the arguments, where any process list would show it. With stdin
/// not a terminal the CLI runs headless and takes stdin as the prompt (checked in Gemini CLI 0.52.0:
/// `isHeadlessMode` is true when stdin is not a TTY, and the piped text becomes the whole input when no
/// `--prompt` is given).
pub(crate) fn command(cli: &Path, model: &str, framework_file: &Path, home: &Path) -> Command {
    let mut c = crate::paths::cli_command(cli);
    c.args(["--model", model, "--output-format", "stream-json", "--approval-mode", "plan", "--skip-trust"])
        .current_dir(crate::paths::run_dir())
        .env("PATH", crate::paths::child_path(cli))
        .env("GEMINI_CLI_HOME", home)
        .env("GEMINI_SYSTEM_MD", framework_file)
        .env("GEMINI_CLI_NO_RELAUNCH", "true");
    c
}

/// Parses `--output-format stream-json` lines: `message` (assistant, `delta: true`) and `result`.
#[derive(Default)]
pub(crate) struct Parser {
    text: String,
    error: Option<String>,
}

impl LineParser for Parser {
    fn feed(&mut self, line: &str) -> Vec<String> {
        let Ok(v) = serde_json::from_str::<Value>(line) else { return vec![] };
        match v["type"].as_str() {
            Some("message") if v["role"] == "assistant" => {
                let t = v["content"].as_str().unwrap_or("").to_string();
                self.text.push_str(&t);
                vec![t]
            }
            Some("result") if v["status"].as_str().is_some_and(|s| s != "success") => {
                self.error = Some(v["error"]["message"].as_str().unwrap_or("Gemini CLI failed").to_string());
                vec![]
            }
            Some("error") => {
                self.error = Some(v["message"].as_str().unwrap_or("Gemini CLI error").to_string());
                vec![]
            }
            _ => vec![],
        }
    }

    fn finish(&mut self, ok: bool, stderr: &str) -> Result<String, EngineError> {
        if let Some(e) = self.error.clone() {
            return Err(EngineError::Cli(short(&e)));
        }
        if !self.text.trim().is_empty() && ok {
            return Ok(self.text.clone());
        }
        let msg = short(stderr);
        Err(EngineError::Cli(if msg.is_empty() { "Gemini CLI returned no text".into() } else { msg }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_stream_json() {
        let mut p = Parser::default();
        let lines = [
            r#"{"type":"init","timestamp":"t","session_id":"s","model":"gemini-3.5-flash"}"#,
            r#"{"type":"message","role":"user","content":"<context>..."}"#,
            r#"{"type":"message","role":"assistant","content":"Fix the ","delta":true}"#,
            r#"{"type":"message","role":"assistant","content":"login page.","delta":true}"#,
            r#"{"type":"result","status":"success","stats":{}}"#,
        ];
        let d: Vec<String> = lines.iter().flat_map(|l| p.feed(l)).collect();
        assert_eq!(d, vec!["Fix the ", "login page."]);
        assert_eq!(p.finish(true, "").unwrap(), "Fix the login page.");
    }

    #[test]
    fn error_result() {
        let mut p = Parser::default();
        p.feed(r#"{"type":"result","status":"error","error":{"type":"auth","message":"Please set an Auth method"}}"#);
        assert!(p.finish(false, "").unwrap_err().to_string().contains("Auth method"));
    }

    #[test]
    fn sign_in_detection() {
        let base = std::env::temp_dir().join(format!("pmx-gemini-test-{}", std::process::id()));
        std::fs::create_dir_all(&base).unwrap();
        std::fs::write(base.join("settings.json"), r#"{"context":{"fileName":["AGENTS.md"]}}"#).unwrap();
        if std::env::var_os("GEMINI_API_KEY").is_none() {
            assert_eq!(signed_in(&base), (false, "Not signed in"));
        }
        std::fs::write(base.join("settings.json"), r#"{"security":{"auth":{"selectedType":"oauth-personal"}}}"#).unwrap();
        assert_eq!(signed_in(&base), (true, "Signed in"));

        // The scratch home keeps the sign-in method and drops context files and hooks.
        let scratch = base.join("scratch");
        std::fs::write(base.join("AGENTS.md"), "secret rules").unwrap();
        prepare_home(&base, &scratch).unwrap();
        let s = std::fs::read_to_string(scratch.join(".gemini/settings.json")).unwrap();
        assert_eq!(selected_auth(&s).as_deref(), Some("oauth-personal"));
        assert!(!s.contains("hooks"));
        assert!(!scratch.join(".gemini/AGENTS.md").exists());
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn prepare_home_runs_side_by_side() {
        let base = std::env::temp_dir().join(format!("pmx-gemini-race-{}", std::process::id()));
        std::fs::create_dir_all(&base).unwrap();
        std::fs::write(base.join("settings.json"), r#"{"security":{"auth":{"selectedType":"oauth-personal"}}}"#).unwrap();
        std::fs::write(base.join("oauth_creds.json"), "{}").unwrap();
        let scratch = base.join("scratch");
        std::thread::scope(|s| {
            let hs: Vec<_> = (0..8).map(|_| s.spawn(|| (0..20).try_for_each(|_| prepare_home(&base, &scratch)))).collect();
            for h in hs {
                h.join().unwrap().unwrap();
            }
        });
        assert!(scratch.join(".gemini/oauth_creds.json").exists());
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn command_uses_system_md_and_home() {
        let c = command(Path::new("/x/gemini"), "flash", Path::new("/tmp/f.md"), Path::new("/tmp/h"));
        let args: Vec<String> = c.as_std().get_args().map(|a| a.to_string_lossy().into_owned()).collect();
        assert!(!args.iter().any(|a| a == "--prompt" || a == "-p"), "{args:?}");
        let envs: Vec<(String, String)> = c
            .as_std()
            .get_envs()
            .filter_map(|(k, v)| Some((k.to_string_lossy().into_owned(), v?.to_string_lossy().into_owned())))
            .collect();
        assert!(envs.contains(&("GEMINI_SYSTEM_MD".into(), "/tmp/f.md".into())));
        assert!(envs.contains(&("GEMINI_CLI_HOME".into(), "/tmp/h".into())));
    }
}
