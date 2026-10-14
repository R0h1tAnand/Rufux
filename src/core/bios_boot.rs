//! Legacy BIOS boot support for ISO mode on MBR disks.
//!
//! Everything here is best effort: UEFI booting never depends on it, so a missing helper tool
//! downgrades to a warning instead of failing the whole write.

use std::fs::{self, OpenOptions};
use std::io::{Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use super::device::TargetDevice;
use super::exec;
use super::iso_scanner::{ImageType, IsoReport};
use super::layout::FileSystem;

/// Size of the boot-code area of an MBR; the partition table that follows must stay untouched.
const MBR_CODE_LEN: usize = 440;

const MBR_CANDIDATES: &[&str] = &[
    "/usr/lib/syslinux/mbr/mbr.bin",
    "/usr/lib/syslinux/bios/mbr.bin",
    "/usr/lib/SYSLINUX/mbr.bin",
    "/usr/share/syslinux/mbr.bin",
    "/usr/lib/syslinux/mbr.bin",
];

const MODULE_DIRS: &[&str] = &[
    "/usr/lib/syslinux/modules/bios",
    "/usr/share/syslinux",
    "/usr/lib/syslinux",
    "/usr/lib/SYSLINUX",
];

/// What has to happen to the partition once its contents are in place.
#[derive(Debug, PartialEq, Eq)]
pub enum BootStep {
    /// Install the Syslinux boot sector, loading its files from `directory` (e.g. `/syslinux`).
    Syslinux {
        directory: String,
    },
    /// Write a Windows (bootmgr-compatible) partition boot record.
    WindowsBootRecord,
    /// Write the FreeDOS FAT32 partition boot record.
    FreeDosBootRecord,
    None,
}

/// Writes generic MBR boot code (which chains to the active partition) without touching the
/// partition table. Returns false if no suitable boot code is installed on this machine.
pub fn install_mbr_code(target: &TargetDevice) -> Result<bool> {
    let Some(source) = MBR_CANDIDATES.iter().map(PathBuf::from).find(|p| p.is_file()) else {
        return Ok(false);
    };
    let code = fs::read(&source).with_context(|| format!("cannot read {}", source.display()))?;
    anyhow::ensure!(
        code.len() >= MBR_CODE_LEN,
        "{} is not an MBR boot code image",
        source.display()
    );

    let mut dev = OpenOptions::new().write(true).open(&target.path)?;
    dev.seek(SeekFrom::Start(0))?;
    dev.write_all(&code[..MBR_CODE_LEN])?;
    dev.sync_all()?;
    Ok(true)
}

fn find_module_dir() -> Option<PathBuf> {
    MODULE_DIRS
        .iter()
        .map(PathBuf::from)
        .find(|d| d.join("ldlinux.c32").is_file())
}

/// Prepares the mounted file system for BIOS booting and reports what is left to do once it has
/// been unmounted.
pub fn prepare_files(mount: &Path, report: Option<&IsoReport>, fs_kind: FileSystem) -> Result<BootStep> {
    let Some(report) = report else {
        return Ok(BootStep::None);
    };

    match report.image_type {
        ImageType::Windows if mount.join("bootmgr").is_file() => Ok(BootStep::WindowsBootRecord),
        ImageType::Linux if fs_kind == FileSystem::Fat32 => {
            let Some(dir) = ["isolinux", "syslinux", "boot/syslinux", "boot/isolinux"]
                .iter()
                .find(|d| mount.join(d).is_dir())
            else {
                return Ok(BootStep::None);
            };
            let dir_path = mount.join(dir);

            // Syslinux reads syslinux.cfg; ISO images ship isolinux.cfg with identical syntax.
            let syslinux_cfg = dir_path.join("syslinux.cfg");
            if !syslinux_cfg.exists() {
                if let Ok(cfg) = fs::read(dir_path.join("isolinux.cfg")) {
                    fs::write(&syslinux_cfg, cfg)?;
                }
            }

            // ldlinux.sys and ldlinux.c32 must come from the same Syslinux release.
            if let Some(modules) = find_module_dir() {
                fs::copy(modules.join("ldlinux.c32"), dir_path.join("ldlinux.c32"))
                    .context("cannot install a matching ldlinux.c32")?;
            }
            Ok(BootStep::Syslinux {
                directory: format!("/{}", dir),
            })
        }
        _ => Ok(BootStep::None),
    }
}

/// Configuration written to a drive that boots Syslinux with no operating system on it: a boot
/// prompt the user can extend with their own kernels.
const BARE_SYSLINUX_CFG: &str = "# Created by Rufux. Add your boot entries here; see https://wiki.syslinux.org/\n\
PROMPT 1\nTIMEOUT 100\n";

/// Prepares an otherwise empty FAT32 volume to boot Syslinux from its root directory.
pub fn prepare_bare_syslinux(mount: &Path) -> Result<BootStep> {
    let modules = find_module_dir()
        .context("the Syslinux modules (ldlinux.c32) were not found; install syslinux-common or syslinux")?;
    fs::copy(modules.join("ldlinux.c32"), mount.join("ldlinux.c32")).context("cannot install ldlinux.c32")?;
    fs::write(mount.join("syslinux.cfg"), BARE_SYSLINUX_CFG).context("cannot write syslinux.cfg")?;
    Ok(BootStep::Syslinux {
        directory: "/".to_string(),
    })
}

/// Completes BIOS boot setup on the unmounted partition. Returns a note to log when a helper tool
/// is missing.
pub fn finish(step: &BootStep, partition: &str, fs_kind: FileSystem) -> Result<Option<String>> {
    match step {
        BootStep::None => Ok(None),
        BootStep::Syslinux { directory } => match exec::find_tool("syslinux") {
            Some(tool) => {
                // Syslinux installs into the root directory when no --directory is given.
                if directory == "/" {
                    exec::run_at(&tool, ["--install", partition])?;
                } else {
                    exec::run_at(&tool, ["--install", "--directory", directory.as_str(), partition])?;
                }
                Ok(None)
            }
            None => Ok(Some(
                "syslinux is not installed; the drive will boot in UEFI mode only".into(),
            )),
        },
        BootStep::FreeDosBootRecord => {
            let tool = exec::find_tool("ms-sys").context("ms-sys is required to make a FreeDOS drive bootable")?;
            exec::run_at(&tool, ["--fat32free", partition])?;
            Ok(None)
        }
        BootStep::WindowsBootRecord => match exec::find_tool("ms-sys") {
            Some(tool) => {
                let flag = if fs_kind == FileSystem::Ntfs {
                    "--ntfs"
                } else {
                    "--fat32nt"
                };
                exec::run_at(&tool, [flag, partition])?;
                Ok(None)
            }
            None => Ok(Some(
                "ms-sys is not installed; Windows media will boot in UEFI mode only".into(),
            )),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bare_syslinux_config_has_a_boot_prompt() {
        assert!(BARE_SYSLINUX_CFG.contains("PROMPT 1"));
        assert!(BARE_SYSLINUX_CFG.ends_with('\n'));
    }
}
