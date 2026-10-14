//! The privileged worker (`rufux --worker`), normally started through `pkexec`.
//!
//! Protocol: the request is a single JSON line on stdin (or positional CLI arguments); progress
//! is written to stdout as one JSON [`BurnProgress`] per line. After the request, a `cancel`
//! line, or EOF on stdin, aborts the running job.

use std::io::{self, BufRead, Read, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;

use anyhow::{bail, Context, Result};

use super::device::dump_device_to_image;
use super::elevation::WorkerRequest;
use super::layout::{FileSystem, PartitionScheme, TargetSystem};
use super::privilege::is_root;
use super::writer::{execute_burn, BurnMode, BurnOptions, BurnProgress};
use super::wue;

/// Upper bound for the request line; a request is a few hundred bytes.
const MAX_REQUEST_BYTES: u64 = 1024 * 1024;

fn emit(progress: &BurnProgress) {
    let mut out = io::stdout().lock();
    if serde_json::to_writer(&mut out, progress).is_ok() {
        let _ = out.write_all(b"\n");
        let _ = out.flush();
    }
}

/// `rufux --worker <DEVICE> <iso|dd> <MBR|GPT> <FAT32|NTFS|exFAT|ext4> [IMAGE] [--allow-fixed]`
fn parse_cli(args: &[String]) -> Result<WorkerRequest> {
    let allow_fixed = args.iter().any(|a| a == "--allow-fixed");
    let args: Vec<&String> = args.iter().filter(|a| *a != "--allow-fixed").collect();
    if args.len() < 4 || args.len() > 5 {
        bail!("usage: rufux --worker <DEVICE> <iso|dd> <MBR|GPT> <FAT32|NTFS|exFAT|ext4> [IMAGE] [--allow-fixed]");
    }

    let burn_mode = match args[1].to_ascii_lowercase().as_str() {
        "iso" => BurnMode::IsoMode,
        "dd" => BurnMode::DdMode,
        other => bail!("unknown mode '{}' (expected iso or dd)", other),
    };
    let partition_scheme: PartitionScheme = args[2].parse()?;

    Ok(WorkerRequest::Burn(BurnOptions {
        device_path: PathBuf::from(args[0]),
        image_path: args.get(4).filter(|p| !p.is_empty()).map(PathBuf::from),
        burn_mode,
        partition_scheme,
        target_system: TargetSystem::default_for(partition_scheme),
        file_system: args[3].parse::<FileSystem>()?,
        volume_label: "RUFUX".to_string(),
        quick_format: true,
        cluster_size: None,
        persistence_size_mb: 0,
        bad_blocks_passes: 0,
        allow_fixed_drives: allow_fixed,
        wue_options: None,
        wue_username: String::new(),
        wue_locale: wue::detect_locale(),
        extended_label: false,
        legacy_bios_fixes: false,
        uefi_media_validation: false,
        boot_kind: Default::default(),
    }))
}

/// Reads the JSON request from stdin and starts a thread turning later `cancel`/EOF into a flag.
fn request_from_stdin(cancel: &Arc<AtomicBool>) -> Result<WorkerRequest> {
    let mut line = String::new();
    {
        let mut input = io::stdin().lock();
        (&mut input)
            .take(MAX_REQUEST_BYTES)
            .read_line(&mut line)
            .context("cannot read the request")?;
    }
    let request = serde_json::from_str(line.trim()).context("malformed request")?;

    let cancel = Arc::clone(cancel);
    thread::spawn(move || {
        for line in io::stdin().lock().lines().map_while(|l| l.ok()) {
            if line.trim() == "cancel" {
                break;
            }
        }
        // Either an explicit cancel or EOF (the GUI went away): stop what we are doing.
        cancel.store(true, Ordering::Relaxed);
    });
    Ok(request)
}

/// Entry point of worker mode. `args` are the arguments after `--worker`.
/// Returns the process exit code.
pub fn run_worker(args: &[String]) -> i32 {
    let mut failed = false;
    let mut report = |p: BurnProgress| {
        failed |= matches!(p, BurnProgress::Error(_));
        emit(&p);
    };

    if !is_root() {
        report(BurnProgress::Error(
            "the worker must run as root; start it with pkexec".to_string(),
        ));
        return 1;
    }

    let cancel = Arc::new(AtomicBool::new(false));
    let request = if args.is_empty() {
        request_from_stdin(&cancel)
    } else {
        parse_cli(args)
    };
    let request = match request {
        Ok(r) => r,
        Err(e) => {
            report(BurnProgress::Error(format!("{:#}", e)));
            return 1;
        }
    };

    match request {
        WorkerRequest::Burn(opts) => execute_burn(&opts, None, &cancel, &mut report),
        WorkerRequest::Dump { device, output } => {
            report(BurnProgress::Status("Dumping drive to image file...".to_string()));
            let result = dump_device_to_image(&device, &output, &cancel, |fraction, speed| {
                report(BurnProgress::Progress {
                    percent: fraction,
                    speed_mb_s: speed,
                });
            });
            match result {
                Ok(()) => {
                    report(BurnProgress::Status("Drive image saved".to_string()));
                    report(BurnProgress::Finished);
                }
                Err(e) if e.is::<super::exec::Canceled>() => {
                    report(BurnProgress::Error("Operation canceled by user".into()))
                }
                Err(e) => report(BurnProgress::Error(format!("{:#}", e))),
            }
        }
    }

    i32::from(failed)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn cli_arguments_become_a_burn_request() {
        let WorkerRequest::Burn(o) = parse_cli(&args(&["/dev/sdb", "iso", "GPT", "NTFS", "/tmp/a.iso"])).unwrap()
        else {
            panic!("expected a burn request");
        };
        assert_eq!(o.file_system, FileSystem::Ntfs);
        assert_eq!(o.target_system, TargetSystem::Uefi);
        assert!(!o.allow_fixed_drives);
        assert_eq!(o.image_path, Some(PathBuf::from("/tmp/a.iso")));
    }

    #[test]
    fn cli_rejects_bad_input() {
        assert!(parse_cli(&args(&["/dev/sdb"])).is_err());
        assert!(parse_cli(&args(&["/dev/sdb", "format", "GPT", "NTFS"])).is_err());
        assert!(parse_cli(&args(&["/dev/sdb", "dd", "GPT", "zfs"])).is_err());
    }

    #[test]
    fn allow_fixed_flag_is_optional_anywhere() {
        let WorkerRequest::Burn(o) =
            parse_cli(&args(&["--allow-fixed", "/dev/sdb", "dd", "MBR", "FAT32", "x.img"])).unwrap()
        else {
            panic!();
        };
        assert!(o.allow_fixed_drives);
    }

    #[test]
    fn requests_round_trip_as_single_json_lines() {
        let req = parse_cli(&args(&["/dev/sdb", "dd", "MBR", "FAT32", "x.img"])).unwrap();
        let line = serde_json::to_string(&req).unwrap();
        assert!(!line.contains('\n'));
        assert!(matches!(
            serde_json::from_str::<WorkerRequest>(&line).unwrap(),
            WorkerRequest::Burn(_)
        ));
    }
}
