//! Core Formatting, Partitioning, and Image Flashing Engine
//!
//! Handles both direct DD image streaming and full ISO extraction mode
//! with MBR/GPT partitioning, UEFI:NTFS creation, and WUE answer file injection.

use std::fs::{File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Instant;
use tempfile::TempDir;

use super::device::unmount_all;
use super::iso_scanner::IsoReport;
use super::uefi_ntfs::write_uefi_ntfs;
use super::wue::{install_wue_answer_file, WueOptions};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BurnMode {
    IsoMode,
    DdMode,
}

#[derive(Debug, Clone)]
pub struct BurnOptions {
    pub device_path: PathBuf,
    pub image_path: Option<PathBuf>,
    pub burn_mode: BurnMode,
    pub partition_scheme: String, // "MBR" or "GPT"
    pub target_system: String,    // "BIOS or UEFI" or "UEFI (non CSM)"
    pub file_system: String,      // "FAT32", "NTFS", "exFAT", "ext4"
    pub volume_label: String,
    pub quick_format: bool,
    pub persistence_size_mb: u64,
    pub wue_options: Option<WueOptions>,
    pub wue_username: String,
}

#[derive(Debug, Clone)]
pub enum BurnProgress {
    Status(String),
    Progress { percent: f32, speed_mb_s: f32 },
    Log(String),
    Finished,
    Error(String),
}

/// Executes the flash/format operation, streaming progress through the callback
pub fn execute_burn<F>(
    opts: &BurnOptions,
    iso_report: Option<&IsoReport>,
    cancel_flag: Arc<AtomicBool>,
    mut callback: F,
) where
    F: FnMut(BurnProgress),
{
    callback(BurnProgress::Log(format!("Starting operation on {}", opts.device_path.display())));

    if opts.burn_mode == BurnMode::DdMode {
        if let Some(ref img) = opts.image_path {
            execute_dd_mode(img, &opts.device_path, cancel_flag, &mut callback);
        } else {
            callback(BurnProgress::Error("No image file specified for DD mode".to_string()));
        }
    } else {
        execute_iso_mode(opts, iso_report, cancel_flag, &mut callback);
    }
}

/// Raw DD image stream to block device
fn execute_dd_mode<F>(
    image_path: &Path,
    device_path: &Path,
    cancel_flag: Arc<AtomicBool>,
    callback: &mut F,
) where
    F: FnMut(BurnProgress),
{
    callback(BurnProgress::Status("Unmounting partitions...".to_string()));
    let _ = unmount_all(device_path);

    callback(BurnProgress::Status("Opening raw device for streaming...".to_string()));
    callback(BurnProgress::Log(format!("Direct streaming {} -> {}", image_path.display(), device_path.display())));

    let mut src = match File::open(image_path) {
        Ok(f) => f,
        Err(e) => {
            callback(BurnProgress::Error(format!("Failed to open source image: {}", e)));
            return;
        }
    };

    let total_size = match src.metadata() {
        Ok(m) => m.len(),
        Err(e) => {
            callback(BurnProgress::Error(format!("Failed to query image size: {}", e)));
            return;
        }
    };

    let mut dst = match OpenOptions::new().write(true).open(device_path) {
        Ok(f) => f,
        Err(e) => {
            callback(BurnProgress::Error(format!("Failed to open destination device: {}", e)));
            return;
        }
    };

    let buffer_size = 4 * 1024 * 1024; // 4 MiB chunks
    let mut buffer = vec![0u8; buffer_size];
    let mut bytes_written = 0u64;

    let start_time = Instant::now();
    let mut last_update = Instant::now();

    loop {
        if cancel_flag.load(Ordering::Relaxed) {
            callback(BurnProgress::Error("Operation canceled by user".to_string()));
            return;
        }

        let n = match src.read(&mut buffer) {
            Ok(0) => break,
            Ok(n) => n,
            Err(e) => {
                callback(BurnProgress::Error(format!("Read error: {}", e)));
                return;
            }
        };

        if let Err(e) = dst.write_all(&buffer[..n]) {
            callback(BurnProgress::Error(format!("Write error: {}", e)));
            return;
        }

        bytes_written += n as u64;

        if last_update.elapsed().as_millis() >= 200 {
            let elapsed_sec = start_time.elapsed().as_secs_f32().max(0.001);
            let speed_mb_s = (bytes_written as f32 / (1024.0 * 1024.0)) / elapsed_sec;
            let percent = if total_size > 0 {
                (bytes_written as f32 / total_size as f32).min(1.0)
            } else {
                0.0
            };

            callback(BurnProgress::Status(format!("Writing raw image... {:.1}% ({:.1} MB/s)", percent * 100.0, speed_mb_s)));
            callback(BurnProgress::Progress { percent, speed_mb_s });
            last_update = Instant::now();
        }
    }

    callback(BurnProgress::Status("Flushing buffers to disk...".to_string()));
    callback(BurnProgress::Log("Flushing device write caches (sync)...".to_string()));
    let _ = dst.sync_all();
    let _ = Command::new("sync").status();

    callback(BurnProgress::Status("Operation completed successfully!".to_string()));
    callback(BurnProgress::Progress { percent: 1.0, speed_mb_s: 0.0 });
    callback(BurnProgress::Finished);
}

/// Full ISO partition and extraction mode
fn execute_iso_mode<F>(
    opts: &BurnOptions,
    _iso_report: Option<&IsoReport>,
    cancel_flag: Arc<AtomicBool>,
    callback: &mut F,
) where
    F: FnMut(BurnProgress),
{
    callback(BurnProgress::Status("Unmounting partitions...".to_string()));
    callback(BurnProgress::Log("Unmounting active partitions...".to_string()));
    let _ = unmount_all(&opts.device_path);

    // 1. Wipe existing partition table
    callback(BurnProgress::Status("Clearing partition table...".to_string()));
    callback(BurnProgress::Log("Wiping file system signatures (wipefs)...".to_string()));
    let _ = Command::new("wipefs")
        .args(["-a"])
        .arg(&opts.device_path)
        .status();

    // Zero MBR/GPT sectors
    if let Ok(mut dev) = OpenOptions::new().write(true).open(&opts.device_path) {
        let zero_buf = vec![0u8; 1024 * 1024];
        let _ = dev.write_all(&zero_buf);
        let _ = dev.sync_all();
    }

    if cancel_flag.load(Ordering::Relaxed) {
        callback(BurnProgress::Error("Operation canceled by user".to_string()));
        return;
    }

    // 2. Create partition table using parted
    let is_gpt = opts.partition_scheme.to_uppercase() == "GPT";
    let label_type = if is_gpt { "gpt" } else { "msdos" };
    callback(BurnProgress::Status(format!("Creating {} partition table...", label_type.to_uppercase())));
    callback(BurnProgress::Log(format!("Running parted mklabel {}", label_type)));

    let status = Command::new("parted")
        .args(["-s", &opts.device_path.to_string_lossy(), "mklabel", label_type])
        .status();

    if let Err(e) = status {
        callback(BurnProgress::Error(format!("Failed to create partition table: {}", e)));
        return;
    }

    // 3. Check if UEFI:NTFS dual partition layout is needed
    let is_uefi_ntfs = opts.file_system.to_uppercase() == "NTFS" && opts.target_system.contains("UEFI");

    // Partition creation
    callback(BurnProgress::Status("Creating partitions...".to_string()));
    if is_uefi_ntfs {
        callback(BurnProgress::Log("Creating UEFI:NTFS dual partition scheme (Partition 1: NTFS, Partition 2: UEFI:NTFS FAT)...".to_string()));
        // Partition 1: from 1MiB to 100% - 2MiB
        let _ = Command::new("parted")
            .args([
                "-s",
                &opts.device_path.to_string_lossy(),
                "mkpart",
                "primary",
                "1MiB",
                "-2MiB",
            ])
            .status();

        // Partition 2: from -2MiB to 100%
        let _ = Command::new("parted")
            .args([
                "-s",
                &opts.device_path.to_string_lossy(),
                "mkpart",
                "primary",
                "fat16",
                "-2MiB",
                "100%",
            ])
            .status();

        if is_gpt {
            let _ = Command::new("parted")
                .args(["-s", &opts.device_path.to_string_lossy(), "set", "2", "esp", "on"])
                .status();
        }
    } else if opts.persistence_size_mb > 0 {
        callback(BurnProgress::Log(format!("Creating persistence layout with {} MB persistence partition...", opts.persistence_size_mb)));
        let end_part1 = format!("-{}MiB", opts.persistence_size_mb);
        let _ = Command::new("parted")
            .args([
                "-s",
                &opts.device_path.to_string_lossy(),
                "mkpart",
                "primary",
                "fat32",
                "1MiB",
                &end_part1,
            ])
            .status();

        let _ = Command::new("parted")
            .args([
                "-s",
                &opts.device_path.to_string_lossy(),
                "mkpart",
                "primary",
                "ext4",
                &end_part1,
                "100%",
            ])
            .status();
    } else {
        // Standard single partition
        let part_fs = match opts.file_system.to_uppercase().as_str() {
            "NTFS" => "ntfs",
            "EXFAT" => "fat32",
            "EXT4" => "ext4",
            _ => "fat32",
        };
        let _ = Command::new("parted")
            .args([
                "-s",
                &opts.device_path.to_string_lossy(),
                "mkpart",
                "primary",
                part_fs,
                "1MiB",
                "100%",
            ])
            .status();

        if !is_gpt {
            let _ = Command::new("parted")
                .args(["-s", &opts.device_path.to_string_lossy(), "set", "1", "boot", "on"])
                .status();
        }
    }

    // Refresh partitions
    let _ = Command::new("partx").args(["-u", &opts.device_path.to_string_lossy()]).status();
    let _ = Command::new("udevadm").args(["settle"]).status();
    std::thread::sleep(std::time::Duration::from_millis(500));

    // Determine partition 1 device path
    let dev_str = opts.device_path.to_string_lossy().to_string();
    let part1_path = if dev_str.ends_with(|c: char| c.is_ascii_digit()) {
        format!("{}p1", dev_str)
    } else {
        format!("{}1", dev_str)
    };

    let part2_path = if dev_str.ends_with(|c: char| c.is_ascii_digit()) {
        format!("{}p2", dev_str)
    } else {
        format!("{}2", dev_str)
    };

    // 4. Format Partition 1
    let label = if opts.volume_label.is_empty() { "RUFUX" } else { &opts.volume_label };
    callback(BurnProgress::Status(format!("Formatting partition 1 as {}...", opts.file_system)));
    callback(BurnProgress::Log(format!("Formatting {} as {} with label '{}'...", part1_path, opts.file_system, label)));

    let format_status = match opts.file_system.to_uppercase().as_str() {
        "NTFS" => Command::new("mkfs.ntfs").args(["-f", "-L", label, &part1_path]).status(),
        "FAT32" => Command::new("mkfs.vfat").args(["-F", "32", "-n", label, &part1_path]).status(),
        "EXFAT" => Command::new("mkfs.exfat").args(["-n", label, &part1_path]).status(),
        "EXT4" => Command::new("mkfs.ext4").args(["-F", "-L", label, &part1_path]).status(),
        _ => Command::new("mkfs.vfat").args(["-F", "32", "-n", label, &part1_path]).status(),
    };

    if let Err(e) = format_status {
        callback(BurnProgress::Error(format!("Failed to format partition 1: {}", e)));
        return;
    }

    // Write UEFI:NTFS if required
    if is_uefi_ntfs {
        callback(BurnProgress::Log(format!("Writing UEFI:NTFS driver image to {}...", part2_path)));
        if let Err(e) = write_uefi_ntfs(Path::new(&part2_path), 0) {
            callback(BurnProgress::Log(format!("Warning: Could not write UEFI:NTFS image directly: {}", e)));
        }
    }

    // Format persistence partition if required
    if opts.persistence_size_mb > 0 {
        callback(BurnProgress::Log(format!("Formatting persistence partition {} as ext4 (casper-rw)...", part2_path)));
        let _ = Command::new("mkfs.ext4").args(["-F", "-L", "casper-rw", &part2_path]).status();
    }

    if cancel_flag.load(Ordering::Relaxed) {
        callback(BurnProgress::Error("Operation canceled by user".to_string()));
        return;
    }

    // 5. Mount partition 1 and extract ISO
    if let Some(ref image_path) = opts.image_path {
        let temp_dir = match TempDir::new() {
            Ok(t) => t,
            Err(e) => {
                callback(BurnProgress::Error(format!("Failed to create temporary mount directory: {}", e)));
                return;
            }
        };

        let mount_point = temp_dir.path();
        callback(BurnProgress::Status("Mounting target partition...".to_string()));
        callback(BurnProgress::Log(format!("Mounting {} to {}", part1_path, mount_point.display())));

        let mount_status = Command::new("mount").arg(&part1_path).arg(mount_point).status();
        if let Err(e) = mount_status {
            callback(BurnProgress::Error(format!("Failed to mount partition: {}", e)));
            return;
        }

        callback(BurnProgress::Status("Extracting ISO files...".to_string()));
        callback(BurnProgress::Log(format!("Extracting {} into {} via 7z...", image_path.display(), mount_point.display())));

        let out_arg = format!("-o{}", mount_point.display());
        let extract_cmd = Command::new("7z")
            .args(["x", "-y", &out_arg])
            .arg(image_path)
            .status();

        if let Err(e) = extract_cmd {
            let _ = Command::new("umount").arg(mount_point).status();
            callback(BurnProgress::Error(format!("Extraction error: {}", e)));
            return;
        }

        // Apply Windows User Experience (WUE) tweaks if applicable
        if let Some(ref wue_opts) = opts.wue_options {
            callback(BurnProgress::Status("Applying Windows User Experience customizations...".to_string()));
            callback(BurnProgress::Log("Injecting autounattend.xml (TPM/SecureBoot/RAM bypass, Account bypass)...".to_string()));
            if let Err(e) = install_wue_answer_file(mount_point, wue_opts, &opts.wue_username, "en-US") {
                callback(BurnProgress::Log(format!("Warning: Failed to install WUE answer file: {}", e)));
            }
        }

        callback(BurnProgress::Status("Flushing buffers to disk...".to_string()));
        callback(BurnProgress::Log("Synchronizing filesystem caches...".to_string()));
        let _ = Command::new("sync").status();

        callback(BurnProgress::Status("Unmounting volume...".to_string()));
        let _ = Command::new("umount").arg(mount_point).status();
    }

    callback(BurnProgress::Status("Operation completed successfully!".to_string()));
    callback(BurnProgress::Progress { percent: 1.0, speed_mb_s: 0.0 });
    callback(BurnProgress::Finished);
}
