//! UEFI:NTFS Bootloader Support
//!
//! Embeds Pete Batard's UEFI:NTFS bootloader image (uefi-ntfs.img) which enables
//! UEFI firmware to boot from an NTFS partition.

use std::fs::OpenOptions;
use std::io::{Seek, SeekFrom, Write};
use std::path::Path;

/// Embedded UEFI:NTFS image (1MB partition image)
pub const UEFI_NTFS_IMAGE: &[u8] = include_bytes!("../res/uefi-ntfs.img");

/// Writes the UEFI:NTFS image to a specified partition or byte offset on the target device
pub fn write_uefi_ntfs(device_path: &Path, partition_offset_bytes: u64) -> anyhow::Result<()> {
    let mut file = OpenOptions::new()
        .write(true)
        .open(device_path)?;

    file.seek(SeekFrom::Start(partition_offset_bytes))?;
    file.write_all(UEFI_NTFS_IMAGE)?;
    file.sync_all()?;
    Ok(())
}

/// Returns the size in bytes of the embedded UEFI:NTFS image
pub fn get_uefi_ntfs_size() -> usize {
    UEFI_NTFS_IMAGE.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_uefi_ntfs_image_presence() {
        assert_eq!(UEFI_NTFS_IMAGE.len(), 1048576); // 1 MiB
    }
}
