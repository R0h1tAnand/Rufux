//! Privilege Elevation Engine via Polkit / pkexec
//!
//! Provides seamless root privilege escalation so desktop users get the native
//! graphical authentication prompt without ever having to touch a terminal.

use std::env;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use nix::unistd::Uid;

/// Returns true if currently running with root privileges (UID 0)
pub fn is_root() -> bool {
    Uid::current().is_root()
}

/// Retrieves the path to the currently running executable
pub fn current_exe_path() -> anyhow::Result<PathBuf> {
    Ok(env::current_exe()?)
}

/// Spawns an elevated worker process using pkexec, connecting stdout/stderr for IPC
pub fn spawn_elevated_worker(args: &[&str]) -> anyhow::Result<Child> {
    let exe = current_exe_path()?;

    let mut cmd = Command::new("pkexec");
    cmd.arg(&exe);
    for arg in args {
        cmd.arg(arg);
    }

    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());

    let child = cmd.spawn()?;
    Ok(child)
}

/// Runs a command with pkexec if not root, or directly if root
pub fn run_elevated_command(cmd_name: &str, args: &[&str]) -> anyhow::Result<std::process::Output> {
    if is_root() {
        let output = Command::new(cmd_name).args(args).output()?;
        Ok(output)
    } else {
        let mut cmd = Command::new("pkexec");
        cmd.arg(cmd_name);
        for arg in args {
            cmd.arg(arg);
        }
        let output = cmd.output()?;
        Ok(output)
    }
}
