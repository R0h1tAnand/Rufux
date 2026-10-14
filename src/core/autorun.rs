//! Extended label and icon files.
//!
//! Windows file managers read `autorun.inf` for the label and icon shown for a removable volume.
//! Linux ignores it, so this only helps when the drive is plugged into a Windows machine.

use std::fs;
use std::path::Path;

use anyhow::{Context, Result};

/// The icon that is copied to the drive as `autorun.ico`.
const ICON: &[u8] = include_bytes!("../../assets/icons/rufus.ico");

fn autorun_inf(label: &str) -> String {
    // The label never contains control characters (see `sanitize_label`), so it cannot inject
    // extra keys; strip anyway because this is written verbatim.
    let label: String = label.chars().filter(|c| !c.is_control()).collect();
    format!("[autorun]\r\nlabel={label}\r\nicon=autorun.ico\r\n")
}

/// Writes `autorun.inf` and `autorun.ico` to the root of the mounted volume.
pub fn write_extended_label(root: &Path, label: &str) -> Result<()> {
    fs::write(root.join("autorun.ico"), ICON).context("cannot write autorun.ico")?;
    fs::write(root.join("autorun.inf"), autorun_inf(label)).context("cannot write autorun.inf")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inf_has_label_and_icon_with_crlf() {
        assert_eq!(
            autorun_inf("MY DRIVE"),
            "[autorun]\r\nlabel=MY DRIVE\r\nicon=autorun.ico\r\n"
        );
    }

    #[test]
    fn label_cannot_inject_keys() {
        let inf = autorun_inf("A\r\nopen=evil.exe");
        assert_eq!(inf.lines().count(), 3);
        assert!(!inf.contains("\nopen="));
    }

    #[test]
    fn writes_both_files() {
        let dir = tempfile::tempdir().unwrap();
        write_extended_label(dir.path(), "RUFUX").unwrap();
        assert_eq!(fs::read(dir.path().join("autorun.ico")).unwrap(), ICON);
        assert!(fs::read_to_string(dir.path().join("autorun.inf"))
            .unwrap()
            .contains("label=RUFUX"));
    }
}
