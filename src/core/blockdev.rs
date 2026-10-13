//! Low-level block device I/O helpers: aligned buffers, `O_DIRECT` opening and size queries.

use std::fs::{File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom};
use std::os::fd::AsRawFd;
use std::os::unix::fs::{FileTypeExt, OpenOptionsExt};
use std::thread;
use std::time::Duration;

use nix::fcntl::{fcntl, FcntlArg, OFlag};

/// Alignment satisfying `O_DIRECT` buffer requirements on every common logical sector size.
pub const IO_ALIGN: usize = 4096;

/// A heap buffer whose start address is aligned to [`IO_ALIGN`], as required by `O_DIRECT`.
pub struct AlignedBuf {
    raw: Vec<u8>,
    offset: usize,
    len: usize,
}

impl AlignedBuf {
    pub fn new(len: usize) -> Self {
        let raw = vec![0u8; len + IO_ALIGN];
        let offset = raw.as_ptr().align_offset(IO_ALIGN);
        Self { raw, offset, len }
    }

    pub fn as_slice(&self) -> &[u8] {
        &self.raw[self.offset..self.offset + self.len]
    }

    pub fn as_mut_slice(&mut self) -> &mut [u8] {
        &mut self.raw[self.offset..self.offset + self.len]
    }
}

/// How a device (or image file) should be opened.
#[derive(Debug, Clone, Copy)]
pub struct OpenMode {
    pub read: bool,
    pub write: bool,
    /// Refuse to open a block device that is mounted or otherwise in use (`O_EXCL`).
    pub exclusive: bool,
}

impl OpenMode {
    pub const READ: Self = Self {
        read: true,
        write: false,
        exclusive: false,
    };
    pub const WRITE: Self = Self {
        read: false,
        write: true,
        exclusive: true,
    };
    pub const READ_WRITE: Self = Self {
        read: true,
        write: true,
        exclusive: true,
    };
}

/// An opened device and whether the kernel page cache is being bypassed.
pub struct Device {
    pub file: File,
    pub direct: bool,
}

/// Opens a block device (or, for tests, a regular file) for exclusive, uncached access.
///
/// Writable block devices are opened with `O_EXCL` so the open fails with `EBUSY` while any
/// partition is still mounted, and all devices with `O_DIRECT` so reads genuinely come from the medium. If the filesystem
/// does not support `O_DIRECT` the open transparently falls back to buffered I/O.
pub fn open_device(path: &std::path::Path, mode: OpenMode) -> io::Result<Device> {
    let is_block = path.metadata()?.file_type().is_block_device();
    let exclusive = if is_block && mode.exclusive { libc::O_EXCL } else { 0 };
    let flags = libc::O_DIRECT | exclusive;

    let attempt = |extra: i32| {
        OpenOptions::new()
            .read(mode.read)
            .write(mode.write)
            .custom_flags(extra)
            .open(path)
    };

    let mut last_err = None;
    for _ in 0..10 {
        match attempt(flags) {
            Ok(file) => return Ok(Device { file, direct: true }),
            Err(e) if e.raw_os_error() == Some(libc::EINVAL) => {
                // Filesystem without O_DIRECT support (tmpfs, ...): use ordinary buffered I/O.
                return attempt(exclusive).map(|file| Device { file, direct: false });
            }
            // udev may briefly hold the node right after partitioning; retry a few times.
            Err(e) if e.raw_os_error() == Some(libc::EBUSY) => {
                last_err = Some(e);
                thread::sleep(Duration::from_millis(300));
            }
            Err(e) => return Err(e),
        }
    }
    Err(last_err.unwrap_or_else(|| io::Error::from_raw_os_error(libc::EBUSY)))
}

/// Size in bytes of a block device or file.
pub fn device_size(file: &mut File) -> io::Result<u64> {
    let size = file.seek(SeekFrom::End(0))?;
    file.seek(SeekFrom::Start(0))?;
    Ok(size)
}

/// Disables `O_DIRECT` on an open file so that an unaligned tail can be written.
pub fn clear_direct(file: &File) -> io::Result<()> {
    let bits = fcntl(file.as_raw_fd(), FcntlArg::F_GETFL).map_err(io::Error::from)?;
    let flags = OFlag::from_bits_truncate(bits) & !OFlag::O_DIRECT;
    fcntl(file.as_raw_fd(), FcntlArg::F_SETFL(flags)).map_err(io::Error::from)?;
    Ok(())
}

/// Reads until `buf` is full or EOF; returns the number of bytes read.
pub fn read_fill(src: &mut impl Read, buf: &mut [u8]) -> io::Result<usize> {
    let mut filled = 0;
    while filled < buf.len() {
        match src.read(&mut buf[filled..]) {
            Ok(0) => break,
            Ok(n) => filled += n,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        }
    }
    Ok(filled)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aligned_buffer_is_aligned_and_sized() {
        let mut buf = AlignedBuf::new(8192);
        assert_eq!(buf.as_slice().len(), 8192);
        assert_eq!(buf.as_slice().as_ptr() as usize % IO_ALIGN, 0);
        buf.as_mut_slice()[8191] = 7;
        assert_eq!(buf.as_slice()[8191], 7);
    }

    #[test]
    fn regular_files_fall_back_when_direct_io_is_unsupported() {
        let tmp = tempfile::NamedTempFile::new().unwrap();
        let mut dev = open_device(tmp.path(), OpenMode::READ_WRITE).unwrap();
        assert_eq!(device_size(&mut dev.file).unwrap(), 0);
    }
}
