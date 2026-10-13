//! Bad Blocks Verification Engine
//!
//! Replicates Rufus's badblocks.c to verify flash memory integrity and
//! detect "fake" capacity USB flash drives.

use std::fs::OpenOptions;
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::Path;

#[derive(Debug, Clone, Default)]
pub struct BadBlocksReport {
    pub bad_block_count: u32,
    pub read_errors: u32,
    pub write_errors: u32,
    pub corruption_errors: u32,
}

const PATTERNS: [u8; 4] = [0x55, 0xAA, 0x00, 0xFF];

/// Performs bad blocks check on a storage device
pub fn check_bad_blocks<F>(
    device_path: &Path,
    device_size_bytes: u64,
    passes: u32,
    mut progress_callback: F,
) -> anyhow::Result<BadBlocksReport>
where
    F: FnMut(u32, f32, &str),
{
    let mut report = BadBlocksReport::default();
    let block_size = 1024 * 1024; // 1 MiB block size
    let num_blocks = device_size_bytes / block_size as u64;

    if num_blocks == 0 {
        return Ok(report);
    }

    let mut dev = OpenOptions::new()
        .read(true)
        .write(true)
        .open(device_path)?;

    let mut write_buf = vec![0u8; block_size];
    let mut read_buf = vec![0u8; block_size];

    let actual_passes = passes.clamp(1, 4);

    for pass in 0..actual_passes {
        let pattern = PATTERNS[pass as usize];
        write_buf.fill(pattern);

        // Pass write phase
        for block_idx in 0..num_blocks {
            let offset = block_idx * block_size as u64;
            if dev.seek(SeekFrom::Start(offset)).is_err() {
                report.write_errors += 1;
                continue;
            }

            if dev.write_all(&write_buf).is_err() {
                report.write_errors += 1;
            }

            if block_idx % 32 == 0 {
                let progress = (block_idx as f32 / num_blocks as f32) * 0.5;
                progress_callback(pass + 1, progress, &format!("Writing pattern 0x{:02X} (block {}/{})", pattern, block_idx, num_blocks));
            }
        }

        let _ = dev.sync_all();

        // Pass read & verify phase
        for block_idx in 0..num_blocks {
            let offset = block_idx * block_size as u64;
            if dev.seek(SeekFrom::Start(offset)).is_err() {
                report.read_errors += 1;
                report.bad_block_count += 1;
                continue;
            }

            match dev.read_exact(&mut read_buf) {
                Ok(_) => {
                    if read_buf != write_buf {
                        report.corruption_errors += 1;
                        report.bad_block_count += 1;
                    }
                }
                Err(_) => {
                    report.read_errors += 1;
                    report.bad_block_count += 1;
                }
            }

            if block_idx % 32 == 0 {
                let progress = 0.5 + (block_idx as f32 / num_blocks as f32) * 0.5;
                progress_callback(pass + 1, progress, &format!("Verifying pattern 0x{:02X} (block {}/{})", pattern, block_idx, num_blocks));
            }
        }
    }

    progress_callback(actual_passes, 1.0, "Bad blocks check complete");
    Ok(report)
}
