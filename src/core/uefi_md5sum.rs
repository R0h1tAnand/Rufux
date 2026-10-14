//! Runtime UEFI media validation.
//!
//! Installs Pete Batard's `uefi-md5sum` loader in front of the image's own UEFI boot loader.
//! At every boot it checks the files listed in the image's `md5sum.txt`, then chain-loads the
//! original loader. It only applies to images that ship an `md5sum.txt`.

use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

/// `(architecture suffix of bootXXX.efi, loader image)`; upstream v1.2, checksums in the tests.
const LOADERS: [(&str, &[u8]); 2] = [
    ("x64", include_bytes!("../res/uefi-md5sum-x64.efi")),
    ("ia32", include_bytes!("../res/uefi-md5sum-ia32.efi")),
];

#[derive(Debug, PartialEq, Eq)]
pub enum Outcome {
    /// The loader was installed in front of these architectures.
    Installed(Vec<&'static str>),
    /// Nothing was changed, for the given reason.
    Skipped(&'static str),
}

/// Resolves `relative` below `root` ignoring ASCII case, since images are authored for
/// case-insensitive file systems but the target may be mounted case-sensitively.
fn find_ci(root: &Path, relative: &[&str]) -> Option<PathBuf> {
    let mut current = root.to_path_buf();
    for part in relative {
        current = fs::read_dir(&current)
            .ok()?
            .filter_map(Result::ok)
            .find(|e| e.file_name().to_string_lossy().eq_ignore_ascii_case(part))?
            .path();
    }
    Some(current)
}

/// Points the `md5sum.txt` entry of each renamed loader at its new name; the contents, and so
/// the checksum, are unchanged.
fn rewrite_md5sum(text: &str, renamed: &[&str]) -> String {
    let mut out = String::with_capacity(text.len() + 32);
    for line in text.split_inclusive('\n') {
        out.push_str(&rewrite_line(line, renamed));
    }
    out
}

fn rewrite_line(line: &str, renamed: &[&str]) -> String {
    let body = line.trim_end_matches(['\r', '\n']);
    let ending = &line[body.len()..];
    // "<hash><space>[ *]<path>"
    let Some((hash, rest)) = body.split_once(char::is_whitespace) else {
        return line.to_string();
    };
    let path = rest.trim_start_matches([' ', '*']);
    let prefix = &rest[..rest.len() - path.len()];
    let normalized = path.trim_start_matches("./");

    for arch in renamed {
        let target = format!("efi/boot/boot{arch}.efi");
        if normalized.eq_ignore_ascii_case(&target) {
            let dot = path.rfind('.').unwrap_or(path.len());
            return format!("{hash} {prefix}{}_original{}{ending}", &path[..dot], &path[dot..]);
        }
    }
    line.to_string()
}

/// Installs the validation loader on the mounted volume.
pub fn install(root: &Path) -> Result<Outcome> {
    let Some(md5_path) = find_ci(root, &["md5sum.txt"]) else {
        return Ok(Outcome::Skipped("the image has no md5sum.txt"));
    };

    let mut installed = Vec::new();
    for (arch, loader) in LOADERS {
        let name = format!("boot{arch}.efi");
        let Some(original) = find_ci(root, &["efi", "boot", &name]) else {
            continue;
        };
        let file_name = original.file_name().unwrap_or_default().to_string_lossy().into_owned();
        let stem = &file_name[..file_name.len() - ".efi".len()];
        let ext = &file_name[stem.len()..];
        let renamed = original.with_file_name(format!("{stem}_original{ext}"));

        fs::rename(&original, &renamed).with_context(|| format!("cannot rename {}", original.display()))?;
        fs::write(&original, loader).with_context(|| format!("cannot write {}", original.display()))?;
        installed.push(arch);
    }
    if installed.is_empty() {
        return Ok(Outcome::Skipped("the image has no EFI boot loader"));
    }

    let text = fs::read_to_string(&md5_path).context("cannot read md5sum.txt")?;
    fs::write(&md5_path, rewrite_md5sum(&text, &installed)).context("cannot update md5sum.txt")?;
    Ok(Outcome::Installed(installed))
}

#[cfg(test)]
mod tests {
    use super::*;
    use sha2::{Digest, Sha256};

    #[test]
    fn embedded_loaders_are_the_pinned_upstream_binaries() {
        let sums: Vec<_> = LOADERS.iter().map(|(_, b)| hex::encode(Sha256::digest(b))).collect();
        assert_eq!(
            sums[0],
            "363775f2614cf2f65f5d1934a035c658d61b71664a43f0077d7126f490af1526"
        );
        assert_eq!(
            sums[1],
            "d874349a2df1e4a16ffc926283f35a0bbc6277634c4a80615ab872008a959918"
        );
    }

    #[test]
    fn md5sum_entries_follow_the_renamed_loader() {
        let text = "# md5sum_totalbytes = 0x10\r\n\
                    aaaa  ./EFI/boot/bootx64.efi\r\n\
                    bbbb *./efi/boot/grubx64.efi\n\
                    cccc  ./casper/vmlinuz\n";
        let out = rewrite_md5sum(text, &["x64"]);
        assert!(out.contains("aaaa  ./EFI/boot/bootx64_original.efi\r\n"));
        assert!(out.contains("bbbb *./efi/boot/grubx64.efi\n"));
        assert!(out.contains("# md5sum_totalbytes = 0x10\r\n"));
        assert_eq!(out.lines().count(), 4);
    }

    #[test]
    fn installs_in_front_of_the_original_loader() {
        let dir = tempfile::tempdir().unwrap();
        let boot = dir.path().join("EFI/BOOT");
        fs::create_dir_all(&boot).unwrap();
        fs::write(boot.join("BOOTX64.EFI"), b"original").unwrap();
        fs::write(dir.path().join("md5sum.txt"), "aaaa  ./EFI/BOOT/BOOTX64.EFI\n").unwrap();

        assert_eq!(install(dir.path()).unwrap(), Outcome::Installed(vec!["x64"]));
        assert_eq!(fs::read(boot.join("BOOTX64_original.EFI")).unwrap(), b"original");
        assert_eq!(fs::read(boot.join("BOOTX64.EFI")).unwrap(), LOADERS[0].1);
        assert_eq!(
            fs::read_to_string(dir.path().join("md5sum.txt")).unwrap(),
            "aaaa  ./EFI/BOOT/BOOTX64_original.EFI\n"
        );
    }

    #[test]
    fn leaves_images_without_md5sum_or_loader_alone() {
        let dir = tempfile::tempdir().unwrap();
        assert!(matches!(install(dir.path()).unwrap(), Outcome::Skipped(_)));

        fs::write(dir.path().join("md5sum.txt"), "aaaa  ./x\n").unwrap();
        assert!(matches!(install(dir.path()).unwrap(), Outcome::Skipped(_)));
        assert_eq!(
            fs::read_to_string(dir.path().join("md5sum.txt")).unwrap(),
            "aaaa  ./x\n"
        );
    }
}
