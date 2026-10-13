//! UEFI:NTFS Bootloader Support
//!
//! Embeds Pete Batard's UEFI:NTFS bootloader image (uefi-ntfs.img) which enables
//! UEFI firmware to boot from an NTFS partition.

use std::io::Write;
use std::path::Path;

use anyhow::{ensure, Context, Result};

use super::blockdev::{device_size, open_device, OpenMode};

/// Embedded UEFI:NTFS image (1 MiB FAT partition image)
pub const UEFI_NTFS_IMAGE: &[u8] = include_bytes!("../res/uefi-ntfs.img");

/// SHA-256 of the upstream `uefi-ntfs.img` this build is pinned to.
#[cfg(test)]
const UEFI_NTFS_SHA256: &str = "72683fa1250eeea772d3399277b434d4e55ba8dd0dc926e52d817e701fc2eb9e";

/// Writes the UEFI:NTFS image to the start of the given partition device.
pub fn write_uefi_ntfs(partition: &Path) -> Result<()> {
    let mut dev =
        open_device(partition, OpenMode::WRITE).with_context(|| format!("cannot open {}", partition.display()))?;

    let capacity = device_size(&mut dev.file)?;
    ensure!(
        capacity >= UEFI_NTFS_IMAGE.len() as u64,
        "{} ({} bytes) is too small for the UEFI:NTFS image",
        partition.display(),
        capacity
    );

    // The image is a whole number of sectors, so it is valid for O_DIRECT as long as the source
    // buffer is aligned; copy into an aligned buffer first.
    let mut buf = super::blockdev::AlignedBuf::new(UEFI_NTFS_IMAGE.len());
    buf.as_mut_slice().copy_from_slice(UEFI_NTFS_IMAGE);
    dev.file
        .write_all(buf.as_slice())
        .context("cannot write the UEFI:NTFS image")?;
    dev.file.sync_all().context("cannot flush the UEFI:NTFS image")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use sha2::{Digest, Sha256};

    #[test]
    fn embedded_image_is_the_pinned_upstream_binary() {
        assert_eq!(UEFI_NTFS_IMAGE.len(), 1_048_576);
        assert_eq!(hex::encode(Sha256::digest(UEFI_NTFS_IMAGE)), UEFI_NTFS_SHA256);
    }

    #[test]
    fn writes_image_to_a_file_standing_in_for_a_partition() {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        tmp.as_file().set_len(2 * 1024 * 1024).unwrap();
        write_uefi_ntfs(tmp.path()).unwrap();
        let data = std::fs::read(tmp.path()).unwrap();
        assert_eq!(&data[..UEFI_NTFS_IMAGE.len()], UEFI_NTFS_IMAGE);

        let small = tempfile::NamedTempFile::new().unwrap();
        small.as_file().set_len(4096).unwrap();
        assert!(write_uefi_ntfs(small.path()).is_err());
    }
}
