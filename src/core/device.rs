//! Storage Device Discovery, Inspection, and Safety
//!
//! Enumerates block devices via lsblk/sysfs/udev, extracts disk metadata,
//! and protects critical system drives from accidental overwrites.

use std::fs::File;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::Command;
use serde::Deserialize;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PartitionInfo {
    pub name: String,
    pub path: PathBuf,
    pub size_bytes: u64,
    pub fstype: Option<String>,
    pub mountpoint: Option<String>,
    pub label: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeviceInfo {
    pub name: String,
    pub path: PathBuf,
    pub vendor: String,
    pub model: String,
    pub size_bytes: u64,
    pub is_removable: bool,
    pub bus: String,
    pub partitions: Vec<PartitionInfo>,
    pub is_system_drive: bool,
}

impl DeviceInfo {
    /// Formats device info for display matching Rufus dropdown style:
    /// e.g. "SanDisk Ultra USB 3.0 (32 GB) [/dev/sdb]"
    pub fn display_label(&self) -> String {
        let name_str = if !self.model.is_empty() {
            if !self.vendor.is_empty() && !self.model.starts_with(&self.vendor) {
                format!("{} {}", self.vendor, self.model)
            } else {
                self.model.clone()
            }
        } else if !self.vendor.is_empty() {
            self.vendor.clone()
        } else {
            self.name.clone()
        };

        let size_str = format_size(self.size_bytes);
        format!("{} ({}) [{}]", name_str, size_str, self.path.display())
    }
}

/// Helper to format bytes into GB/MB matching Rufus
pub fn format_size(bytes: u64) -> String {
    let gb = bytes as f64 / (1024.0 * 1024.0 * 1024.0);
    if gb >= 1.0 {
        format!("{:.1} GB", gb)
    } else {
        let mb = bytes as f64 / (1024.0 * 1024.0);
        format!("{:.0} MB", mb)
    }
}

// Structs for parsing lsblk JSON
#[derive(Deserialize)]
struct LsblkRoot {
    blockdevices: Vec<LsblkDevice>,
}

#[derive(Deserialize)]
struct LsblkDevice {
    name: String,
    #[serde(rename = "kname")]
    kname: Option<String>,
    path: Option<String>,
    model: Option<String>,
    vendor: Option<String>,
    size: Option<u64>,
    rm: Option<bool>,
    hotplug: Option<bool>,
    tran: Option<String>,
    #[serde(rename = "type")]
    dev_type: Option<String>,
    mountpoint: Option<String>,
    mountpoints: Option<Vec<Option<String>>>,
    label: Option<String>,
    fstype: Option<String>,
    children: Option<Vec<LsblkDevice>>,
}

/// Enumerate removable USB storage devices
pub fn enumerate_devices(list_all_drives: bool) -> anyhow::Result<Vec<DeviceInfo>> {
    let output = Command::new("lsblk")
        .args(["--json", "-b", "-O"])
        .output()?;

    if !output.status.success() {
        return Err(anyhow::anyhow!("Failed to run lsblk: status {:?}", output.status));
    }

    let root: LsblkRoot = serde_json::from_slice(&output.stdout)?;
    let mut devices = Vec::new();

    for dev in root.blockdevices {
        let dev_type = dev.dev_type.as_deref().unwrap_or("");
        if dev_type != "disk" {
            continue;
        }

        let name = dev.kname.clone().unwrap_or(dev.name.clone());
        // Skip loop devices, zram, ram disks
        if name.starts_with("loop") || name.starts_with("zram") || name.starts_with("ram") {
            continue;
        }

        let path = dev.path.as_deref().map(PathBuf::from).unwrap_or_else(|| PathBuf::from(format!("/dev/{}", name)));
        let vendor = dev.vendor.as_deref().unwrap_or("").trim().to_string();
        let model = dev.model.as_deref().unwrap_or("").trim().to_string();
        let size_bytes = dev.size.unwrap_or(0);
        let bus = dev.tran.as_deref().unwrap_or("").to_lowercase();
        let is_removable = dev.rm.unwrap_or(false) || dev.hotplug.unwrap_or(false) || bus == "usb";

        // Check if any partition contains root filesystem "/" or critical system mounts
        let mut is_system_drive = false;
        let mut partitions = Vec::new();

        check_mounts(&dev, &mut is_system_drive, &mut partitions);

        // Safety filter:
        // By default, only show removable drives or USB drives, and never show root drives
        if is_system_drive && !list_all_drives {
            continue;
        }

        if !is_removable && !list_all_drives {
            continue;
        }

        devices.push(DeviceInfo {
            name,
            path,
            vendor,
            model,
            size_bytes,
            is_removable,
            bus,
            partitions,
            is_system_drive,
        });
    }

    Ok(devices)
}

fn check_mounts(dev: &LsblkDevice, is_system_drive: &mut bool, partitions: &mut Vec<PartitionInfo>) {
    // Check main device mountpoint
    if let Some(mp) = &dev.mountpoint {
        if mp == "/" || mp == "/boot" || mp == "/home" {
            *is_system_drive = true;
        }
    }
    if let Some(mps) = &dev.mountpoints {
        for mp in mps.iter().flatten() {
            if mp == "/" || mp == "/boot" || mp == "/home" {
                *is_system_drive = true;
            }
        }
    }

    if let Some(children) = &dev.children {
        for child in children {
            let child_name = child.kname.clone().unwrap_or(child.name.clone());
            let child_path = child.path.clone().map(PathBuf::from).unwrap_or_else(|| PathBuf::from(format!("/dev/{}", child_name)));
            let child_size = child.size.unwrap_or(0);
            let mut mount = child.mountpoint.clone();
            if mount.is_none() {
                if let Some(mps) = &child.mountpoints {
                    mount = mps.iter().flatten().next().cloned();
                }
            }

            if let Some(ref mp) = mount {
                if mp == "/" || mp == "/boot" || mp == "/home" {
                    *is_system_drive = true;
                }
            }

            partitions.push(PartitionInfo {
                name: child_name,
                path: child_path,
                size_bytes: child_size,
                fstype: child.fstype.clone(),
                mountpoint: mount,
                label: child.label.clone(),
            });

            check_mounts(child, is_system_drive, partitions);
        }
    }
}

/// Unmounts all mounted partitions on a target device
pub fn unmount_all(device_path: &Path) -> anyhow::Result<()> {
    // Query mounts for this device
    let dev_str = device_path.to_string_lossy();
    let mounts_content = std::fs::read_to_string("/proc/mounts").unwrap_or_default();

    for line in mounts_content.lines() {
        let parts: Vec<&str> = line.split_whitespace().collect();
        if parts.len() >= 2 {
            let mount_dev = parts[0];
            let mount_dir = parts[1];
            if mount_dev.starts_with(dev_str.as_ref()) {
                let _ = Command::new("umount").arg("-f").arg(mount_dir).status();
            }
        }
    }

    Ok(())
}

/// Dumps a physical device to an image file (matching Rufus "..." dump button)
pub fn dump_device_to_image<F>(
    device_path: &Path,
    output_path: &Path,
    mut progress_callback: F,
) -> anyhow::Result<()>
where
    F: FnMut(f32, u64),
{
    let mut src = File::open(device_path)?;
    let mut dst = File::create(output_path)?;

    let total_size = src.metadata()?.len();
    let mut buffer = vec![0u8; 4 * 1024 * 1024]; // 4 MiB buffer
    let mut bytes_written = 0u64;

    loop {
        let n = src.read(&mut buffer)?;
        if n == 0 {
            break;
        }

        dst.write_all(&buffer[..n])?;
        bytes_written += n as u64;

        if total_size > 0 {
            let progress = (bytes_written as f32 / total_size as f32).min(1.0);
            progress_callback(progress, bytes_written);
        }
    }

    dst.sync_all()?;
    progress_callback(1.0, bytes_written);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_format_size() {
        assert_eq!(format_size(32 * 1024 * 1024 * 1024), "32.0 GB");
        assert_eq!(format_size(512 * 1024 * 1024), "512 MB");
    }
}
