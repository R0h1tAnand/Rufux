//! Core Formatting, Partitioning, and Image Flashing Engine
//!
//! Handles both direct DD image streaming and full ISO extraction mode
//! with MBR/GPT partitioning, UEFI:NTFS creation, and WUE answer file injection.
//!
//! This module performs destructive operations and is only ever run as root (through the
//! `pkexec` worker). Every external command is checked for failure; any error aborts the run
//! and is reported once, at the end of [`execute_burn`].

use std::ffi::OsStr;
use std::fs::{self, OpenOptions};
use std::io::{self, Read, Write};
use std::os::unix::fs::{FileExt, FileTypeExt};
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::time::{Duration, Instant};

use anyhow::{anyhow, bail, ensure, Context, Result};
use serde::{Deserialize, Serialize};
use tempfile::TempDir;

use super::autorun;
use super::badblocks::check_bad_blocks;
use super::bios_boot::{self, BootStep};
use super::blockdev::{clear_direct, open_device, read_fill, AlignedBuf, Device, OpenMode};
use super::device::{format_size, open_image, unmount_all, validate_target, ImageFile, TargetDevice};
use super::exec::{self, check_cancel, Canceled};
use super::freedos;
use super::iso_scanner::{scan_image, ImageType, IsoReport, PersistenceKind};
use super::layout::{
    mkfs_args, parted_args, partition_path, plan_partitions, sanitize_label, FileSystem, Layout, PartitionScheme,
    TargetSystem, MIB,
};
use super::privilege::with_invoking_user;
use super::uefi_md5sum;
use super::uefi_ntfs::write_uefi_ntfs;
use super::wue::{self, WueOptions};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum BurnMode {
    IsoMode,
    DdMode,
}

/// What the drive should boot, chosen in "Boot selection".
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum BootKind {
    /// Write the selected disk or ISO image.
    #[default]
    Image,
    /// Format only; the drive is not made bootable.
    NonBootable,
    /// An embedded FreeDOS system on FAT32 (BIOS, MBR).
    FreeDos,
    /// An empty Syslinux boot loader on FAT32 (BIOS, MBR).
    Syslinux,
    /// The UEFI:NTFS helper only, so files copied later can boot from NTFS or exFAT.
    UefiNtfs,
}

impl BootKind {
    /// Checks that the rest of the options can produce this kind of drive.
    pub fn validate(self, opts: &BurnOptions) -> Result<()> {
        let fat32_mbr = || {
            ensure!(
                opts.file_system == FileSystem::Fat32 && opts.partition_scheme == PartitionScheme::Mbr,
                "this boot selection needs the MBR partition scheme and the FAT32 file system"
            );
            Ok(())
        };
        match self {
            Self::Image => Ok(()),
            _ if opts.burn_mode == BurnMode::DdMode => bail!("this boot selection cannot be used in DD mode"),
            Self::NonBootable => Ok(()),
            Self::FreeDos | Self::Syslinux => fat32_mbr(),
            Self::UefiNtfs => {
                ensure!(
                    matches!(opts.file_system, FileSystem::Ntfs | FileSystem::ExFat),
                    "UEFI:NTFS needs the NTFS or exFAT file system"
                );
                Ok(())
            }
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BurnOptions {
    pub device_path: PathBuf,
    pub image_path: Option<PathBuf>,
    pub burn_mode: BurnMode,
    pub partition_scheme: PartitionScheme,
    pub target_system: TargetSystem,
    pub file_system: FileSystem,
    pub volume_label: String,
    pub quick_format: bool,
    /// Cluster size in bytes; `None` lets the file system tool choose.
    pub cluster_size: Option<u32>,
    pub persistence_size_mb: u64,
    /// Number of bad-block test passes (0 disables the test).
    pub bad_blocks_passes: u32,
    /// Allow non-removable disks (the "List USB Hard Drives" option).
    pub allow_fixed_drives: bool,
    pub wue_options: Option<WueOptions>,
    pub wue_username: String,
    pub wue_locale: String,
    /// Write `autorun.inf` and an icon so file managers show the volume label and icon.
    #[serde(default)]
    pub extended_label: bool,
    /// Start the first MBR partition at sector 63, as old BIOSes expect.
    #[serde(default)]
    pub legacy_bios_fixes: bool,
    /// Install `uefi-md5sum` so the image verifies its `md5sum.txt` at every UEFI boot.
    #[serde(default)]
    pub uefi_media_validation: bool,
    #[serde(default)]
    pub boot_kind: BootKind,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum BurnProgress {
    Status(String),
    Progress { percent: f32, speed_mb_s: f32 },
    Log(String),
    Finished,
    Error(String),
}

const DD_CHUNK: usize = 4 * 1024 * 1024;
const FOUR_GIB: u64 = 4 * 1024 * 1024 * 1024;

/// Progress reporting plus cancellation, mapping stage fractions onto a sub-range of the bar.
struct Run<'a> {
    emit: &'a mut dyn FnMut(BurnProgress),
    cancel: &'a AtomicBool,
    lo: f32,
    hi: f32,
}

impl Run<'_> {
    fn range(&mut self, lo: f32, hi: f32) {
        self.lo = lo;
        self.hi = hi;
    }

    fn map(&self, fraction: f32) -> f32 {
        let f = if fraction.is_finite() {
            fraction.clamp(0.0, 1.0)
        } else {
            0.0
        };
        self.lo + f * (self.hi - self.lo)
    }

    fn log(&mut self, text: impl Into<String>) {
        (self.emit)(BurnProgress::Log(text.into()));
    }

    fn progress(&mut self, fraction: f32, speed_mb_s: f32) {
        let percent = self.map(fraction);
        (self.emit)(BurnProgress::Progress { percent, speed_mb_s });
    }

    /// Announces a stage: status text, log line and a progress position.
    fn stage(&mut self, fraction: f32, text: impl Into<String>) {
        let text = text.into();
        (self.emit)(BurnProgress::Status(text.clone()));
        (self.emit)(BurnProgress::Log(text));
        self.progress(fraction, 0.0);
    }

    fn check(&self) -> Result<()> {
        check_cancel(self.cancel)
    }
}

/// Executes the flash/format operation, streaming progress through the callback.
///
/// Exactly one terminal message ([`BurnProgress::Finished`] or [`BurnProgress::Error`]) is sent.
pub fn execute_burn<F>(opts: &BurnOptions, iso_report: Option<&IsoReport>, cancel: &AtomicBool, mut callback: F)
where
    F: FnMut(BurnProgress),
{
    let mut run = Run {
        emit: &mut callback,
        cancel,
        lo: 0.0,
        hi: 1.0,
    };

    match run_burn(&mut run, opts, iso_report) {
        Ok(()) => {
            run.range(0.0, 1.0);
            (run.emit)(BurnProgress::Status("Operation completed successfully!".to_string()));
            run.progress(1.0, 0.0);
            (run.emit)(BurnProgress::Finished);
        }
        Err(e) if e.is::<Canceled>() => (run.emit)(BurnProgress::Error("Operation canceled by user".to_string())),
        Err(e) => (run.emit)(BurnProgress::Error(format!("{:#}", e))),
    }
}

fn run_burn(run: &mut Run, opts: &BurnOptions, iso_report: Option<&IsoReport>) -> Result<()> {
    let target = validate_target(&opts.device_path, opts.allow_fixed_drives)?;
    opts.boot_kind.validate(opts)?;
    let image_path = opts.image_path.as_deref().filter(|_| opts.boot_kind == BootKind::Image);
    let image = image_path.map(|p| open_image(p, &target)).transpose()?;

    run.log(format!(
        "Starting operation on {} ({})",
        target.path.display(),
        format_size(target.size_bytes)
    ));

    // Everything that can be validated without touching the device is validated up front.
    let wue_xml = match (&opts.wue_options, opts.burn_mode) {
        (Some(w), BurnMode::IsoMode) => Some(wue::generate_autounattend_xml(w, &opts.wue_username, &opts.wue_locale)?),
        _ => None,
    };

    if opts.bad_blocks_passes > 0 {
        run.range(0.0, 0.4);
        run_bad_blocks(run, opts, &target)?;
        run.range(0.4, 1.0);
    }

    match opts.burn_mode {
        BurnMode::DdMode => {
            let image = image.ok_or_else(|| anyhow!("No image file specified for DD mode"))?;
            run_dd(run, &target, &image)
        }
        BurnMode::IsoMode => {
            let scanned;
            let report = match (iso_report, &image) {
                (Some(r), _) => Some(r),
                (None, Some(img)) => {
                    scanned =
                        with_invoking_user(|| scan_image(&img.path).map_err(|e| io::Error::other(e.to_string()))).ok();
                    scanned.as_ref()
                }
                (None, None) => None,
            };
            run_iso(run, opts, &target, image.as_ref(), report, wue_xml.as_deref())
        }
    }
}

fn run_bad_blocks(run: &mut Run, opts: &BurnOptions, target: &TargetDevice) -> Result<()> {
    let passes = opts.bad_blocks_passes.clamp(1, 4);
    run.stage(0.0, format!("Checking device for bad blocks ({} pass(es))...", passes));
    unmount_all(target)?;

    let (lo, hi) = (run.lo, run.hi);
    let cancel = run.cancel;
    let emit = &mut *run.emit;
    let report = check_bad_blocks(&target.path, passes, cancel, |pass, frac, msg| {
        let overall = ((pass - 1) as f32 + frac) / passes as f32;
        emit(BurnProgress::Status(format!("Pass {}/{}: {}", pass, passes, msg)));
        emit(BurnProgress::Progress {
            percent: lo + overall * (hi - lo),
            speed_mb_s: 0.0,
        });
    })?;

    run.log(format!(
        "Bad blocks check finished: {} bad block(s) ({} read, {} write, {} corruption)",
        report.bad_block_count, report.read_errors, report.write_errors, report.corruption_errors
    ));
    ensure!(
        report.bad_block_count == 0,
        "the device has {} bad block(s) and may be counterfeit or failing; nothing was written",
        report.bad_block_count
    );
    Ok(())
}

// ---------------------------------------------------------------------------------------------
// DD mode
// ---------------------------------------------------------------------------------------------

fn run_dd(run: &mut Run, target: &TargetDevice, image: &ImageFile) -> Result<()> {
    ensure!(
        image.size_bytes <= target.size_bytes,
        "the image ({}) is larger than the device ({})",
        format_size(image.size_bytes),
        format_size(target.size_bytes)
    );

    run.stage(0.0, "Unmounting partitions...");
    unmount_all(target)?;
    run.check()?;

    run.stage(0.01, "Opening raw device for streaming...");
    run.log(format!(
        "Direct streaming {} -> {}",
        image.path.display(),
        target.path.display()
    ));
    let mut dst = open_device(&target.path, OpenMode::WRITE)
        .with_context(|| format!("cannot open {} for writing", target.path.display()))?;
    if !dst.direct {
        run.log("Direct I/O is not available; falling back to buffered writes");
    }

    let mut src = image.file.try_clone()?;
    let (lo, hi) = (run.lo, run.hi);
    let emit = &mut *run.emit;
    write_raw(&mut src, image.size_bytes, &mut dst, run.cancel, |fraction, speed| {
        let pct = lo + 0.02 * (hi - lo) + fraction * 0.98 * (hi - lo);
        emit(BurnProgress::Status(format!(
            "Writing raw image... {:.1}% ({:.1} MB/s)",
            fraction * 100.0,
            speed
        )));
        emit(BurnProgress::Progress {
            percent: pct,
            speed_mb_s: speed,
        });
    })?;

    run.stage(0.99, "Flushing device write caches (sync)...");
    nix::unistd::sync();
    Ok(())
}

/// Streams `src` onto the device in 4 MiB chunks and flushes it. The final partial chunk is
/// written without `O_DIRECT`, which would otherwise require sector-aligned lengths.
pub(crate) fn write_raw(
    src: &mut impl Read,
    total: u64,
    dst: &mut Device,
    cancel: &AtomicBool,
    mut progress: impl FnMut(f32, f32),
) -> Result<()> {
    let mut buf = AlignedBuf::new(DD_CHUNK);
    let (started, mut last_report) = (Instant::now(), Instant::now());
    let mut written = 0u64;

    loop {
        check_cancel(cancel)?;
        let n = read_fill(src, buf.as_mut_slice()).context("read error on the image")?;
        if n == 0 {
            break;
        }
        if n < DD_CHUNK && dst.direct {
            clear_direct(&dst.file)?;
            dst.direct = false;
        }
        dst.file
            .write_all(&buf.as_slice()[..n])
            .context("write error on the device")?;
        written += n as u64;

        if last_report.elapsed() >= Duration::from_millis(200) {
            let speed = written as f32 / (1024.0 * 1024.0) / started.elapsed().as_secs_f32().max(0.001);
            let fraction = if total > 0 {
                (written as f32 / total as f32).min(1.0)
            } else {
                0.0
            };
            progress(fraction, speed);
            last_report = Instant::now();
        }
    }

    dst.file.sync_all().context("cannot flush the device")?;
    progress(1.0, 0.0);
    Ok(())
}

// ---------------------------------------------------------------------------------------------
// ISO extraction mode
// ---------------------------------------------------------------------------------------------

/// A mounted partition that is always unmounted again, and whose mount directory is never
/// deleted while it might still hold a mounted file system.
struct Mounted {
    dir: Option<TempDir>,
    active: bool,
}

impl Mounted {
    fn mount(partition: &str) -> Result<Self> {
        let dir = tempfile::Builder::new()
            .prefix("rufux-")
            .tempdir()
            .context("cannot create a mount directory")?;
        let dir_str = dir.path().to_string_lossy().into_owned();
        exec::run("mount", ["-o", "nosuid,nodev,noexec", partition, dir_str.as_str()])
            .with_context(|| format!("cannot mount {}", partition))?;
        Ok(Self {
            dir: Some(dir),
            active: true,
        })
    }

    fn path(&self) -> &Path {
        self.dir.as_ref().expect("mount directory exists while mounted").path()
    }

    fn unmount(&mut self) -> Result<()> {
        if self.active {
            exec::run("umount", [self.path().as_os_str()])?;
            self.active = false;
        }
        Ok(())
    }
}

impl Drop for Mounted {
    fn drop(&mut self) {
        if self.active {
            let path = self.path().to_path_buf();
            if exec::run("umount", [path.as_os_str()]).is_err()
                && exec::run("umount", [OsStr::new("-l"), path.as_os_str()]).is_err()
            {
                // Still mounted: leak the directory rather than let its removal recurse into the volume.
                if let Some(dir) = self.dir.take() {
                    let _ = dir.keep();
                }
            }
        }
    }
}

/// The old exfat-utils package installs `mkfs.exfat` as a link to `mkexfatfs` and labels with `-n`;
/// exfatprogs (the maintained one) uses `-L`.
fn is_legacy_exfat_utils() -> bool {
    exec::find_tool("mkfs.exfat")
        .and_then(|p| fs::canonicalize(p).ok())
        .and_then(|p| p.file_name().map(|n| n == "mkexfatfs"))
        .unwrap_or(false)
}

fn wait_for_node(path: &str) -> Result<()> {
    for _ in 0..150 {
        if fs::metadata(path)
            .map(|m| m.file_type().is_block_device())
            .unwrap_or(false)
        {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(100));
    }
    bail!("the kernel did not create {} after partitioning", path)
}

/// Zeroes the first and last MiB of the disk so no stale partition tables or metadata survive.
fn zero_disk_edges(target: &TargetDevice) -> Result<()> {
    let dev = OpenOptions::new().write(true).open(&target.path)?;
    let zeros = vec![0u8; MIB as usize];
    let head = MIB.min(target.size_bytes);
    dev.write_all_at(&zeros[..head as usize], 0)?;
    if target.size_bytes > 2 * MIB {
        let start = target.size_bytes - MIB;
        dev.write_all_at(&zeros, start)?;
    }
    dev.sync_all()?;
    Ok(())
}

/// Parses an `NN%` progress token from 7-Zip's `-bsp1` output.
fn parse_percent(line: &str) -> Option<f32> {
    let line = line.trim_start_matches(|c: char| c.is_whitespace() || c.is_control());
    let digits: String = line.chars().take_while(char::is_ascii_digit).collect();
    if digits.is_empty() || !line[digits.len()..].starts_with('%') {
        return None;
    }
    digits.parse::<f32>().ok().map(|p| (p / 100.0).min(1.0))
}

fn run_iso(
    run: &mut Run,
    opts: &BurnOptions,
    target: &TargetDevice,
    image: Option<&ImageFile>,
    report: Option<&IsoReport>,
    wue_xml: Option<&str>,
) -> Result<()> {
    let fs_kind = opts.file_system;
    let scheme = opts.partition_scheme;

    // ---- pre-flight: everything that can fail without having touched the device yet ----
    exec::require_tools(&["parted", "wipefs", "mount", "umount", fs_kind.mkfs_tool()])?;
    let seven_zip = match image {
        Some(_) => Some(
            exec::find_any_tool(&["7z", "7zz"])
                .ok_or_else(|| anyhow!("required tool '7z' (p7zip) is not installed"))?,
        ),
        None => None,
    };

    match opts.boot_kind {
        BootKind::FreeDos => exec::require_tools(&["ms-sys"])?,
        BootKind::Syslinux => exec::require_tools(&["syslinux"])?,
        _ => {}
    }

    let use_uefi_ntfs = opts.boot_kind == BootKind::UefiNtfs
        || (image.is_some()
            && matches!(fs_kind, FileSystem::Ntfs | FileSystem::ExFat)
            && report.is_none_or(|r| r.is_efi_bootable));

    let persistence: Option<PersistenceKind> = if opts.persistence_size_mb > 0 {
        let kind = report
            .and_then(|r| r.persistence)
            .ok_or_else(|| anyhow!("this image does not support a persistent partition"))?;
        ensure!(
            !use_uefi_ntfs,
            "a persistent partition cannot be combined with UEFI:NTFS"
        );
        Some(kind)
    } else {
        None
    };

    if fs_kind == FileSystem::Fat32 {
        if let Some(r) = report.filter(|r| r.has_install_wim && r.install_wim_size >= FOUR_GIB) {
            bail!(
                "this image contains a {} install file, which does not fit on FAT32 (4 GiB limit); choose NTFS",
                format_size(r.install_wim_size)
            );
        }
    }
    if opts.target_system == TargetSystem::Uefi && fs_kind == FileSystem::Ntfs && image.is_some() && !use_uefi_ntfs {
        run.log("Warning: the image is not EFI bootable; UEFI firmware may not be able to start it");
    }

    let layout = match persistence {
        Some(_) => Layout::Persistence {
            size_mib: opts.persistence_size_mb,
        },
        None if use_uefi_ntfs => Layout::UefiNtfs,
        None => Layout::Single,
    };
    let plan = plan_partitions(target.size_bytes, scheme, layout)?;
    if let (Some(report), true) = (report, wue_xml.is_some()) {
        if report.image_type != ImageType::Windows {
            run.log("Warning: Windows customisation was requested but the image does not look like Windows media");
        }
    }
    if opts.wue_options.is_some_and(|w| w.create_local_account) && wue_xml.is_some() {
        run.log("Note: the Windows local account is created without a password; set one after installation");
    }

    let dev = target.path.to_string_lossy().into_owned();
    let part1 = partition_path(&dev, 1);
    let part2 = partition_path(&dev, 2);
    let label = sanitize_label(fs_kind, &opts.volume_label);

    // ---- destructive part ----
    run.stage(0.02, "Unmounting partitions...");
    for mountpoint in unmount_all(target)? {
        run.log(format!("Unmounted {}", mountpoint));
    }
    run.check()?;

    run.stage(0.05, "Clearing partition table...");
    exec::run("wipefs", ["--all", "--force", dev.as_str()])?;
    zero_disk_edges(target).context("cannot erase the old partition table")?;
    run.check()?;

    run.stage(0.08, format!("Creating {} partition table...", scheme));
    if opts.legacy_bios_fixes && scheme == PartitionScheme::Mbr {
        run.log("Aligning the first partition to sector 63 for old BIOSes");
    }
    let args = parted_args(&dev, scheme, fs_kind, layout, &plan, opts.legacy_bios_fixes);
    run.log(format!("parted {}", args.join(" ")));
    exec::run("parted", &args)?;

    let _ = exec::run("udevadm", ["settle"]);
    wait_for_node(&part1)?;
    if plan.len() > 1 {
        wait_for_node(&part2)?;
    }
    run.check()?;

    let bios_files = matches!(opts.boot_kind, BootKind::FreeDos | BootKind::Syslinux);
    if scheme == PartitionScheme::Mbr && (image.is_some() || bios_files) {
        match bios_boot::install_mbr_code(target) {
            Ok(true) => run.log("Installed MBR boot code"),
            Ok(false) => run.log("Warning: no syslinux MBR boot code found; BIOS booting may not work"),
            Err(e) => run.log(format!("Warning: could not install MBR boot code: {:#}", e)),
        }
    }

    run.stage(0.12, format!("Formatting partition 1 as {}...", fs_kind));
    run.log(format!("Formatting {} as {} with label '{}'", part1, fs_kind, label));
    let mut mkfs = mkfs_args(fs_kind, &label, opts.cluster_size, opts.quick_format, &part1);
    if fs_kind == FileSystem::ExFat && is_legacy_exfat_utils() {
        mkfs.iter_mut()
            .filter(|a| *a == "-L")
            .for_each(|a| *a = "-n".to_string());
    }
    exec::run(fs_kind.mkfs_tool(), mkfs)?;
    run.check()?;

    if use_uefi_ntfs {
        run.stage(0.16, "Writing the UEFI:NTFS boot partition...");
        write_uefi_ntfs(Path::new(&part2)).context("cannot write the UEFI:NTFS partition")?;
    }
    if let Some(kind) = persistence {
        run.stage(
            0.16,
            format!("Formatting the persistence partition ({})...", kind.partition_label()),
        );
        let args = mkfs_args(FileSystem::Ext4, kind.partition_label(), None, true, &part2);
        exec::run(FileSystem::Ext4.mkfs_tool(), args)?;
    }
    run.check()?;

    let extract = match (image, seven_zip) {
        (Some(image), Some(seven_zip)) => Some((image, seven_zip)),
        _ => None,
    };
    let extracting = extract.is_some();
    if !extracting && !bios_files && !opts.extended_label {
        return Ok(());
    }

    run.stage(0.18, "Mounting target partition...");
    let mut mounted = Mounted::mount(&part1)?;
    run.log(format!("Mounted {} at {}", part1, mounted.path().display()));

    if let Some((image, seven_zip)) = extract {
        run.stage(0.20, "Extracting ISO files...");
        let args = [
            "x".into(),
            "-y".into(),
            "-aoa".into(),
            "-bsp1".into(),
            "-bso0".into(),
            format!("-o{}", mounted.path().display()),
            "--".into(),
            image.proc_path().to_string_lossy().into_owned(),
        ];
        let (lo, hi) = (run.lo, run.hi);
        let emit = &mut *run.emit;
        exec::run_streaming(&seven_zip, args, run.cancel, |line| {
            if let Some(fraction) = parse_percent(line) {
                emit(BurnProgress::Status(format!(
                    "Extracting ISO files... {:.0}%",
                    fraction * 100.0
                )));
                let overall = 0.20 + fraction * 0.68;
                emit(BurnProgress::Progress {
                    percent: lo + overall * (hi - lo),
                    speed_mb_s: 0.0,
                });
            }
        })
        .context("extracting the image failed")?;
    }

    if let Some(xml) = wue_xml {
        run.stage(0.90, "Applying Windows User Experience customizations...");
        wue::write_answer_file(mounted.path(), xml)?;
    }

    if opts.uefi_media_validation && extracting {
        match uefi_md5sum::install(mounted.path()) {
            Ok(uefi_md5sum::Outcome::Installed(archs)) => {
                run.log(format!("Installed UEFI media validation ({})", archs.join(", ")))
            }
            Ok(uefi_md5sum::Outcome::Skipped(why)) => run.log(format!("UEFI media validation not installed: {why}")),
            Err(e) => bail!("cannot install UEFI media validation: {:#}", e),
        }
    }

    if opts.extended_label {
        run.stage(0.91, "Writing label and icon files...");
        autorun::write_extended_label(mounted.path(), &label)?;
    }

    let mut boot_step = BootStep::None;
    match opts.boot_kind {
        BootKind::FreeDos => {
            run.stage(0.90, "Copying FreeDOS files...");
            freedos::install(mounted.path())?;
            boot_step = BootStep::FreeDosBootRecord;
        }
        BootKind::Syslinux => {
            run.stage(0.90, "Preparing Syslinux...");
            boot_step = bios_boot::prepare_bare_syslinux(mounted.path())?;
        }
        _ => {}
    }

    let boot_step = if boot_step != BootStep::None {
        boot_step
    } else if scheme == PartitionScheme::Mbr && extracting {
        run.stage(0.92, "Preparing BIOS boot files...");
        bios_boot::prepare_files(mounted.path(), report, fs_kind).unwrap_or_else(|e| {
            run.log(format!("Warning: BIOS boot preparation failed: {:#}", e));
            BootStep::None
        })
    } else {
        BootStep::None
    };

    run.stage(0.95, "Flushing buffers to disk...");
    nix::unistd::sync();
    run.stage(0.97, "Unmounting volume...");
    mounted.unmount().context("cannot unmount the target volume")?;

    if persistence == Some(PersistenceKind::DebianLive) {
        // Debian-live expects persistence.conf on the persistence partition itself.
        run.log("Writing persistence.conf");
        write_persistence_conf(&part2)?;
    }

    if let Some(note) = bios_boot::finish(&boot_step, &part1, fs_kind)
        .unwrap_or_else(|e| Some(format!("BIOS boot setup failed: {:#}", e)))
    {
        run.log(format!("Warning: {}", note));
    }
    nix::unistd::sync();
    Ok(())
}

/// Creates `persistence.conf` (`/ union`) on a freshly formatted Debian-live persistence partition.
fn write_persistence_conf(partition: &str) -> Result<()> {
    let mut mounted = Mounted::mount(partition)?;
    let conf = mounted.path().join("persistence.conf");
    let mut file = fs::File::create(&conf).context("cannot create persistence.conf")?;
    file.write_all(b"/ union\n")?;
    file.sync_all()?;
    drop(file);
    mounted.unmount()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Cursor;
    use std::sync::atomic::Ordering;

    fn options_for(device: &str) -> BurnOptions {
        BurnOptions {
            device_path: PathBuf::from(device),
            image_path: None,
            burn_mode: BurnMode::IsoMode,
            partition_scheme: PartitionScheme::Gpt,
            target_system: TargetSystem::Uefi,
            file_system: FileSystem::Fat32,
            volume_label: "TEST".into(),
            quick_format: true,
            cluster_size: None,
            persistence_size_mb: 0,
            bad_blocks_passes: 0,
            allow_fixed_drives: true,
            wue_options: None,
            wue_username: String::new(),
            wue_locale: "en-US".into(),
            extended_label: false,
            legacy_bios_fixes: false,
            uefi_media_validation: false,
            boot_kind: BootKind::Image,
        }
    }

    #[test]
    fn boot_kinds_enforce_their_requirements() {
        let mut o = options_for("/dev/sdb");
        assert!(BootKind::Image.validate(&o).is_ok());
        assert!(BootKind::NonBootable.validate(&o).is_ok());

        // GPT + FAT32 (the test default) cannot hold a FreeDOS or Syslinux BIOS boot record.
        assert!(BootKind::FreeDos.validate(&o).is_err());
        o.partition_scheme = PartitionScheme::Mbr;
        assert!(BootKind::FreeDos.validate(&o).is_ok());
        assert!(BootKind::Syslinux.validate(&o).is_ok());
        o.file_system = FileSystem::Ntfs;
        assert!(BootKind::Syslinux.validate(&o).is_err());

        assert!(BootKind::UefiNtfs.validate(&o).is_ok());
        o.file_system = FileSystem::Ext4;
        assert!(BootKind::UefiNtfs.validate(&o).is_err());

        o.burn_mode = BurnMode::DdMode;
        assert!(BootKind::NonBootable.validate(&o).is_err());
    }

    #[test]
    fn old_requests_without_the_new_fields_still_parse() {
        let json = serde_json::to_value(options_for("/dev/sdb")).unwrap();
        let mut obj = json.as_object().unwrap().clone();
        for k in [
            "extended_label",
            "legacy_bios_fixes",
            "uefi_media_validation",
            "boot_kind",
        ] {
            obj.remove(k);
        }
        let parsed: BurnOptions = serde_json::from_value(obj.into()).unwrap();
        assert_eq!(parsed.boot_kind, BootKind::Image);
        assert!(!parsed.extended_label);
    }

    #[test]
    fn percent_tokens_from_7z() {
        assert_eq!(parse_percent(" 45% 12 + file.bin"), Some(0.45));
        assert_eq!(parse_percent("\u{8}\u{8}100%"), Some(1.0));
        assert_eq!(parse_percent("Everything is Ok"), None);
        assert_eq!(parse_percent("12 files"), None);
    }

    #[test]
    fn raw_write_copies_exact_bytes_including_unaligned_tail() {
        let payload: Vec<u8> = (0..(DD_CHUNK + 1234)).map(|i| (i % 251) as u8).collect();
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let mut dev = open_device(tmp.path(), OpenMode::WRITE).unwrap();

        let cancel = AtomicBool::new(false);
        write_raw(
            &mut Cursor::new(&payload),
            payload.len() as u64,
            &mut dev,
            &cancel,
            |_, _| {},
        )
        .unwrap();
        drop(dev);
        assert_eq!(fs::read(tmp.path()).unwrap(), payload);
    }

    #[test]
    fn raw_write_honours_cancellation() {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let mut dev = open_device(tmp.path(), OpenMode::WRITE).unwrap();
        let cancel = AtomicBool::new(true);
        let err = write_raw(&mut Cursor::new(vec![1u8; 10]), 10, &mut dev, &cancel, |_, _| {}).unwrap_err();
        assert!(err.is::<Canceled>());
        cancel.store(false, Ordering::Relaxed);
    }

    #[test]
    fn refuses_targets_that_are_not_whole_block_devices() {
        for bad in ["/etc/passwd", "/dev/null", "/does/not/exist"] {
            let mut events = Vec::new();
            let cancel = AtomicBool::new(false);
            execute_burn(&options_for(bad), None, &cancel, |p| events.push(p));
            assert!(
                matches!(events.last(), Some(BurnProgress::Error(_))),
                "{bad}: {events:?}"
            );
            assert!(!events.iter().any(|e| matches!(e, BurnProgress::Finished)));
        }
    }

    #[test]
    fn progress_messages_round_trip_through_json() {
        let msg = BurnProgress::Progress {
            percent: 0.5,
            speed_mb_s: 12.5,
        };
        let line = serde_json::to_string(&msg).unwrap();
        assert!(!line.contains('\n'));
        assert!(matches!(
            serde_json::from_str::<BurnProgress>(&line).unwrap(),
            BurnProgress::Progress { .. }
        ));
    }
}
