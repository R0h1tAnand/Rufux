//! Multi-hash computation for MD5, SHA-1, SHA-256, and SHA-512
//!
//! Computes checksums simultaneously in a single stream pass with real-time progress.

use std::fs::File;
use std::io::Read;
use std::path::Path;
use sha2::{Sha256, Sha512, Digest};
use sha1::Sha1;
use md5::Md5;

#[derive(Debug, Clone, Default)]
pub struct ImageHashes {
    pub md5: String,
    pub sha1: String,
    pub sha256: String,
    pub sha512: String,
}

/// Compute all 4 hashes simultaneously for a file, reporting progress (0.0 to 1.0)
pub fn compute_hashes<F>(file_path: &Path, mut progress_callback: F) -> anyhow::Result<ImageHashes>
where
    F: FnMut(f32),
{
    let mut file = File::open(file_path)?;
    let total_size = file.metadata()?.len();

    let mut md5_hasher = Md5::new();
    let mut sha1_hasher = Sha1::new();
    let mut sha256_hasher = Sha256::new();
    let mut sha512_hasher = Sha512::new();

    let mut buffer = vec![0u8; 1024 * 1024]; // 1 MiB read buffer
    let mut bytes_read_total = 0u64;

    loop {
        let n = file.read(&mut buffer)?;
        if n == 0 {
            break;
        }

        let chunk = &buffer[..n];
        md5_hasher.update(chunk);
        sha1_hasher.update(chunk);
        sha256_hasher.update(chunk);
        sha512_hasher.update(chunk);

        bytes_read_total += n as u64;
        if total_size > 0 {
            let progress = (bytes_read_total as f32 / total_size as f32).min(1.0);
            progress_callback(progress);
        }
    }

    progress_callback(1.0);

    let md5_res = format!("{:x}", md5_hasher.finalize());
    let sha1_res = format!("{:x}", sha1_hasher.finalize());
    let sha256_res = format!("{:x}", sha256_hasher.finalize());
    let sha512_res = format!("{:x}", sha512_hasher.finalize());

    Ok(ImageHashes {
        md5: md5_res,
        sha1: sha1_res,
        sha256: sha256_res,
        sha512: sha512_res,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    #[test]
    fn test_compute_hashes() {
        let mut tmp = NamedTempFile::new().unwrap();
        tmp.write_all(b"Hello Rufux World!").unwrap();

        let hashes = compute_hashes(tmp.path(), |_| {}).unwrap();
        assert_eq!(hashes.md5, "b01b3f74292d790132cff74a8a6e944a");
        assert_eq!(hashes.sha1, "f7cb4fe2005d1805f2cb70ba58ebd628802dca36");
        assert_eq!(
            hashes.sha256,
            "4cd59ded9fefcc7931209e3270e5a37e8024ce0baa34f617c1a3d84243a441c6"
        );
    }
}
