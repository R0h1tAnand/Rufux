//! Storage Device Discovery, Inspection, and Safety
//!
//! Enumerates block devices via lsblk/sysfs, extracts disk metadata, and decides which disks may
//! be written to. The safety checks here are the last line of defence and are re-run by the
//! privileged worker, which never trusts what the GUI selected.

use std::collections::BTreeSet;
use std::fs::{self, File, OpenOptions};
use std::io::Write;
use std::os::fd::AsRawFd;
use std::os::unix::fs::{FileTypeExt, MetadataExt, OpenOptionsExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::time::Instant;

use anyhow::{anyhow, ensure, Context, Result};
use nix::sys::stat::{major, minor};
use serde::Deserialize;
use serde_json::Value;

use super::blockdev::{device_size, open_device, read_fill, AlignedBuf, OpenMode};
use super::exec::{self, check_cancel};
use super::privilege::with_invoking_user;

const SYS_CLASS_BLOCK: &str = "/sys/class/block";
const SYS_DEV_BLOCK: &str = "/sys/dev/block";

/// Mount points whose backing disk must never be written to. Includes the media a live system
/// booted from.
const CRITICAL_MOUNTS: &[&str] = &[
    "/",
    "/boot",
    "/boot/efi",
    "/boot/firmware",
    "/efi",
    "/usr",
    "/var",
    "/etc",
    "/home",
    "/opt",
    "/srv",
    "/nix",
    "/cdrom",
    "/run/live/medium",
    "/run/archiso/bootmnt",
    "/lib/live/mount/medium",
    "/run/initramfs/live",
];

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

        format!(
            "{} ({}) [{}]",
            name_str,
            format_size(self.size_bytes),
            self.path.display()
        )
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

// ---------------------------------------------------------------------------------------------
// sysfs / mount table inspection
// ---------------------------------------------------------------------------------------------

fn read_sysfs(path: &Path) -> Option<String> {
    fs::read_to_string(path).ok().map(|s| s.trim().to_string())
}

fn file_name_string(path: &Path) -> Option<String> {
    path.file_name().map(|n| n.to_string_lossy().into_owned())
}

/// Resolves `major:minor` to the kernel name of the block device (`sda2`, `dm-0`, ...).
fn name_from_dev_numbers(maj: u32, min: u32) -> Option<String> {
    file_name_string(&fs::canonicalize(format!("{}/{}:{}", SYS_DEV_BLOCK, maj, min)).ok()?)
}

/// Collects the whole-disk device(s) that physically back `name`, following partitions up to
/// their disk and stacked devices (LVM, LUKS, md) down to their members.
fn backing_disks(name: &str, out: &mut BTreeSet<String>, depth: u8) {
    if depth > 8 {
        return;
    }
    let base = Path::new(SYS_CLASS_BLOCK).join(name);

    if base.join("partition").exists() {
        if let Some(parent) = fs::canonicalize(&base)
            .ok()
            .and_then(|real| real.parent().and_then(file_name_string))
        {
            out.insert(parent);
            return;
        }
    }

    let slaves: Vec<String> = fs::read_dir(base.join("slaves"))
        .map(|rd| rd.flatten().filter_map(|e| e.file_name().into_string().ok()).collect())
        .unwrap_or_default();

    if slaves.is_empty() {
        out.insert(name.to_string());
    } else {
        for slave in slaves {
            backing_disks(&slave, out, depth + 1);
        }
    }
}

#[derive(Debug, Clone)]
struct MountEntry {
    mountpoint: String,
    major: u32,
    minor: u32,
    source: String,
}

/// Undoes the octal escaping (`\040`) used in `/proc/self/mountinfo`.
fn unescape_mount_field(field: &str) -> String {
    let bytes = field.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'\\' && i + 3 < bytes.len() && bytes[i + 1..i + 4].iter().all(|b| (b'0'..=b'7').contains(b)) {
            let value = bytes[i + 1..i + 4]
                .iter()
                .fold(0u32, |acc, b| acc * 8 + u32::from(b - b'0'));
            out.push(value as u8);
            i += 4;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn parse_mountinfo_line(line: &str) -> Option<MountEntry> {
    let (head, tail) = line.split_once(" - ")?;
    let mut fields = head.split_whitespace();
    let _id = fields.next()?;
    let _parent = fields.next()?;
    let (maj, min) = fields.next()?.split_once(':')?;
    let _root = fields.next()?;
    let mountpoint = unescape_mount_field(fields.next()?);

    let mut tail = tail.split_whitespace();
    let _fstype = tail.next()?;
    let source = unescape_mount_field(tail.next()?);

    Some(MountEntry {
        mountpoint,
        major: maj.parse().ok()?,
        minor: min.parse().ok()?,
        source,
    })
}

fn read_mounts() -> Result<Vec<MountEntry>> {
    let text = fs::read_to_string("/proc/self/mountinfo").context("cannot read /proc/self/mountinfo")?;
    Ok(text.lines().filter_map(parse_mountinfo_line).collect())
}

fn mount_device_name(entry: &MountEntry) -> Option<String> {
    if entry.source.starts_with("/dev/") {
        if let Some(name) = fs::canonicalize(&entry.source)
            .ok()
            .as_deref()
            .and_then(file_name_string)
        {
            return Some(name);
        }
    }
    if entry.major != 0 {
        return name_from_dev_numbers(entry.major, entry.minor);
    }
    None
}

fn mount_disks(entry: &MountEntry) -> BTreeSet<String> {
    let mut set = BTreeSet::new();
    if let Some(name) = mount_device_name(entry) {
        backing_disks(&name, &mut set, 0);
    }
    set
}

/// Whole-disk devices holding the running system (root, boot, home, live medium, swap, ...).
pub fn system_disks() -> Result<BTreeSet<String>> {
    let mut disks = BTreeSet::new();
    for entry in read_mounts()? {
        if CRITICAL_MOUNTS.contains(&entry.mountpoint.as_str()) {
            disks.extend(mount_disks(&entry));
        }
    }

    if let Ok(swaps) = fs::read_to_string("/proc/swaps") {
        for line in swaps.lines().skip(1) {
            if let Some(src) = line.split_whitespace().next() {
                let src = unescape_mount_field(src);
                if let Some(name) = fs::canonicalize(&src).ok().as_deref().and_then(file_name_string) {
                    backing_disks(&name, &mut disks, 0);
                }
            }
        }
    }
    Ok(disks)
}

/// Whole-disk devices backing the file system that has the given `st_dev`.
fn disks_of_dev(dev: u64) -> BTreeSet<String> {
    let (maj, min) = (major(dev) as u32, minor(dev) as u32);
    let mut set = BTreeSet::new();

    let from_mounts = read_mounts()
        .ok()
        .and_then(|mounts| mounts.into_iter().find(|e| e.major == maj && e.minor == min))
        .and_then(|e| mount_device_name(&e));

    if let Some(name) = from_mounts.or_else(|| name_from_dev_numbers(maj, min)) {
        backing_disks(&name, &mut set, 0);
    }
    set
}

/// True for USB-attached, removable, SD/MMC card and loop devices.
fn is_removable_disk(name: &str) -> bool {
    let base = Path::new(SYS_CLASS_BLOCK).join(name);
    if name.starts_with("loop") || read_sysfs(&base.join("removable")).as_deref() == Some("1") {
        return true;
    }
    if matches!(
        read_sysfs(&base.join("device/type")).as_deref(),
        Some("SD") | Some("MMC")
    ) {
        return true;
    }
    fs::canonicalize(&base)
        .map(|real| {
            real.components().any(|c| {
                let s = c.as_os_str().to_string_lossy();
                s.len() > 3 && s.starts_with("usb") && s[3..].chars().all(|ch| ch.is_ascii_digit())
            })
        })
        .unwrap_or(false)
}

// ---------------------------------------------------------------------------------------------
// Enumeration (unprivileged)
// ---------------------------------------------------------------------------------------------

#[derive(Deserialize)]
struct LsblkRoot {
    blockdevices: Vec<LsblkDevice>,
}

// Older util-linux versions emit numbers/booleans as strings, so those columns stay `Value`.
#[derive(Deserialize)]
struct LsblkDevice {
    name: String,
    kname: Option<String>,
    path: Option<String>,
    model: Option<String>,
    vendor: Option<String>,
    size: Option<Value>,
    rm: Option<Value>,
    hotplug: Option<Value>,
    tran: Option<String>,
    #[serde(rename = "type")]
    dev_type: Option<String>,
    mountpoint: Option<String>,
    mountpoints: Option<Vec<Option<String>>>,
    label: Option<String>,
    fstype: Option<String>,
    children: Option<Vec<LsblkDevice>>,
}

fn value_to_bool(v: &Option<Value>) -> bool {
    match v {
        Some(Value::Bool(b)) => *b,
        Some(Value::Number(n)) => n.as_u64() == Some(1),
        Some(Value::String(s)) => s == "1" || s.eq_ignore_ascii_case("true"),
        _ => false,
    }
}

fn value_to_u64(v: &Option<Value>) -> u64 {
    match v {
        Some(Value::Number(n)) => n.as_u64().unwrap_or(0),
        Some(Value::String(s)) => s.trim().parse().unwrap_or(0),
        _ => 0,
    }
}

/// Enumerate removable USB storage devices. Disks holding the running system are never listed.
pub fn enumerate_devices(list_all_drives: bool) -> Result<Vec<DeviceInfo>> {
    let json = exec::run("lsblk", ["--json", "-b", "-O"])?;
    let root: LsblkRoot = serde_json::from_str(&json).context("unexpected lsblk output")?;
    let system = system_disks().context("cannot determine which disks hold the running system")?;
    let mut devices = Vec::new();

    for dev in root.blockdevices {
        if dev.dev_type.as_deref() != Some("disk") {
            continue;
        }

        let name = dev.kname.clone().unwrap_or_else(|| dev.name.clone());
        if name.starts_with("loop") || name.starts_with("zram") || name.starts_with("ram") {
            continue;
        }
        if system.contains(&name) {
            continue;
        }

        let size_bytes = value_to_u64(&dev.size);
        let bus = dev.tran.as_deref().unwrap_or("").to_lowercase();
        let is_removable =
            value_to_bool(&dev.rm) || value_to_bool(&dev.hotplug) || bus == "usb" || is_removable_disk(&name);

        if size_bytes == 0 || (!is_removable && !list_all_drives) {
            continue;
        }

        let mut partitions = Vec::new();
        collect_partitions(&dev, &mut partitions);

        devices.push(DeviceInfo {
            path: dev
                .path
                .as_deref()
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from(format!("/dev/{}", name))),
            vendor: dev.vendor.as_deref().unwrap_or("").trim().to_string(),
            model: dev.model.as_deref().unwrap_or("").trim().to_string(),
            name,
            size_bytes,
            is_removable,
            bus,
            partitions,
        });
    }

    Ok(devices)
}

fn collect_partitions(dev: &LsblkDevice, partitions: &mut Vec<PartitionInfo>) {
    for child in dev.children.iter().flatten() {
        let child_name = child.kname.clone().unwrap_or_else(|| child.name.clone());
        let mount = child
            .mountpoint
            .clone()
            .or_else(|| child.mountpoints.iter().flatten().flatten().next().cloned());

        partitions.push(PartitionInfo {
            path: child
                .path
                .clone()
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from(format!("/dev/{}", child_name))),
            name: child_name,
            size_bytes: value_to_u64(&child.size),
            fstype: child.fstype.clone(),
            mountpoint: mount,
            label: child.label.clone(),
        });

        collect_partitions(child, partitions);
    }
}

// ---------------------------------------------------------------------------------------------
// Validation (re-run by the privileged worker)
// ---------------------------------------------------------------------------------------------

/// A block device that passed validation, addressed by its canonical `/dev` path.
#[derive(Debug, Clone)]
pub struct TargetDevice {
    pub path: PathBuf,
    pub name: String,
    pub size_bytes: u64,
}

/// Checks that `path` is a whole-disk block device node with media present.
pub fn validate_block_disk(path: &Path) -> Result<TargetDevice> {
    let canonical = fs::canonicalize(path).with_context(|| format!("cannot resolve {}", path.display()))?;
    ensure!(
        canonical.parent() == Some(Path::new("/dev")),
        "{} is not a device node under /dev",
        canonical.display()
    );
    let meta = fs::metadata(&canonical).with_context(|| format!("cannot stat {}", canonical.display()))?;
    ensure!(
        meta.file_type().is_block_device(),
        "{} is not a block device",
        canonical.display()
    );

    let name = file_name_string(&canonical).ok_or_else(|| anyhow!("invalid device path"))?;
    let sys = Path::new(SYS_CLASS_BLOCK).join(&name);
    ensure!(sys.exists(), "{} is not known to the kernel", name);
    ensure!(
        !sys.join("partition").exists(),
        "{} is a partition; select the whole disk instead",
        canonical.display()
    );

    let sectors: u64 = read_sysfs(&sys.join("size")).and_then(|s| s.parse().ok()).unwrap_or(0);
    let size_bytes = sectors * 512;
    ensure!(size_bytes > 0, "{} reports no media", canonical.display());

    Ok(TargetDevice {
        path: canonical,
        name,
        size_bytes,
    })
}

/// Validates a device as a destination for destructive writes.
///
/// Refuses partitions, disks that hold the running system, and (unless `allow_fixed`) anything
/// that is not removable, USB, SD/MMC or a loop device.
pub fn validate_target(path: &Path, allow_fixed: bool) -> Result<TargetDevice> {
    let target = validate_block_disk(path)?;

    let system = system_disks().context("cannot determine which disks hold the running system")?;
    ensure!(
        !system.contains(&target.name),
        "refusing to modify {}: it holds the running system",
        target.path.display()
    );
    ensure!(
        allow_fixed || is_removable_disk(&target.name),
        "{} is not a removable or USB drive (enable 'List USB Hard Drives' to override)",
        target.path.display()
    );
    Ok(target)
}

/// An image opened on behalf of the invoking user.
pub struct ImageFile {
    pub file: File,
    pub path: PathBuf,
    pub size_bytes: u64,
}

impl ImageFile {
    /// A path other processes (7z) can use to read exactly the file validated here, immune to
    /// the original path being swapped for a symlink afterwards.
    pub fn proc_path(&self) -> PathBuf {
        PathBuf::from(format!("/proc/{}/fd/{}", std::process::id(), self.file.as_raw_fd()))
    }
}

/// Opens an image with the invoking user's permissions and makes sure it is a regular file that
/// does not live on the device about to be erased.
pub fn open_image(path: &Path, target: &TargetDevice) -> Result<ImageFile> {
    let (file, canonical) = with_invoking_user(|| {
        let canonical = fs::canonicalize(path)?;
        let file = File::open(&canonical)?;
        Ok((file, canonical))
    })
    .with_context(|| format!("cannot open image {}", path.display()))?;

    let meta = file.metadata()?;
    ensure!(meta.is_file(), "{} is not a regular file", canonical.display());
    ensure!(
        !disks_of_dev(meta.dev()).contains(&target.name),
        "the image {} is stored on the target device {}",
        canonical.display(),
        target.path.display()
    );
    Ok(ImageFile {
        file,
        path: canonical,
        size_bytes: meta.len(),
    })
}

/// Unmounts every file system located on the target disk, deepest mount first.
pub fn unmount_all(target: &TargetDevice) -> Result<Vec<String>> {
    let mut mounted: Vec<String> = read_mounts()?
        .into_iter()
        .filter(|e| mount_disks(e).contains(&target.name))
        .map(|e| e.mountpoint)
        .collect();
    mounted.sort_by_key(|m| std::cmp::Reverse(m.len()));
    mounted.dedup();

    for mountpoint in &mounted {
        exec::run("umount", ["--", mountpoint.as_str()]).with_context(|| format!("cannot unmount {}", mountpoint))?;
    }
    Ok(mounted)
}

/// Dumps a physical device to an image file (matching Rufus "..." dump button).
///
/// `progress` receives the completed fraction and the throughput in MB/s.
pub fn dump_device_to_image(
    device_path: &Path,
    output_path: &Path,
    cancel: &AtomicBool,
    mut progress: impl FnMut(f32, f32),
) -> Result<()> {
    let target = validate_block_disk(device_path)?;
    ensure!(!output_path.starts_with("/dev"), "refusing to write the dump into /dev");

    let mut src = open_device(&target.path, OpenMode::READ)?;
    let total = device_size(&mut src.file)?;

    let mut dst = with_invoking_user(|| {
        OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(output_path)
    })
    .with_context(|| format!("cannot create {}", output_path.display()))?;

    let mut buf = AlignedBuf::new(4 * 1024 * 1024);
    let (mut done, started) = (0u64, Instant::now());
    let mut last_report = Instant::now();

    let result = (|| -> Result<()> {
        loop {
            check_cancel(cancel)?;
            let n = read_fill(&mut src.file, buf.as_mut_slice()).context("read error")?;
            if n == 0 {
                break;
            }
            dst.write_all(&buf.as_slice()[..n]).context("write error")?;
            done += n as u64;

            if last_report.elapsed().as_millis() >= 200 && total > 0 {
                let mb_s = done as f32 / (1024.0 * 1024.0) / started.elapsed().as_secs_f32().max(0.001);
                progress((done as f32 / total as f32).min(1.0), mb_s);
                last_report = Instant::now();
            }
        }
        dst.sync_all().context("cannot flush dump file")?;
        Ok(())
    })();

    if let Err(e) = result {
        drop(dst);
        let _ = with_invoking_user(|| fs::remove_file(output_path));
        return Err(e);
    }

    progress(1.0, 0.0);
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

    #[test]
    fn mountinfo_lines_are_parsed_and_unescaped() {
        let line = "36 35 8:2 / /media/my\\040usb rw,relatime shared:1 - ext4 /dev/sdb2 rw";
        let e = parse_mountinfo_line(line).unwrap();
        assert_eq!((e.major, e.minor), (8, 2));
        assert_eq!(e.mountpoint, "/media/my usb");
        assert_eq!(e.source, "/dev/sdb2");
        assert!(parse_mountinfo_line("garbage").is_none());
    }

    #[test]
    fn lsblk_accepts_legacy_string_values() {
        let json = r#"{"blockdevices":[{"name":"sdb","type":"disk","size":"8000000000","rm":"1","hotplug":"0","tran":"usb"}]}"#;
        let root: LsblkRoot = serde_json::from_str(json).unwrap();
        let d = &root.blockdevices[0];
        assert!(value_to_bool(&d.rm));
        assert!(!value_to_bool(&d.hotplug));
        assert_eq!(value_to_u64(&d.size), 8_000_000_000);
    }

    #[test]
    fn enumeration_never_lists_the_system_disk() {
        let system = system_disks().unwrap();
        let listed = enumerate_devices(true).expect("lsblk available");
        for dev in &listed {
            println!("listed: {}", dev.display_label());
            assert!(!system.contains(&dev.name), "{} holds the running system", dev.name);
        }
    }

    #[test]
    fn rejects_non_device_paths() {
        assert!(validate_target(Path::new("/etc/passwd"), true).is_err());
        assert!(
            validate_target(Path::new("/dev/null"), true).is_err(),
            "character device"
        );
        assert!(validate_target(Path::new("/nonexistent"), true).is_err());
    }

    #[test]
    fn running_system_disks_are_detected() {
        // Whatever machine runs the tests has a root mount; it must resolve to some disk.
        let disks = system_disks().expect("mountinfo readable");
        assert!(
            !disks.is_empty()
                || fs::read_to_string("/proc/self/mountinfo")
                    .unwrap()
                    .contains(" overlay ")
        );
    }
}
