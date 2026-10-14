//! Partition schemes, file systems and the pure planning logic that depends on them.
//!
//! Everything here is free of I/O so it can be unit-tested without touching a device.

use std::fmt;
use std::str::FromStr;

use anyhow::{bail, ensure, Result};
use serde::{Deserialize, Serialize};

pub const MIB: u64 = 1024 * 1024;

/// Size of the UEFI:NTFS helper partition (it holds the 1 MiB `uefi-ntfs.img`).
pub const UEFI_NTFS_PART_MIB: u64 = 1;

/// Smallest device (in MiB) Rufux will partition.
const MIN_DEVICE_MIB: u64 = 16;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PartitionScheme {
    Mbr,
    Gpt,
}

impl fmt::Display for PartitionScheme {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Mbr => "MBR",
            Self::Gpt => "GPT",
        })
    }
}

impl FromStr for PartitionScheme {
    type Err = anyhow::Error;
    fn from_str(s: &str) -> Result<Self> {
        match s.to_ascii_lowercase().as_str() {
            "mbr" | "msdos" => Ok(Self::Mbr),
            "gpt" => Ok(Self::Gpt),
            other => bail!("unknown partition scheme '{}' (expected MBR or GPT)", other),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TargetSystem {
    /// "BIOS (or UEFI-CSM)"
    BiosOrUefi,
    /// "UEFI (non CSM)"
    Uefi,
}

impl TargetSystem {
    pub fn default_for(scheme: PartitionScheme) -> Self {
        match scheme {
            PartitionScheme::Mbr => Self::BiosOrUefi,
            PartitionScheme::Gpt => Self::Uefi,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum FileSystem {
    Fat32,
    Ntfs,
    ExFat,
    Ext4,
}

impl fmt::Display for FileSystem {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Fat32 => "FAT32",
            Self::Ntfs => "NTFS",
            Self::ExFat => "exFAT",
            Self::Ext4 => "ext4",
        })
    }
}

impl FromStr for FileSystem {
    type Err = anyhow::Error;
    fn from_str(s: &str) -> Result<Self> {
        match s.to_ascii_lowercase().as_str() {
            "fat32" | "fat" | "vfat" => Ok(Self::Fat32),
            "ntfs" => Ok(Self::Ntfs),
            "exfat" => Ok(Self::ExFat),
            "ext4" => Ok(Self::Ext4),
            other => bail!("unknown file system '{}' (expected FAT32, NTFS, exFAT or ext4)", other),
        }
    }
}

impl FileSystem {
    /// Partition type hint given to `parted` (decides the MBR type byte).
    pub fn parted_hint(self) -> &'static str {
        match self {
            Self::Fat32 => "fat32",
            Self::Ntfs | Self::ExFat => "ntfs",
            Self::Ext4 => "ext4",
        }
    }

    pub fn mkfs_tool(self) -> &'static str {
        match self {
            Self::Fat32 => "mkfs.vfat",
            Self::Ntfs => "mkfs.ntfs",
            Self::ExFat => "mkfs.exfat",
            Self::Ext4 => "mkfs.ext4",
        }
    }
}

/// Produces a label the target file system accepts that can never be mistaken for an option.
pub fn sanitize_label(fs: FileSystem, label: &str) -> String {
    const FAT_FORBIDDEN: &str = "\"*+,./:;<=>?[\\]|";
    let ascii_only = matches!(fs, FileSystem::Fat32 | FileSystem::Ext4);

    let mut out = String::new();
    for c in label.trim().chars() {
        let c = match c {
            c if c.is_control() => continue,
            c if ascii_only && !c.is_ascii() => '_',
            c if fs == FileSystem::Fat32 && FAT_FORBIDDEN.contains(c) => '_',
            c if fs == FileSystem::Fat32 => c.to_ascii_uppercase(),
            c => c,
        };
        let fits = match fs {
            FileSystem::Fat32 | FileSystem::ExFat => out.chars().count() < 11,
            FileSystem::Ext4 => out.len() < 16,
            FileSystem::Ntfs => out.chars().count() < 32,
        };
        if !fits {
            break;
        }
        out.push(c);
    }

    // A leading '-' would be parsed as an option by mkfs.
    let out = out.trim_start_matches('-').trim().to_string();
    if out.is_empty() {
        "RUFUX".to_string()
    } else {
        out
    }
}

/// Command-line arguments for the `mkfs` tool (program name excluded, device path last).
pub fn mkfs_args(fs: FileSystem, label: &str, cluster_bytes: Option<u32>, quick: bool, device: &str) -> Vec<String> {
    let label = sanitize_label(fs, label);
    let mut args: Vec<String> = Vec::new();
    match fs {
        FileSystem::Fat32 => {
            args.extend(["-F".into(), "32".into(), "-n".into(), label]);
            if let Some(c) = cluster_bytes.filter(|c| c.is_power_of_two() && (512..=65536).contains(c)) {
                args.extend(["-s".into(), (c / 512).to_string()]);
            }
        }
        FileSystem::Ntfs => {
            if quick {
                args.push("-Q".into());
            }
            args.extend(["-L".into(), label]);
            if let Some(c) = cluster_bytes.filter(|c| c.is_power_of_two() && (512..=65536).contains(c)) {
                args.extend(["-c".into(), c.to_string()]);
            }
        }
        FileSystem::ExFat => {
            // exfatprogs syntax; the legacy exfat-utils spelling (-n) is handled by the caller.
            args.extend(["-L".into(), label]);
            if let Some(c) = cluster_bytes.filter(|c| c.is_power_of_two() && (512..=33_554_432).contains(c)) {
                args.extend(["-c".into(), c.to_string()]);
            }
        }
        FileSystem::Ext4 => {
            args.extend(["-F".into(), "-L".into(), label]);
            if let Some(c) = cluster_bytes.filter(|c| matches!(c, 1024 | 2048 | 4096)) {
                args.extend(["-b".into(), c.to_string()]);
            }
        }
    }
    args.push(device.to_string());
    args
}

/// Which partitions the ISO mode should create.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Layout {
    /// One data partition spanning the device.
    Single,
    /// Data partition plus a 1 MiB UEFI:NTFS helper partition at the end.
    UefiNtfs,
    /// Data partition plus an ext4 persistence partition of the given size.
    Persistence { size_mib: u64 },
}

/// A partition expressed in MiB: `[start_mib, end_mib)`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PartitionSpec {
    pub start_mib: u64,
    pub end_mib: u64,
}

impl PartitionSpec {
    pub fn size_mib(&self) -> u64 {
        self.end_mib - self.start_mib
    }
}

/// Computes aligned partition boundaries for a device.
///
/// The first MiB is left free for the partition table and boot code; GPT additionally keeps the
/// last MiB free for the backup table.
pub fn plan_partitions(disk_bytes: u64, scheme: PartitionScheme, layout: Layout) -> Result<Vec<PartitionSpec>> {
    let total_mib = disk_bytes / MIB;
    ensure!(total_mib >= MIN_DEVICE_MIB, "device is too small ({} MiB)", total_mib);

    let usable_end = total_mib - if scheme == PartitionScheme::Gpt { 1 } else { 0 };
    let data_start = 1;

    let parts = match layout {
        Layout::Single => vec![PartitionSpec {
            start_mib: data_start,
            end_mib: usable_end,
        }],
        Layout::UefiNtfs => {
            let split = usable_end - UEFI_NTFS_PART_MIB;
            vec![
                PartitionSpec {
                    start_mib: data_start,
                    end_mib: split,
                },
                PartitionSpec {
                    start_mib: split,
                    end_mib: usable_end,
                },
            ]
        }
        Layout::Persistence { size_mib } => {
            ensure!(size_mib >= 8, "persistence partition must be at least 8 MiB");
            ensure!(
                usable_end > data_start + size_mib + 8,
                "persistence size ({} MiB) leaves no room for the boot partition",
                size_mib
            );
            let split = usable_end - size_mib;
            vec![
                PartitionSpec {
                    start_mib: data_start,
                    end_mib: split,
                },
                PartitionSpec {
                    start_mib: split,
                    end_mib: usable_end,
                },
            ]
        }
    };

    ensure!(parts[0].size_mib() >= 8, "main partition would be smaller than 8 MiB");
    Ok(parts)
}

/// First sector used for the data partition by the old-BIOS fix: the classic CHS-aligned start
/// (one track of 63 sectors) that legacy BIOSes and DOS-era boot code expect.
pub const LEGACY_FIRST_SECTOR: u64 = 63;

/// Arguments for `parted` (program name excluded) that create the planned partition table.
///
/// With `legacy_bios` on an MBR disk the first partition starts at sector 63 instead of 1 MiB.
pub fn parted_args(
    device: &str,
    scheme: PartitionScheme,
    fs: FileSystem,
    layout: Layout,
    plan: &[PartitionSpec],
    legacy_bios: bool,
) -> Vec<String> {
    let legacy = legacy_bios && scheme == PartitionScheme::Mbr;
    let label_type = if scheme == PartitionScheme::Gpt { "gpt" } else { "msdos" };

    let mut args: Vec<String> = vec!["-s".into()];
    if legacy {
        // Without this parted would round the 63 s start up to a MiB boundary.
        args.extend(["-a".into(), "none".into()]);
    }
    args.extend([device.to_string(), "mklabel".into(), label_type.into()]);

    for (i, part) in plan.iter().enumerate() {
        let hint = match (i, layout) {
            (0, _) => fs.parted_hint(),
            (_, Layout::UefiNtfs) => "fat16",
            _ => "ext4",
        };
        args.extend(["mkpart", "primary", hint].map(String::from));
        args.push(if legacy && i == 0 {
            format!("{LEGACY_FIRST_SECTOR}s")
        } else {
            format!("{}MiB", part.start_mib)
        });
        args.push(format!("{}MiB", part.end_mib));
    }
    if scheme == PartitionScheme::Mbr {
        args.extend(["set", "1", "boot", "on"].map(String::from));
    }
    if layout == Layout::UefiNtfs {
        args.extend(["set", "2", "esp", "on"].map(String::from));
    }
    args
}

/// Path of the n-th partition node of a whole-disk device (`sdb` -> `sdb1`, `nvme0n1` -> `nvme0n1p1`).
pub fn partition_path(disk: &str, index: u32) -> String {
    if disk.ends_with(|c: char| c.is_ascii_digit()) {
        format!("{}p{}", disk, index)
    } else {
        format!("{}{}", disk, index)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const GIB: u64 = 1024 * MIB;

    #[test]
    fn single_partition_math() {
        let p = plan_partitions(8 * GIB, PartitionScheme::Mbr, Layout::Single).unwrap();
        assert_eq!(
            p,
            [PartitionSpec {
                start_mib: 1,
                end_mib: 8192
            }]
        );
        let g = plan_partitions(8 * GIB, PartitionScheme::Gpt, Layout::Single).unwrap();
        assert_eq!(g[0].end_mib, 8191, "GPT keeps the last MiB for the backup table");
    }

    #[test]
    fn uefi_ntfs_partitions_are_adjacent_and_exactly_one_mib() {
        let p = plan_partitions(16 * GIB, PartitionScheme::Gpt, Layout::UefiNtfs).unwrap();
        assert_eq!(p.len(), 2);
        assert_eq!(p[0].end_mib, p[1].start_mib);
        assert_eq!(p[1].size_mib(), UEFI_NTFS_PART_MIB);
        assert_eq!(p[1].end_mib, 16 * 1024 - 1);
    }

    #[test]
    fn persistence_layout_and_limits() {
        let p = plan_partitions(8 * GIB, PartitionScheme::Mbr, Layout::Persistence { size_mib: 2048 }).unwrap();
        assert_eq!(p[1].size_mib(), 2048);
        assert_eq!(p[0].end_mib, p[1].start_mib);
        assert!(plan_partitions(GIB, PartitionScheme::Mbr, Layout::Persistence { size_mib: 1020 }).is_err());
        assert!(plan_partitions(8 * MIB, PartitionScheme::Mbr, Layout::Single).is_err());
    }

    #[test]
    fn parted_arguments_are_mib_aligned_by_default() {
        let plan = plan_partitions(8 * GIB, PartitionScheme::Mbr, Layout::Single).unwrap();
        let a = parted_args(
            "/dev/sdb",
            PartitionScheme::Mbr,
            FileSystem::Fat32,
            Layout::Single,
            &plan,
            false,
        );
        assert_eq!(
            a,
            [
                "-s", "/dev/sdb", "mklabel", "msdos", "mkpart", "primary", "fat32", "1MiB", "8192MiB", "set", "1",
                "boot", "on"
            ]
        );
    }

    #[test]
    fn legacy_fix_starts_the_first_partition_at_sector_63() {
        let plan = plan_partitions(8 * GIB, PartitionScheme::Mbr, Layout::UefiNtfs).unwrap();
        let a = parted_args(
            "/dev/sdb",
            PartitionScheme::Mbr,
            FileSystem::Ntfs,
            Layout::UefiNtfs,
            &plan,
            true,
        );
        assert_eq!(&a[..3], ["-s", "-a", "none"]);
        assert!(a.windows(2).any(|w| w == ["ntfs", "63s"]));
        // Only the first partition moves; the helper partition keeps its MiB boundary.
        assert!(a.windows(2).any(|w| w == ["fat16", "8191MiB"]));
    }

    #[test]
    fn legacy_fix_is_ignored_on_gpt() {
        let plan = plan_partitions(8 * GIB, PartitionScheme::Gpt, Layout::Single).unwrap();
        let a = parted_args(
            "/dev/sdb",
            PartitionScheme::Gpt,
            FileSystem::Fat32,
            Layout::Single,
            &plan,
            true,
        );
        assert!(!a.contains(&"-a".to_string()) && !a.contains(&"63s".to_string()));
    }

    #[test]
    fn partition_node_names() {
        assert_eq!(partition_path("/dev/sdb", 1), "/dev/sdb1");
        assert_eq!(partition_path("/dev/nvme0n1", 2), "/dev/nvme0n1p2");
        assert_eq!(partition_path("/dev/mmcblk0", 1), "/dev/mmcblk0p1");
    }

    #[test]
    fn labels_are_sanitised_per_file_system() {
        assert_eq!(
            sanitize_label(FileSystem::Fat32, "my usb: stick-long-name"),
            "MY USB_ STI"
        );
        assert_eq!(sanitize_label(FileSystem::Fat32, "--help"), "HELP");
        assert_eq!(sanitize_label(FileSystem::Ntfs, "   "), "RUFUX");
        assert_eq!(sanitize_label(FileSystem::Ext4, "éééééééééé"), "__________");
        assert!(sanitize_label(FileSystem::Ext4, "abcdefghijklmnopqrstuvwxyz").len() <= 16);
    }

    #[test]
    fn mkfs_arguments() {
        let a = mkfs_args(FileSystem::Fat32, "boot", Some(8192), true, "/dev/sdb1");
        assert_eq!(a, ["-F", "32", "-n", "BOOT", "-s", "16", "/dev/sdb1"]);
        let n = mkfs_args(FileSystem::Ntfs, "win", None, true, "/dev/sdb1");
        assert_eq!(n, ["-Q", "-L", "win", "/dev/sdb1"]);
        let x = mkfs_args(FileSystem::ExFat, "data", Some(131072), true, "/dev/sdb1");
        assert_eq!(x, ["-L", "data", "-c", "131072", "/dev/sdb1"]);
        let bad = mkfs_args(FileSystem::Fat32, "x", Some(1000), true, "/dev/sdb1");
        assert!(!bad.contains(&"-s".to_string()));
    }

    #[test]
    fn enums_parse_from_cli_text() {
        assert_eq!("gpt".parse::<PartitionScheme>().unwrap(), PartitionScheme::Gpt);
        assert_eq!("exFAT".parse::<FileSystem>().unwrap(), FileSystem::ExFat);
        assert!("zfs".parse::<FileSystem>().is_err());
    }
}
