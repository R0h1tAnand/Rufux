//! Bad Blocks Verification Engine
//!
//! Replicates Rufus's badblocks.c to verify flash memory integrity and
//! detect "fake" capacity USB flash drives.
//!
//! Each pass writes a fill pattern to the whole medium and reads it back. I/O bypasses the page
//! cache (`O_DIRECT`), otherwise the read-back would be served from RAM and prove nothing.
//! **This destroys all data on the device.**

use std::os::unix::fs::FileExt;
use std::path::Path;
use std::sync::atomic::AtomicBool;

use anyhow::Result;

use super::blockdev::{device_size, open_device, AlignedBuf, OpenMode};
use super::exec::check_cancel;

#[derive(Debug, Clone, Default)]
pub struct BadBlocksReport {
    pub bad_block_count: u32,
    pub read_errors: u32,
    pub write_errors: u32,
    pub corruption_errors: u32,
}

const PATTERNS: [u8; 4] = [0x55, 0xAA, 0x00, 0xFF];
const BLOCK_SIZE: usize = 1024 * 1024;

/// Performs a bad blocks check on a storage device.
///
/// `progress_callback` receives `(pass, fraction_of_this_pass, description)`. Only whole 1 MiB
/// blocks are tested; a trailing remainder smaller than that is skipped.
pub fn check_bad_blocks<F>(
    device_path: &Path,
    passes: u32,
    cancel: &AtomicBool,
    mut progress_callback: F,
) -> Result<BadBlocksReport>
where
    F: FnMut(u32, f32, &str),
{
    let mut report = BadBlocksReport::default();
    let mut dev = open_device(device_path, OpenMode::READ_WRITE)?;
    let num_blocks = device_size(&mut dev.file)? / BLOCK_SIZE as u64;
    if num_blocks == 0 {
        return Ok(report);
    }

    let mut write_buf = AlignedBuf::new(BLOCK_SIZE);
    let mut read_buf = AlignedBuf::new(BLOCK_SIZE);
    let actual_passes = passes.clamp(1, PATTERNS.len() as u32);

    for pass in 0..actual_passes {
        let pattern = PATTERNS[pass as usize];
        write_buf.as_mut_slice().fill(pattern);

        for block in 0..num_blocks {
            check_cancel(cancel)?;
            if dev
                .file
                .write_all_at(write_buf.as_slice(), block * BLOCK_SIZE as u64)
                .is_err()
            {
                report.write_errors += 1;
                report.bad_block_count += 1;
            }
            if block % 32 == 0 {
                let frac = (block as f32 / num_blocks as f32) * 0.5;
                progress_callback(
                    pass + 1,
                    frac,
                    &format!("Writing pattern 0x{:02X} (block {}/{})", pattern, block, num_blocks),
                );
            }
        }
        dev.file.sync_all()?;

        for block in 0..num_blocks {
            check_cancel(cancel)?;
            match dev
                .file
                .read_exact_at(read_buf.as_mut_slice(), block * BLOCK_SIZE as u64)
            {
                Ok(()) if read_buf.as_slice() != write_buf.as_slice() => {
                    report.corruption_errors += 1;
                    report.bad_block_count += 1;
                }
                Ok(()) => {}
                Err(_) => {
                    report.read_errors += 1;
                    report.bad_block_count += 1;
                }
            }
            if block % 32 == 0 {
                let frac = 0.5 + (block as f32 / num_blocks as f32) * 0.5;
                progress_callback(
                    pass + 1,
                    frac,
                    &format!("Verifying pattern 0x{:02X} (block {}/{})", pattern, block, num_blocks),
                );
            }
        }
    }

    progress_callback(actual_passes, 1.0, "Bad blocks check complete");
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::exec::Canceled;
    use std::sync::atomic::Ordering;

    fn fake_device(mib: u64) -> tempfile::NamedTempFile {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        tmp.as_file().set_len(mib * 1024 * 1024).unwrap();
        tmp
    }

    #[test]
    fn healthy_medium_reports_no_errors() {
        let dev = fake_device(4);
        let cancel = AtomicBool::new(false);
        let report = check_bad_blocks(dev.path(), 2, &cancel, |_, _, _| {}).unwrap();
        assert_eq!(report.bad_block_count, 0);
    }

    #[test]
    fn cancel_is_honoured() {
        let dev = fake_device(4);
        let cancel = AtomicBool::new(true);
        let err = check_bad_blocks(dev.path(), 1, &cancel, |_, _, _| {}).unwrap_err();
        assert!(err.is::<Canceled>());
        cancel.store(false, Ordering::Relaxed);
    }
}
