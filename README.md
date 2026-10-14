<p align="center"><img src="assets/icons/rufux.svg" alt="Rufux icon" width="128"></p>

# Rufux

**A native Linux take on [Rufus](https://github.com/pbatard/rufus), written in Rust.**
Create bootable USB drives from ISO and disk images with a familiar interface and no terminal.

[![Rust](https://img.shields.io/badge/rust-1.92%2B-orange.svg?style=flat-square&logo=rust)](https://www.rust-lang.org/)
[![License](https://img.shields.io/badge/license-GPLv3-blue.svg?style=flat-square)](LICENSE)
[![Platform](https://img.shields.io/badge/platform-Linux-lightgrey.svg?style=flat-square&logo=linux)](https://kernel.org)
[![Status](https://img.shields.io/badge/status-early%20development-yellow.svg?style=flat-square)](#project-status)

---

## Features

- **Two write modes**
  - *ISO mode*: partitions the drive (MBR or GPT), formats it (FAT32, NTFS, exFAT, ext4) and extracts the image onto it.
  - *DD mode*: streams the image to the device block for block with direct I/O.
- **Boot selection**: write an ISO or disk image, or create a FreeDOS drive, an empty Syslinux 6.x drive, a UEFI:NTFS-only drive or a plain non-bootable drive.
- **UEFI:NTFS** layout for booting NTFS media on UEFI systems, using the upstream `uefi-ntfs.img` (checksum-pinned).
- **Runtime UEFI media validation**: for images that ship an `md5sum.txt`, installs the upstream [`uefi-md5sum`](https://github.com/pbatard/uefi-md5sum) loader (checksum-pinned) so the drive verifies itself on every UEFI boot.
- **Old-BIOS fixes**: starts the first MBR partition at the CHS-aligned sector 63.
- **Extended label and icon files**: writes `autorun.inf` and an icon so Windows shows the label and icon.
- **Windows setup customisation**: generates an `autounattend.xml` that can skip the TPM, Secure Boot and RAM checks, skip the online-account requirement, create a local account, and disable BitLocker auto-encryption and telemetry prompts.
- **Linux persistence**: creates a `casper-rw` (Ubuntu family) or `persistence` (Debian live) partition.
- **Bad-block check**: multi-pass write and read-back test that bypasses the page cache, to expose counterfeit or failing drives.
- **Checksums**: MD5, SHA-1, SHA-256 and SHA-512 computed in parallel in a single read of the file.
- **Drive dump**: save a whole drive to an image file.
- **Live log** with save-to-file.
- **Native authentication**: the interface runs unprivileged; Polkit shows the standard password dialog only when a drive is written.

## Safety

- Disks that hold the running system (root, boot, home, swap, a live-USB boot medium, and the disks behind LVM/LUKS volumes) are never listed and are refused again by the privileged worker.
- Non-removable disks are hidden unless you enable **List USB Hard Drives**.
- A confirmation dialog names the device before anything is erased.
- Images are opened with your own permissions, and an image stored on the target drive is rejected.
- Every external command is checked; a failed `parted` or `mkfs` stops the run instead of reporting success.

## Requirements

| Type | Packages |
|------|----------|
| Toolchain | Rust 1.92 or newer ([rustup](https://rustup.rs)) |
| Required tools | `util-linux` (`lsblk`, `mount`), `parted`, `p7zip`, `dosfstools`, `ntfs-3g`, `exfatprogs`, `e2fsprogs`, `polkit` (`pkexec`) |
| Optional | `syslinux` (BIOS boot of Linux media and the Syslinux boot selection), `ms-sys` (BIOS boot of Windows media and the FreeDOS boot selection) |
| Interface | GTK 4 and libadwaita 1.5 or newer (`libgtk-4-dev`, `libadwaita-1-dev` to build) |

## Installation

### Quick install

```bash
curl -fsSL https://raw.githubusercontent.com/R0h1tAnand/Rufux/main/install.sh | sh
```

The script installs the required system packages (apt, dnf, pacman and zypper are supported), installs Rust through rustup if you do not have 1.92 or newer, builds Rufux from source and installs it under `/usr/local`. It uses `sudo` only for package installation and for copying files into system directories.

Prefer to read it first? Download it, inspect it, then run it:

```bash
curl -fsSLO https://raw.githubusercontent.com/R0h1tAnand/Rufux/main/install.sh
less install.sh
sh install.sh
```

| Option | Effect |
|--------|--------|
| `--prefix DIR` | install under `DIR` instead of `/usr/local` |
| `--no-deps` | skip installing system packages |
| `--uninstall` | remove Rufux |
| `RUFUX_REF=<tag>` | build a specific branch or tag instead of `main` |

Options are passed after `sh -s --` when piping, for example:

```bash
curl -fsSL https://raw.githubusercontent.com/R0h1tAnand/Rufux/main/install.sh | sh -s -- --prefix /usr
```

The installer also registers the Polkit policy. It is generated for the chosen prefix and allows only `rufux --worker`, so authenticating never grants a general root session.

Start Rufux from your application menu or with `rufux`.

**Uninstall:**

```bash
curl -fsSL https://raw.githubusercontent.com/R0h1tAnand/Rufux/main/install.sh | sh -s -- --uninstall
```

### Manual build

Install the packages from the table above, then:

```bash
git clone https://github.com/R0h1tAnand/Rufux.git
cd Rufux
make release
./target/release/rufux
```

When run from the build directory (not installed), Polkit falls back to its generic "run a program as root" prompt. Use `sh install.sh` for the restricted policy.

## Usage

1. Start Rufux.
2. Choose the target **Device**.
3. Click **SELECT** and pick an ISO or disk image.
4. Review the partition scheme, target system and file system.
5. Click **START**, confirm the warning and authenticate when prompted.

> **Warning:** writing an image erases everything on the selected device.

Raw `.img` files are always written in DD mode. For ISOHybrid images you are asked which mode to use.

### Command-line worker

The privileged part of Rufux can be run on its own, for scripting:

```bash
pkexec rufux --worker <DEVICE> <iso|dd> <MBR|GPT> <FAT32|NTFS|exFAT|ext4> [IMAGE] [--allow-fixed]

# Example
pkexec rufux --worker /dev/sdX iso GPT NTFS ~/Downloads/Win11.iso
```

Progress is printed as one JSON object per line. Without arguments the worker reads a JSON request from stdin, which is how the interface talks to it.

## Development

```bash
make build     # debug build
make test      # unit tests
make check     # rustfmt, clippy (warnings are errors) and tests
make run       # run a release build
```

```
src/
├── main.rs        entry point
├── core/          devices and safety checks, ISO scanner, writer, hashing, WUE, UEFI:NTFS, worker
└── ui/            GTK4 / libadwaita window and dialogs
assets/            icons and FreeDOS files
```

To exercise the write path without a physical drive, attach a loop device (`sudo losetup -fP disk.img`) and pass it with `--worker`.

## Project status

Rufux is in early development. The partitioning, formatting and extraction code is covered by unit tests, but real-drive behaviour varies between machines: try it on a drive you can afford to erase first.

Not implemented yet:

- MS-DOS boot media (its files cannot be redistributed) and Windows To Go (it needs a Windows-only BCD/DISM step); neither is offered.
- Syslinux 4.x; Syslinux boot media uses the version installed on your system.
- Compressed images (`.gz`, `.xz`); decompress them first.
- BIOS booting depends on the optional `syslinux` and `ms-sys` tools; without them the drive boots in UEFI mode only.

## Acknowledgments

- [Pete Batard](https://github.com/pbatard) and the Rufus contributors for the original tool, `uefi-ntfs.img` and `uefi-md5sum` (GPL-2.0).
- The [FreeDOS](https://www.freedos.org) project (provenance of the bundled files in `assets/freedos/readme.txt`).
- [Rust](https://www.rust-lang.org/) and [GTK 4](https://gtk.org) and [libadwaita](https://gnome.pages.gitlab.gnome.org/libadwaita/).

Rufux is an independent project and is not affiliated with Rufus.

## License

[GPL-3.0-or-later](LICENSE)
