//! Run one CLI call: write stdin, read stdout line by line, stream deltas, obey cancel and timeout.
//! The child gets its own process group (Windows: a job object), and cancel or timeout kills the whole group.

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

#[cfg(unix)]
mod group {
    /// The child's process group (its pid, because of `process_group(0)`).
    pub(super) struct Group(u32);

    pub(super) fn prepare(cmd: &mut tokio::process::Command) {
        cmd.process_group(0);
    }

    pub(super) fn attach(child: &tokio::process::Child) -> Option<Group> {
        child.id().map(Group)
    }

    pub(super) fn kill(g: Group) {
        // SAFETY: plain syscall; the group id is the child's pid because of process_group(0).
        unsafe {
            libc::killpg(g.0 as libc::pid_t, libc::SIGKILL);
        }
    }
}

#[cfg(windows)]
mod group {
    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
    use windows_sys::Win32::System::JobObjects::{AssignProcessToJobObject, CreateJobObjectW, TerminateJobObject};

    /// A job object holding the child; processes it starts join the job too, so the whole tree can be ended.
    pub(super) struct Group(HANDLE);
    // SAFETY: a job handle may be used and closed from any thread.
    unsafe impl Send for Group {}
    unsafe impl Sync for Group {}

    impl Drop for Group {
        fn drop(&mut self) {
            // SAFETY: our own handle, closed once. Closing does not end the processes.
            unsafe {
                CloseHandle(self.0);
            }
        }
    }

    pub(super) fn prepare(_cmd: &mut tokio::process::Command) {}

    pub(super) fn attach(child: &tokio::process::Child) -> Option<Group> {
        let proc_handle = child.raw_handle()? as HANDLE;
        // SAFETY: plain calls on a new job handle and the live child's handle.
        unsafe {
            let job = CreateJobObjectW(std::ptr::null(), std::ptr::null());
            if job.is_null() {
                return None;
            }
            let g = Group(job);
            (AssignProcessToJobObject(g.0, proc_handle) != 0).then_some(g)
        }
    }

    pub(super) fn kill(g: Group) {
        // SAFETY: our own live job handle.
        unsafe {
            TerminateJobObject(g.0, 1);
        }
    }
}

fn kill_group(g: Option<group::Group>) {
    if let Some(g) = g {
        group::kill(g);
    }
}

/// Kills the process group if the future is dropped before the child is reaped.
struct GroupGuard(Option<group::Group>);
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
        .kill_on_drop(true);
    group::prepare(&mut cmd);
    let mut child = cmd.spawn().map_err(|e| EngineError::Cli(format!("could not start the CLI: {e}")))?;
    let mut guard = GroupGuard(group::attach(&child));

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
