//! Privilege helpers shared by the GUI and the elevated worker.

use std::env;
use std::io;

use nix::unistd::{setfsuid, Uid};

/// Returns true if currently running with root privileges (UID 0).
pub fn is_root() -> bool {
    Uid::current().is_root()
}

/// UID of the desktop user on whose behalf `pkexec` started this process, if any.
///
/// `pkexec` sets `PKEXEC_UID` itself; the value is only trusted while running as root.
pub fn invoking_uid() -> Option<Uid> {
    if !is_root() {
        return None;
    }
    env::var("PKEXEC_UID").ok()?.parse::<u32>().ok().map(Uid::from_raw)
}

/// Runs `f` with file-system permission checks performed as the invoking desktop user.
///
/// The elevated worker uses this to open files the user picked (images, dump targets), so root
/// can never be tricked into reading or overwriting a file the user could not touch themselves.
/// Outside of `pkexec` this simply calls `f`.
pub fn with_invoking_user<T>(f: impl FnOnce() -> io::Result<T>) -> io::Result<T> {
    match invoking_uid() {
        Some(uid) if !uid.is_root() => {
            let previous = setfsuid(uid);
            let result = f();
            setfsuid(previous);
            result
        }
        _ => f(),
    }
}
