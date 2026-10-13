//! Privilege Elevation Engine via Polkit / pkexec
//!
//! The GUI never touches a disk itself. Destructive work is described by a [`WorkerRequest`],
//! sent as one JSON line to `pkexec rufux --worker`, and progress comes back as JSON lines on
//! the worker's stdout. Closing the worker's stdin, or sending the line `cancel`, aborts the job.

use std::env;
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;

use anyhow::{anyhow, Context, Result};
use serde::{Deserialize, Serialize};

use super::exec::{drain_capped, find_tool};
pub use super::privilege::is_root;
use super::writer::{BurnOptions, BurnProgress};

/// Work the privileged worker can be asked to perform.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum WorkerRequest {
    /// Write or format a drive.
    Burn(BurnOptions),
    /// Copy a drive into an image file owned by the invoking user.
    Dump { device: PathBuf, output: PathBuf },
}

/// `pkexec` exit status when the authentication dialog was dismissed.
const PKEXEC_DISMISSED: i32 = 126;
/// `pkexec` exit status when authorization failed or no agent is available.
const PKEXEC_NOT_AUTHORIZED: i32 = 127;

/// Relays the worker's JSON lines; anything that is not valid progress becomes a log entry.
/// Returns whether a terminal (`Finished`/`Error`) message was seen.
fn forward_progress(reader: impl BufRead, on_progress: &mut dyn FnMut(BurnProgress)) -> bool {
    let mut terminal_seen = false;
    for line in reader.lines().map_while(|l| l.ok()) {
        match serde_json::from_str::<BurnProgress>(&line) {
            Ok(progress) => {
                terminal_seen |= matches!(progress, BurnProgress::Finished | BurnProgress::Error(_));
                on_progress(progress);
            }
            Err(_) => on_progress(BurnProgress::Log(line)),
        }
    }
    terminal_seen
}

/// Runs a request in the elevated worker, forwarding its progress to `on_progress`.
///
/// Always ends with exactly one [`BurnProgress::Finished`] or [`BurnProgress::Error`].
pub fn run_job(request: &WorkerRequest, cancel: &AtomicBool, mut on_progress: impl FnMut(BurnProgress)) {
    if let Err(e) = run_job_inner(request, cancel, &mut on_progress) {
        on_progress(BurnProgress::Error(format!("{:#}", e)));
    }
}

fn run_job_inner(
    request: &WorkerRequest,
    cancel: &AtomicBool,
    on_progress: &mut dyn FnMut(BurnProgress),
) -> Result<()> {
    let exe = env::current_exe().context("cannot locate the Rufux executable")?;

    let mut cmd = if is_root() {
        Command::new(&exe)
    } else {
        let pkexec = find_tool("pkexec")
            .ok_or_else(|| anyhow!("pkexec (polkit) is not installed; it is required to write to drives"))?;
        let mut c = Command::new(pkexec);
        c.arg(&exe);
        c
    };
    cmd.arg("--worker")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let mut child = cmd.spawn().context("cannot start the privileged worker")?;
    let mut stdin = child.stdin.take().ok_or_else(|| anyhow!("worker stdin unavailable"))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| anyhow!("worker stdout unavailable"))?;
    let stderr = child
        .stderr
        .take()
        .ok_or_else(|| anyhow!("worker stderr unavailable"))?;

    writeln!(stdin, "{}", serde_json::to_string(request)?).context("cannot send the request to the worker")?;
    stdin.flush()?;

    let done = AtomicBool::new(false);
    let mut terminal_seen = false;

    let stderr_text = thread::scope(|scope| {
        let err_reader = scope.spawn(move || String::from_utf8_lossy(&drain_capped(stderr)).trim().to_string());

        // Keeps the worker's stdin open for the whole job; cancelling (or the GUI dying, which
        // closes the pipe) makes the worker stop.
        let done = &done;
        scope.spawn(move || {
            while !done.load(Ordering::Relaxed) {
                if cancel.load(Ordering::Relaxed) {
                    let _ = writeln!(stdin, "cancel");
                    let _ = stdin.flush();
                    break;
                }
                thread::sleep(Duration::from_millis(100));
            }
            drop(stdin);
        });

        terminal_seen = forward_progress(BufReader::new(stdout), on_progress);
        done.store(true, Ordering::Relaxed);
        err_reader.join().unwrap_or_default()
    });

    let status = child.wait()?;
    if !terminal_seen {
        let message = match status.code() {
            Some(PKEXEC_DISMISSED) => "Authentication was cancelled; nothing was changed".to_string(),
            Some(PKEXEC_NOT_AUTHORIZED) => {
                "Authorization failed (wrong password, or no Polkit authentication agent is running)".to_string()
            }
            _ if stderr_text.is_empty() => format!("the privileged worker stopped unexpectedly ({})", status),
            _ => format!(
                "the privileged worker stopped unexpectedly ({}): {}",
                status, stderr_text
            ),
        };
        on_progress(BurnProgress::Error(message));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn worker_output_is_relayed_and_terminal_state_detected() {
        let input =
            "{\"Status\":\"Working\"}\nstray text\n{\"Progress\":{\"percent\":0.5,\"speed_mb_s\":2.0}}\n\"Finished\"\n";
        let mut seen = Vec::new();
        let terminal = forward_progress(input.as_bytes(), &mut |p| seen.push(p));
        assert!(terminal);
        assert!(matches!(seen[0], BurnProgress::Status(_)));
        assert!(matches!(&seen[1], BurnProgress::Log(l) if l == "stray text"));
        assert!(matches!(seen[3], BurnProgress::Finished));

        let mut none = Vec::new();
        assert!(!forward_progress("{\"Status\":\"x\"}\n".as_bytes(), &mut |p| none.push(p)));
    }

    #[test]
    fn requests_survive_the_pipe() {
        let req = WorkerRequest::Dump {
            device: "/dev/sdb".into(),
            output: "/tmp/a.img".into(),
        };
        let line = serde_json::to_string(&req).unwrap();
        assert!(matches!(
            serde_json::from_str::<WorkerRequest>(&line).unwrap(),
            WorkerRequest::Dump { .. }
        ));
    }
}
