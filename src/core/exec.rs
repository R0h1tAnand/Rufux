//! Hardened external command execution.
//!
//! Every helper tool (`parted`, `mkfs.*`, `mount`, `7z`, ...) is resolved against a fixed,
//! root-owned search path and executed with a minimal environment, so that the privileged worker
//! never runs a binary picked up from the caller's `PATH`. Failures carry the tool's stderr.

use std::error::Error;
use std::ffi::OsStr;
use std::fmt;
use std::io::Read;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;

use anyhow::{anyhow, bail, Context, Result};

/// Directories searched for helper tools. Only system locations, never the caller's `PATH`.
pub const SAFE_PATH: &str = "/usr/local/sbin:/usr/local/bin:/usr/sbin:/usr/bin:/sbin:/bin";

/// Maximum amount of a tool's stderr kept for error reporting.
const STDERR_CAP: usize = 64 * 1024;

/// Error returned when an operation was aborted by the user.
#[derive(Debug, Clone, Copy)]
pub struct Canceled;

impl fmt::Display for Canceled {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("operation canceled by user")
    }
}

impl Error for Canceled {}

/// Returns an `Err(Canceled)` when the cancellation flag has been raised.
pub fn check_cancel(flag: &AtomicBool) -> Result<()> {
    if flag.load(Ordering::Relaxed) {
        Err(Canceled.into())
    } else {
        Ok(())
    }
}

fn is_executable(path: &Path) -> bool {
    path.metadata()
        .map(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
        .unwrap_or(false)
}

/// Locates a tool in [`SAFE_PATH`].
pub fn find_tool(name: &str) -> Option<PathBuf> {
    SAFE_PATH
        .split(':')
        .map(|dir| Path::new(dir).join(name))
        .find(|candidate| is_executable(candidate))
}

/// Locates the first available tool out of several alternative names.
pub fn find_any_tool(names: &[&str]) -> Option<PathBuf> {
    names.iter().find_map(|name| find_tool(name))
}

/// Fails with a readable list when any of the named tools is missing.
pub fn require_tools(names: &[&str]) -> Result<()> {
    let missing: Vec<&str> = names.iter().copied().filter(|n| find_tool(n).is_none()).collect();
    if missing.is_empty() {
        Ok(())
    } else {
        bail!("required tool(s) not installed: {}", missing.join(", "))
    }
}

fn command<S: AsRef<OsStr>>(exe: &Path, args: impl IntoIterator<Item = S>) -> Command {
    let mut cmd = Command::new(exe);
    cmd.args(args)
        .env_clear()
        .env("PATH", SAFE_PATH)
        .env("LC_ALL", "C")
        .stdin(Stdio::null());
    cmd
}

fn tool_name(exe: &Path) -> String {
    exe.file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

fn failure(exe: &Path, status: std::process::ExitStatus, stderr: &[u8]) -> anyhow::Error {
    let text = String::from_utf8_lossy(stderr);
    let text = text.trim();
    if text.is_empty() {
        anyhow!("{} failed ({})", tool_name(exe), status)
    } else {
        anyhow!("{} failed ({}): {}", tool_name(exe), status, text)
    }
}

/// Runs a tool to completion and returns its standard output; a non-zero exit is an error.
pub fn run<S: AsRef<OsStr>>(program: &str, args: impl IntoIterator<Item = S>) -> Result<String> {
    let exe = find_tool(program).ok_or_else(|| anyhow!("required tool '{}' is not installed", program))?;
    run_at(&exe, args)
}

/// Like [`run`], for a tool that was already resolved with [`find_tool`].
pub fn run_at<S: AsRef<OsStr>>(exe: &Path, args: impl IntoIterator<Item = S>) -> Result<String> {
    let output = command(exe, args)
        .output()
        .with_context(|| format!("failed to start {}", tool_name(exe)))?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    } else {
        Err(failure(exe, output.status, &output.stderr))
    }
}

/// Reads a stream to the end, keeping at most [`STDERR_CAP`] bytes.
pub fn drain_capped(mut stream: impl Read) -> Vec<u8> {
    let mut kept = Vec::new();
    let mut chunk = [0u8; 4096];
    while let Ok(n) = stream.read(&mut chunk) {
        if n == 0 {
            break;
        }
        let room = STDERR_CAP.saturating_sub(kept.len());
        kept.extend_from_slice(&chunk[..n.min(room)]);
    }
    kept
}

/// Runs a tool, handing every `\n`/`\r` terminated line of its stdout to `on_line` as it appears.
///
/// The child is killed and [`Canceled`] returned as soon as `cancel` is raised and the tool
/// produces further output.
pub fn run_streaming<S: AsRef<OsStr>>(
    exe: &Path,
    args: impl IntoIterator<Item = S>,
    cancel: &AtomicBool,
    mut on_line: impl FnMut(&str),
) -> Result<()> {
    let mut child = command(exe, args)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| format!("failed to start {}", tool_name(exe)))?;

    let stderr = child.stderr.take().map(|s| thread::spawn(move || drain_capped(s)));
    let mut stdout = child.stdout.take().ok_or_else(|| anyhow!("stdout was not captured"))?;

    let mut buf = [0u8; 4096];
    let mut line = Vec::new();
    let mut outcome: Result<()> = Ok(());

    loop {
        if let Err(e) = check_cancel(cancel) {
            outcome = Err(e);
            break;
        }
        match stdout.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                for &byte in &buf[..n] {
                    if byte == b'\n' || byte == b'\r' {
                        if !line.is_empty() {
                            on_line(&String::from_utf8_lossy(&line));
                            line.clear();
                        }
                    } else if line.len() < 4096 {
                        line.push(byte);
                    }
                }
            }
            Err(e) => {
                outcome = Err(e.into());
                break;
            }
        }
    }

    if outcome.is_err() {
        let _ = child.kill();
    }
    let status = child.wait()?;
    let stderr = stderr.and_then(|h| h.join().ok()).unwrap_or_default();
    outcome?;

    if status.success() {
        Ok(())
    } else {
        Err(failure(exe, status, &stderr))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_standard_tools_only_in_safe_path() {
        let sh = find_tool("sh").expect("sh must exist");
        assert!(sh.is_absolute());
        assert!(find_tool("definitely-not-a-real-tool").is_none());
    }

    #[test]
    fn reports_stderr_on_failure() {
        let err = run("sh", ["-c", "echo boom >&2; exit 3"]).unwrap_err().to_string();
        assert!(err.contains("boom"), "{err}");
    }

    #[test]
    fn streams_lines_and_honours_cancel() {
        let sh = find_tool("sh").unwrap();
        let cancel = AtomicBool::new(false);
        let mut seen = Vec::new();
        run_streaming(&sh, ["-c", "printf '10%%\\r20%%\\n'"], &cancel, |l| {
            seen.push(l.to_string())
        })
        .unwrap();
        assert_eq!(seen, ["10%", "20%"]);

        cancel.store(true, Ordering::Relaxed);
        let err = run_streaming(&sh, ["-c", "exec sleep 5"], &cancel, |_| {}).unwrap_err();
        assert!(err.is::<Canceled>());
    }
}
