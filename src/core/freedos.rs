//! FreeDOS boot media.
//!
//! Copies an embedded FreeDOS 1.4 system (kernel, command interpreter, keyboard layouts and code
//! pages) onto a FAT32 volume. The boot sector is written afterwards by `ms-sys`
//! (see [`super::bios_boot`]). Provenance and the kernel patch are described in
//! `assets/freedos/readme.txt`.

use std::fs;
use std::path::Path;

use anyhow::{Context, Result};

macro_rules! freedos_files {
    ($($name:literal),* $(,)?) => {
        &[$(($name, include_bytes!(concat!("../../assets/freedos/", $name)))),*]
    };
}

/// The boot sector looks the kernel up by name in the root directory, so it is written first.
const FILES: &[(&str, &[u8])] = freedos_files![
    "KERNEL.SYS",
    "COMMAND.COM",
    "DISPLAY.EXE",
    "EGA.CPX",
    "EGA10.CPX",
    "EGA11.CPX",
    "EGA12.CPX",
    "EGA13.CPX",
    "EGA14.CPX",
    "EGA15.CPX",
    "EGA16.CPX",
    "EGA17.CPX",
    "EGA18.CPX",
    "EGA2.CPX",
    "EGA3.CPX",
    "EGA4.CPX",
    "EGA5.CPX",
    "EGA6.CPX",
    "EGA7.CPX",
    "EGA8.CPX",
    "EGA9.CPX",
    "KEYB.EXE",
    "KEYBOARD.SYS",
    "KEYBRD2.SYS",
    "KEYBRD3.SYS",
    "KEYBRD4.SYS",
    "MODE.COM",
];

/// Number of files [`install`] writes.
pub const FILE_COUNT: usize = FILES.len();

pub fn install(root: &Path) -> Result<()> {
    for (name, data) in FILES {
        fs::write(root.join(name), data).with_context(|| format!("cannot write {name}"))?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kernel_comes_first_and_everything_is_non_empty() {
        assert_eq!(FILES[0].0, "KERNEL.SYS");
        assert!(FILES.iter().all(|(_, d)| !d.is_empty()));
        assert!(FILES.iter().any(|(n, _)| *n == "COMMAND.COM"));
    }

    #[test]
    fn installs_every_file() {
        let dir = tempfile::tempdir().unwrap();
        install(dir.path()).unwrap();
        assert_eq!(fs::read_dir(dir.path()).unwrap().count(), FILE_COUNT);
        assert_eq!(
            fs::read(dir.path().join("COMMAND.COM")).unwrap(),
            include_bytes!("../../assets/freedos/COMMAND.COM")
        );
    }
}
