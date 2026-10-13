//! ISO and Disk Image Inspection Engine
//!
//! Analyzes ISO 9660, UDF, and raw disk images to detect operating system type,
//! bootloader capabilities (UEFI / BIOS), install.wim size, and persistence support.

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use anyhow::Result;

use super::exec;
use super::layout::{FileSystem, PartitionScheme, TargetSystem};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ImageType {
    Windows,
    Linux,
    FreeDos,
    RawDisk,
    Unknown,
}

/// Flavour of Linux live persistence an image supports.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PersistenceKind {
    /// Ubuntu family: ext4 partition labelled `casper-rw`.
    Casper,
    /// Debian live family: ext4 partition labelled `persistence` plus a `persistence.conf`.
    DebianLive,
}

impl PersistenceKind {
    pub fn partition_label(self) -> &'static str {
        match self {
            Self::Casper => "casper-rw",
            Self::DebianLive => "persistence",
        }
    }
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
    pub persistence: Option<PersistenceKind>,
    pub windows_version: Option<String>,
    pub recommended_partition_scheme: PartitionScheme,
    pub recommended_target_system: TargetSystem,
    pub recommended_file_system: FileSystem,
}

/// What the file listing of an ISO tells us.
#[derive(Debug, Clone, PartialEq, Eq)]
struct Classification {
    image_type: ImageType,
    is_efi_bootable: bool,
    is_bios_bootable: bool,
    has_install_wim: bool,
    install_wim_size: u64,
    persistence: Option<PersistenceKind>,
}

fn normalize(path: &str) -> String {
    path.replace('\\', "/").trim_start_matches('/').to_lowercase()
}

fn classify(files: &[(String, u64)]) -> Classification {
    let mut c = Classification {
        image_type: ImageType::Unknown,
        is_efi_bootable: false,
        is_bios_bootable: false,
        has_install_wim: false,
        install_wim_size: 0,
        persistence: None,
    };
    let mut linux_hint = false;

    for (raw, size) in files {
        let p = normalize(raw);

        if p == "sources/install.wim" || p == "sources/install.esd" {
            c.has_install_wim = true;
            c.install_wim_size = *size;
            c.image_type = ImageType::Windows;
        } else if p == "sources/boot.wim" {
            c.image_type = ImageType::Windows;
        }

        if (p.starts_with("efi/boot/boot") && p.ends_with(".efi"))
            || p.ends_with("efi.img")
            || p.ends_with("efiboot.img")
        {
            c.is_efi_bootable = true;
        }

        if p == "bootmgr"
            || p.ends_with("isolinux.bin")
            || p.ends_with("syslinux.bin")
            || p.ends_with("ldlinux.c32")
            || p.starts_with("boot/grub/")
            || p.starts_with("boot/grub2/")
        {
            c.is_bios_bootable = true;
        }

        if p.starts_with("casper/") {
            c.persistence = Some(PersistenceKind::Casper);
            linux_hint = true;
        } else if p.starts_with("live/") && p.ends_with(".squashfs") {
            c.persistence.get_or_insert(PersistenceKind::DebianLive);
            linux_hint = true;
        } else if [
            "live/",
            "arch/",
            "isolinux/",
            "syslinux/",
            "loader/",
            "boot/vmlinuz",
            "images/pxeboot/",
            "boot/grub/",
        ]
        .iter()
        .any(|prefix| p.starts_with(prefix))
        {
            linux_hint = true;
        }
    }

    if c.image_type == ImageType::Unknown && linux_hint {
        c.image_type = ImageType::Linux;
    }
    c
}

fn windows_version(volume_label: &str, filename: &str) -> String {
    let hay = format!("{} {}", volume_label, filename).to_lowercase();
    let has = |needles: &[&str]| needles.iter().any(|n| hay.contains(n));
    if has(&["win11", "windows 11", "w11"]) {
        "Windows 11"
    } else if has(&["win10", "windows 10", "w10"]) {
        "Windows 10"
    } else if has(&["win8", "windows 8"]) {
        "Windows 8.1"
    } else if has(&["win7", "windows 7"]) {
        "Windows 7"
    } else {
        "Windows 10/11"
    }
    .to_string()
}

/// Analyze an ISO or disk image
pub fn scan_image(image_path: &Path) -> Result<IsoReport> {
    let mut file = File::open(image_path)?;
    let size_bytes = file.metadata()?.len();
    let filename = image_path.file_name().unwrap_or_default().to_string_lossy().to_string();

    let ext = image_path
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_lowercase();
    let is_iso = ext == "iso" || ext == "udf";

    // 1. Check MBR boot signature at offset 0x1fe
    let mut mbr_buf = [0u8; 512];
    let has_mbr_signature = file.read_exact(&mut mbr_buf).is_ok() && mbr_buf[510] == 0x55 && mbr_buf[511] == 0xAA;

    // 2. Read ISO 9660 Volume Descriptor at 0x8000
    let mut volume_label = String::new();
    let mut is_iso9660 = false;
    if file.seek(SeekFrom::Start(0x8000)).is_ok() {
        let mut pvd = [0u8; 2048];
        if file.read_exact(&mut pvd).is_ok() && &pvd[1..6] == b"CD001" {
            is_iso9660 = true;
            // Volume Identifier is at offset 40 (0x28) for 32 bytes
            volume_label = String::from_utf8_lossy(&pvd[40..72])
                .trim_matches(|c: char| c.is_whitespace() || c == '\0')
                .to_string();
        }
    }

    if volume_label.is_empty() {
        volume_label = image_path.file_stem().unwrap_or_default().to_string_lossy().to_string();
    }

    let is_isohybrid = (is_iso || is_iso9660) && has_mbr_signature;

    // 3. Inspect internal files via 7z
    let class = if is_iso || is_iso9660 {
        let listing = list_iso_files_with_7z(image_path).unwrap_or_default();
        classify(&listing)
    } else {
        Classification {
            image_type: ImageType::RawDisk,
            is_efi_bootable: false,
            is_bios_bootable: false,
            has_install_wim: false,
            install_wim_size: 0,
            persistence: None,
        }
    };

    let windows_version = (class.image_type == ImageType::Windows).then(|| windows_version(&volume_label, &filename));

    // Recommendations matching Rufus defaults
    let uefi_preferred = class.is_efi_bootable || windows_version.as_deref() == Some("Windows 11");
    let (rec_part, rec_target) = match (class.image_type, uefi_preferred) {
        (ImageType::Windows | ImageType::Linux, true) => (PartitionScheme::Gpt, TargetSystem::Uefi),
        _ => (PartitionScheme::Mbr, TargetSystem::BiosOrUefi),
    };
    let rec_fs = if class.image_type == ImageType::Windows {
        FileSystem::Ntfs
    } else {
        FileSystem::Fat32
    };

    Ok(IsoReport {
        path: image_path.to_path_buf(),
        filename,
        size_bytes,
        image_type: class.image_type,
        volume_label,
        is_iso,
        is_isohybrid,
        is_efi_bootable: class.is_efi_bootable,
        is_bios_bootable: class.is_bios_bootable,
        has_install_wim: class.has_install_wim,
        install_wim_size: class.install_wim_size,
        has_persistence: class.persistence.is_some(),
        persistence: class.persistence,
        windows_version,
        recommended_partition_scheme: rec_part,
        recommended_target_system: rec_target,
        recommended_file_system: rec_fs,
    })
}

/// Lists files inside an ISO using 7z
fn list_iso_files_with_7z(iso_path: &Path) -> Result<Vec<(String, u64)>> {
    let tool = exec::find_any_tool(&["7z", "7zz"]).ok_or_else(|| anyhow::anyhow!("7z is not installed"))?;
    let output = exec::run_at(
        &tool,
        [
            std::ffi::OsStr::new("l"),
            "-slt".as_ref(),
            "--".as_ref(),
            iso_path.as_os_str(),
        ],
    )?;
    Ok(parse_7z_listing(&output))
}

/// Parses `7z l -slt` output: records follow a `----------` line, separated by blank lines.
fn parse_7z_listing(text: &str) -> Vec<(String, u64)> {
    let mut files = Vec::new();
    let mut in_records = false;
    let (mut path, mut size) = (String::new(), 0u64);

    let mut flush = |path: &mut String, size: &mut u64| {
        if !path.is_empty() {
            files.push((std::mem::take(path), *size));
        }
        *size = 0;
    };

    for line in text.lines() {
        let line = line.trim();
        if line == "----------" {
            in_records = true;
        } else if !in_records {
            continue;
        } else if let Some(p) = line.strip_prefix("Path = ") {
            path = p.to_string();
        } else if let Some(s) = line.strip_prefix("Size = ") {
            size = s.parse().unwrap_or(0);
        } else if line.is_empty() {
            flush(&mut path, &mut size);
        }
    }
    flush(&mut path, &mut size);
    files
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn listing(paths: &[(&str, u64)]) -> Vec<(String, u64)> {
        paths.iter().map(|(p, s)| (p.to_string(), *s)).collect()
    }

    #[test]
    fn detects_windows_install_media() {
        let c = classify(&listing(&[
            ("bootmgr", 1),
            ("efi/boot/bootx64.efi", 1),
            ("sources/boot.wim", 10),
            ("sources/install.wim", 5_000_000_000),
        ]));
        assert_eq!(c.image_type, ImageType::Windows);
        assert!(c.is_efi_bootable && c.is_bios_bootable && c.has_install_wim);
        assert_eq!(c.install_wim_size, 5_000_000_000);
    }

    #[test]
    fn detects_linux_persistence_flavours() {
        let ubuntu = classify(&listing(&[("casper/vmlinuz", 1), ("isolinux/isolinux.bin", 1)]));
        assert_eq!(ubuntu.image_type, ImageType::Linux);
        assert_eq!(ubuntu.persistence, Some(PersistenceKind::Casper));

        let debian = classify(&listing(&[
            ("live/filesystem.squashfs", 1),
            ("EFI/boot/bootx64.efi", 1),
        ]));
        assert_eq!(debian.persistence, Some(PersistenceKind::DebianLive));
        assert!(debian.is_efi_bootable);

        let search = classify(&listing(&[("docs/research/readme.txt", 1)]));
        assert_eq!(
            search.image_type,
            ImageType::Unknown,
            "'arch' inside a word must not match"
        );
    }

    #[test]
    fn parses_7z_listing_output() {
        let text = "Listing archive: x.iso\n\n--\nPath = x.iso\nType = Iso\n\n----------\nPath = casper/vmlinuz\nSize = 42\n\nPath = boot/grub\nSize = 0\n\n";
        let files = parse_7z_listing(text);
        assert_eq!(
            files,
            vec![("casper/vmlinuz".to_string(), 42), ("boot/grub".to_string(), 0)]
        );
    }

    #[test]
    fn reads_iso_header_and_flags_isohybrid() {
        let mut tmp = tempfile::Builder::new().suffix(".iso").tempfile().unwrap();
        let mut data = vec![0u8; 0x8000 + 2048];
        data[510] = 0x55;
        data[511] = 0xAA;
        data[0x8001..0x8006].copy_from_slice(b"CD001");
        data[0x8000 + 40..0x8000 + 40 + 6].copy_from_slice(b"MYDISK");
        tmp.write_all(&data).unwrap();

        let report = scan_image(tmp.path()).unwrap();
        assert_eq!(report.volume_label, "MYDISK");
        assert!(report.is_iso && report.is_isohybrid);
    }

    #[test]
    fn plain_images_are_raw_disks() {
        let mut tmp = tempfile::Builder::new().suffix(".img").tempfile().unwrap();
        tmp.write_all(&[0u8; 4096]).unwrap();
        let report = scan_image(tmp.path()).unwrap();
        assert_eq!(report.image_type, ImageType::RawDisk);
        assert!(!report.is_isohybrid);
    }
}
