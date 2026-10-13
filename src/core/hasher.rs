//! Multi-hash computation for MD5, SHA-1, SHA-256, and SHA-512
//!
//! The file is read once; every chunk is fanned out to one worker thread per algorithm so the
//! four digests are computed in parallel. The `sha1`/`sha2` crates use the CPU's SHA extensions
//! when available.

use std::fs::File;
use std::io::Read;
use std::path::Path;
use std::sync::Arc;
use std::thread::{self, Scope, ScopedJoinHandle};

use anyhow::Result;
use crossbeam_channel::{bounded, Sender};
use md5::Md5;
use sha1::Sha1;
use sha2::{Digest, Sha256, Sha512};

const CHUNK: usize = 1024 * 1024;

#[derive(Debug, Clone, Default)]
pub struct ImageHashes {
    pub md5: String,
    pub sha1: String,
    pub sha256: String,
    pub sha512: String,
}

type Chunk = Arc<Vec<u8>>;

fn spawn_hasher<'scope, 'env, D>(
    scope: &'scope Scope<'scope, 'env>,
) -> (Sender<Chunk>, ScopedJoinHandle<'scope, String>)
where
    D: Digest + Send + 'scope,
{
    let (tx, rx) = bounded::<Chunk>(4);
    let handle = scope.spawn(move || {
        let mut digest = D::new();
        for chunk in rx {
            digest.update(chunk.as_slice());
        }
        hex::encode(digest.finalize())
    });
    (tx, handle)
}

/// Compute all 4 hashes simultaneously for a file, reporting progress (0.0 to 1.0)
pub fn compute_hashes<F>(file_path: &Path, mut progress_callback: F) -> Result<ImageHashes>
where
    F: FnMut(f32),
{
    let mut file = File::open(file_path)?;
    let total_size = file.metadata()?.len();

    thread::scope(|scope| -> Result<ImageHashes> {
        let (md5_tx, md5) = spawn_hasher::<Md5>(scope);
        let (sha1_tx, sha1) = spawn_hasher::<Sha1>(scope);
        let (sha256_tx, sha256) = spawn_hasher::<Sha256>(scope);
        let (sha512_tx, sha512) = spawn_hasher::<Sha512>(scope);
        let senders = [md5_tx, sha1_tx, sha256_tx, sha512_tx];

        let mut done = 0u64;
        loop {
            let mut buf = vec![0u8; CHUNK];
            let n = file.read(&mut buf)?;
            if n == 0 {
                break;
            }
            buf.truncate(n);
            let chunk = Arc::new(buf);
            for tx in &senders {
                // A hasher only stops early if it panicked; the join below reports that.
                let _ = tx.send(Arc::clone(&chunk));
            }

            done += n as u64;
            if total_size > 0 {
                progress_callback((done as f32 / total_size as f32).min(1.0));
            }
        }
        drop(senders);

        let join = |h: ScopedJoinHandle<'_, String>| h.join().map_err(|_| anyhow::anyhow!("hash worker panicked"));
        let hashes = ImageHashes {
            md5: join(md5)?,
            sha1: join(sha1)?,
            sha256: join(sha256)?,
            sha512: join(sha512)?,
        };
        progress_callback(1.0);
        Ok(hashes)
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
        assert_eq!(hashes.sha512.len(), 128);
    }

    #[test]
    fn hashes_files_larger_than_one_chunk() {
        let mut tmp = NamedTempFile::new().unwrap();
        tmp.write_all(&vec![0xABu8; CHUNK * 2 + 17]).unwrap();
        let mut last = 0.0;
        let hashes = compute_hashes(tmp.path(), |p| last = p).unwrap();
        assert_eq!(last, 1.0);
        assert_eq!(hashes.sha256.len(), 64);
    }
}
