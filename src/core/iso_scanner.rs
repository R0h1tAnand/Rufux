//! ISO and Disk Image Inspection Engine
//!
//! Analyzes ISO 9660, UDF, and raw disk images to detect operating system type,
//! bootloader capabilities (UEFI / BIOS), install.wim size, and persistence support.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageType {
    Windows,
    Linux,
    FreeDos,
    RawDisk,
    Unknown,
}

#[derive(Debug, Clone)]
pub struct IsoReport {
    pub path: PathBuf,
    pub filename: String,
    pub size_bytes: u64,
    pub image_type: ImageType,
    pub volume_label: String,
    pub is_iso: bool,
    pub is_isohybrid: bool,
    pub is_efi_bootable: bool,
    pub is_bios_bootable: bool,
    pub has_install_wim: bool,
    pub install_wim_size: u64,
    pub has_persistence: bool,
    pub windows_version: Option<String>,
    pub recommended_partition_scheme: String,
    pub recommended_target_system: String,
    pub recommended_file_system: String,
}

/// Analyze an ISO or disk image
pub fn scan_image(image_path: &Path) -> anyhow::Result<IsoReport> {
    let mut file = File::open(image_path)?;
    let size_bytes = file.metadata()?.len();
    let filename = image_path.file_name().unwrap_or_default().to_string_lossy().to_string();

    let ext = image_path.extension().and_then(|s| s.to_str()).unwrap_or("").to_lowercase();
    let is_iso = ext == "iso" || ext == "udf";

    // 1. Check MBR boot signature at offset 0x1fe
    let mut mbr_buf = [0u8; 512];
    let mut has_mbr_signature = false;
    if file.read_exact(&mut mbr_buf).is_ok() {
        if mbr_buf[510] == 0x55 && mbr_buf[511] == 0xAA {
            has_mbr_signature = true;
        }
    }

    // 2. Read ISO 9660 Volume Descriptor at 0x8000
    let mut volume_label = String::new();
    let mut is_iso9660 = false;
    if file.seek(SeekFrom::Start(0x8000)).is_ok() {
        let mut pvd = [0u8; 2048];
        if file.read_exact(&mut pvd).is_ok() {
            // Check standard identifier "CD001"
            if &pvd[1..6] == b"CD001" {
                is_iso9660 = true;
                // Volume Identifier is at offset 40 (0x28) for 32 bytes
                let raw_label = &pvd[40..72];
                volume_label = String::from_utf8_lossy(raw_label).trim().to_string();
            }
        }
    }

    if volume_label.is_empty() {
        // Fallback to filename stem
        volume_label = image_path.file_stem().unwrap_or_default().to_string_lossy().to_string();
    }

    let is_isohybrid = (is_iso || is_iso9660) && has_mbr_signature;

    // 3. Inspect internal files via 7z or direct check
    let mut image_type = ImageType::Unknown;
    let mut is_efi_bootable = false;
    let mut is_bios_bootable = false;
    let mut has_install_wim = false;
    let mut install_wim_size = 0u64;
    let mut has_persistence = false;
    let mut windows_version = None;

    if is_iso || is_iso9660 {
        if let Ok(file_list) = list_iso_files_with_7z(image_path) {
            for (f_path, f_size) in file_list {
                let lower = f_path.to_lowercase();

                // Check Windows indicators
                if lower.contains("sources/install.wim") || lower.contains("sources/install.esd") {
                    has_install_wim = true;
                    install_wim_size = f_size;
                    image_type = ImageType::Windows;
                }
                if lower.contains("sources/boot.wim") {
                    image_type = ImageType::Windows;
                }

                // Check EFI bootloaders
                if lower.contains("efi/boot/bootx64.efi")
                    || lower.contains("efi/boot/bootaa64.efi")
                    || lower.contains("efi/boot/bootia32.efi")
                    || lower.contains("efi/boot/bootarm.efi")
                {
                    is_efi_bootable = true;
                }

                // Check BIOS bootloaders
                if lower.contains("bootmgr")
                    || lower.contains("isolinux.bin")
                    || lower.contains("syslinux.bin")
                    || lower.contains("ldlinux.c32")
                    || lower.contains("boot/grub")
                {
                    is_bios_bootable = true;
                }

                // Check Linux persistence distributions
                if lower.contains("casper") || lower.contains("live") || lower.contains("arch") {
                    if image_type == ImageType::Unknown {
                        image_type = ImageType::Linux;
                    }
                    if lower.contains("casper") || lower.contains("live") {
                        has_persistence = true;
                    }
                }
            }
        }
    } else {
        image_type = ImageType::RawDisk;
    }

    if image_type == ImageType::Windows {
        // Detect Windows version from label or filename
        let lower_label = format!("{} {}", volume_label, filename).to_lowercase();
        if lower_label.contains("win11") || lower_label.contains("windows 11") || lower_label.contains("w11") {
            windows_version = Some("Windows 11".to_string());
        } else if lower_label.contains("win10") || lower_label.contains("windows 10") || lower_label.contains("w10") {
            windows_version = Some("Windows 10".to_string());
        } else if lower_label.contains("win8") || lower_label.contains("windows 8") {
            windows_version = Some("Windows 8.1".to_string());
        } else if lower_label.contains("win7") || lower_label.contains("windows 7") {
            windows_version = Some("Windows 7".to_string());
        } else {
            windows_version = Some("Windows 10/11".to_string());
        }
    }

    // Determine recommendations matching Rufus defaults
    let (rec_part, rec_target, rec_fs) = match image_type {
        ImageType::Windows => {
            if windows_version.as_deref() == Some("Windows 11") {
                ("GPT".to_string(), "UEFI (non CSM)".to_string(), "NTFS".to_string())
            } else if is_efi_bootable {
                ("GPT".to_string(), "UEFI (non CSM)".to_string(), "NTFS".to_string())
            } else {
                ("MBR".to_string(), "BIOS (or UEFI-CSM)".to_string(), "NTFS".to_string())
            }
        }
        ImageType::Linux => {
            if is_efi_bootable {
                ("GPT".to_string(), "UEFI (non CSM)".to_string(), "FAT32".to_string())
            } else {
                ("MBR".to_string(), "BIOS (or UEFI-CSM)".to_string(), "FAT32".to_string())
            }
        }
        _ => {
            ("MBR".to_string(), "BIOS (or UEFI-CSM)".to_string(), "FAT32".to_string())
        }
    };

    Ok(IsoReport {
        path: image_path.to_path_buf(),
        filename,
        size_bytes,
        image_type,
        volume_label,
        is_iso,
        is_isohybrid,
        is_efi_bootable,
        is_bios_bootable,
        has_install_wim,
        install_wim_size,
        has_persistence,
        windows_version,
        recommended_partition_scheme: rec_part,
        recommended_target_system: rec_target,
        recommended_file_system: rec_fs,
    })
}

/// Lists files inside an ISO using 7z
fn list_iso_files_with_7z(iso_path: &Path) -> anyhow::Result<Vec<(String, u64)>> {
    let output = Command::new("7z")
        .args(["l", "-slt"])
        .arg(iso_path)
        .output()?;

    if !output.status.success() {
        return Ok(Vec::new());
    }

    let text = String::from_utf8_lossy(&output.stdout);
    let mut files = Vec::new();
    let mut current_path = String::new();
    let mut current_size = 0u64;

    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("Path = ") {
            current_path = trimmed[7..].to_string();
        } else if trimmed.starts_with("Size = ") {
            if let Ok(sz) = trimmed[7..].parse::<u64>() {
                current_size = sz;
            }
        } else if trimmed.is_empty() {
            if !current_path.is_empty() {
                files.push((current_path.clone(), current_size));
                current_path.clear();
                current_size = 0;
            }
        }
    }

    if !current_path.is_empty() {
        files.push((current_path, current_size));
    }

    Ok(files)
}

#[cfg(test)]
mod tests {
    #[test]
    fn test_image_type_detection() {
        // Basic sanity test
    }
}
