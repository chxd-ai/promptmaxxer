//! Run one CLI call: write stdin, read stdout line by line, stream deltas, obey cancel and timeout.
//! The child gets its own process group, and cancel or timeout kills the whole group.

use crate::{EngineError, Event};
use std::process::Stdio;
use std::time::Duration;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::Command;
use tokio::sync::mpsc::UnboundedSender;
use tokio_util::sync::CancellationToken;

/// Turns the CLI's stdout lines into text deltas and a final text.
pub(crate) trait LineParser: Send {
    /// One stdout line in; zero or more text deltas out.
    fn feed(&mut self, line: &str) -> Vec<String>;
    /// Called after the process ended. `ok` is the exit status.
    fn finish(&mut self, ok: bool, stderr: &str) -> Result<String, EngineError>;
    /// True once the answer is complete, so the caller can stop the CLI without waiting for it to exit
    /// (Codex takes about a second to shut down after its last event).
    fn done(&self) -> bool {
        false
    }
}

fn kill_group(pid: Option<u32>) {
    if let Some(pid) = pid {
        // SAFETY: plain syscall; the group id is the child's pid because of process_group(0).
        unsafe {
            libc::killpg(pid as libc::pid_t, libc::SIGKILL);
        }
    }
}

/// Kills the process group if the future is dropped before the child is reaped.
struct GroupGuard(Option<u32>);
impl Drop for GroupGuard {
    fn drop(&mut self) {
        kill_group(self.0.take());
    }
}

pub(crate) async fn run_cli(
    mut cmd: Command,
    stdin_text: Option<String>,
    parser: &mut dyn LineParser,
    tx: &UnboundedSender<Event>,
    cancel: &CancellationToken,
    timeout: Duration,
) -> Result<String, EngineError> {
    crate::paths::fill_env(cmd.as_std_mut());
    cmd.stdin(if stdin_text.is_some() { Stdio::piped() } else { Stdio::null() })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .process_group(0);
    let mut child = cmd.spawn().map_err(|e| EngineError::Cli(format!("could not start the CLI: {e}")))?;
    let mut guard = GroupGuard(child.id());

    if let Some(text) = stdin_text {
        let mut stdin = child.stdin.take().expect("stdin piped");
        tokio::spawn(async move {
            let _ = stdin.write_all(text.as_bytes()).await;
            let _ = stdin.shutdown().await;
        });
    }
    let mut stderr = child.stderr.take().expect("stderr piped");
    let err_task = tokio::spawn(async move {
        let mut s = String::new();
        let _ = stderr.read_to_string(&mut s).await;
        s
    });
    let mut lines = BufReader::new(child.stdout.take().expect("stdout piped")).lines();

    let work = async {
        while let Some(line) = lines.next_line().await.map_err(|e| EngineError::Cli(e.to_string()))? {
            for d in parser.feed(&line) {
                if !d.is_empty() {
                    let _ = tx.send(Event::Delta(d));
                }
            }
            if parser.done() {
                return Ok::<Option<bool>, EngineError>(None);
            }
        }
        let status = child.wait().await.map_err(|e| EngineError::Cli(e.to_string()))?;
        Ok(Some(status.success()))
    };

    let result = tokio::select! {
        r = work => r,
        _ = cancel.cancelled() => Err(EngineError::Cancelled),
        _ = tokio::time::sleep(timeout) => Err(EngineError::Timeout(timeout.as_secs())),
    };
    match result {
        Ok(None) => {
            // Answer complete: stop the CLI now instead of waiting for its shutdown.
            kill_group(guard.0.take());
            let _ = child.kill().await;
            parser.finish(true, "")
        }
        Ok(Some(ok)) => {
            guard.0 = None; // reaped; do not signal a pid that may be reused
            let stderr = tokio::time::timeout(Duration::from_secs(2), err_task).await.ok().and_then(|r| r.ok()).unwrap_or_default();
            parser.finish(ok, &stderr)
        }
        Err(e) => {
            kill_group(guard.0.take());
            let _ = child.kill().await;
            Err(e)
        }
    }
}

/// Short one-line form of a CLI error text for the UI.
pub(crate) fn short(s: &str) -> String {
    let s = s.trim();
    let line = s.lines().rev().find(|l| !l.trim().is_empty()).unwrap_or(s).trim();
    line.chars().take(300).collect()
}
