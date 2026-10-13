# Rufux: The Reliable USB Formatting Utility for Linux

[![Rust](https://img.shields.io/badge/rust-1.92%2B-orange.svg?style=flat-square&logo=rust)](https://www.rust-lang.org/)
[![License](https://img.shields.io/badge/license-GPLv3-blue.svg?style=flat-square)](https://www.gnu.org/licenses/gpl-3.0.en.html)
[![Platform](https://img.shields.io/badge/platform-Linux-lightgrey.svg?style=flat-square&logo=linux)](https://kernel.org)

**Rufux** is a native, blazingly fast, 1-to-1 faithful Linux port of [Rufus](https://github.com/pbatard/rufus) (The Reliable USB Formatting Utility) rewritten in **Rust**.

It brings the complete Rufus experience, iconic layout, advanced options, and hardware compatibility to Linux with bare-metal performance, memory safety, and native Polkit graphical privilege escalation.

---

## Features

* **1-to-1 Faithful GUI**: Pixel-precise reproduction of the iconic Rufus interface, typography, dark/light theme, and control flows.
* **Dual Flashing Modes**:
  * **ISO Extraction Mode**: Partitions target disk (MBR/GPT), formats filesystem, extracts ISO payload, and sets up bootloader.
  * **DD Image Mode**: Direct raw byte-for-byte block streaming to `/dev/sdX` with 4MB aligned buffers and real-time MB/s throughput.
* **Windows 11 Customization (WUE)**:
  * Automatically injects `autounattend.xml` to bypass TPM 2.0, Secure Boot, and 4GB+ RAM checks.
  * Bypasses the online Microsoft Account requirement (`BypassNRO`).
  * Creates an offline local administrator account.
  * Disables telemetry and privacy diagnostic questionnaires.
  * Disables BitLocker automatic encryption.
* **Signature UEFI:NTFS Dual-Partitioning**:
  * Seamlessly boots Windows installations from NTFS partitions on UEFI firmware without running into FAT32 4GB file size limits.
  * Directly embeds Pete Batard's official `uefi-ntfs.img` bootloader.
* **Linux Persistence Partitions**:
  * Automatically detects Ubuntu / Debian Live systems and allocates an ext4 `casper-rw` / `persistence` partition using a real-time size slider.
* **Multi-Algorithm Checksums (`#` Button)**:
  * Computes MD5, SHA-1, SHA-256, and SHA-512 simultaneously in a single stream pass with verification match highlighting.
* **Bad Blocks Verification**:
  * Multi-pass test patterns (0x55, 0xAA, 0x00, 0xFF) to detect bad memory sectors and counterfeit "fake capacity" flash drives.
* **Live Rufus Log (`IDD_LOG`)**:
  * Real-time console displaying every partition offset, formatting step, speed, and execution detail with Clear and Save options.
* **Zero-Terminal Experience via Polkit (`pkexec`)**:
  * Launches as a standard unprivileged desktop application from your system app menu.
  * Prompts for standard desktop GUI authentication only when writing to block devices.
* **Blazingly Fast**:
  * Sub-10ms startup time, tiny RAM footprint, and pure Rust concurrency.

---

## Architecture Overview

```
                      ┌────────────────────────────┐
                      │    Rufux Desktop UI        │
                      │  (Rust + eframe / egui)    │
                      └─────────────┬──────────────┘
                                    │
              ┌─────────────────────┼─────────────────────┐
              ▼                     ▼                     ▼
       ┌──────────────┐      ┌──────────────┐      ┌──────────────┐
       │ Device Mgr   │      │ ISO Scanner  │      │   Checksum   │
       │(lsblk/udev)  │      │ (7z/ISO9660) │      │(MD5/SHA1/256)│
       └──────────────┘      └──────────────┘      └──────────────┘
                                    │
                         User clicks "START"
                                    │
                                    ▼
                      ┌────────────────────────────┐
                      │  Elevated Backend Worker   │
                      │  (Direct / pkexec Polkit)  │
                      └─────────────┬──────────────┘
                                    │
              ┌─────────────────────┴─────────────────────┐
              ▼                                           ▼
       ┌──────────────┐                            ┌──────────────┐
       │   DD Mode    │                            │   ISO Mode   │
       │(Raw Streams) │                            │(Partitioning)│
       └──────────────┘                            └──────┬───────┘
                                                          │
                                           ┌──────────────┼──────────────┐
                                           ▼              ▼              ▼
                                     ┌───────────┐  ┌───────────┐  ┌───────────┐
                                     │  parted   │  │  mkfs.*   │  │ UEFI:NTFS │
                                     │ (MBR/GPT) │  │(FAT/NTFS) │  │  Driver   │
                                     └───────────┘  └───────────┘  └───────────┘
```

---

## Building from Source

### Prerequisites

Ensure you have the Rust toolchain and standard disk utilities installed:

```bash
# Ubuntu / Debian / Pop!_OS
sudo apt update
sudo apt install -y build-essential parted dosfstools ntfs-3g p7zip-full policykit-1 zenity

# Fedora
sudo dnf install -y parted dosfstools ntfs-3g p7zip polkit zenity

# Arch Linux
sudo pacman -S --needed parted dosfstools ntfs-3g p7zip polkit zenity
```

### Build & Run

```bash
# Clone the repository
git clone https://github.com/R0h1tAnand/Rufux.git
cd Rufux

# Build optimized release binary
make release

# Run directly
./target/release/rufux
```

---

## Installation

Install Rufux system-wide with application menu launcher and Polkit rules:

```bash
sudo make install
```

This installs:
- Binary: `/usr/local/bin/rufux`
- Desktop shortcut: `/usr/share/applications/rufux.desktop`
- Polkit policy: `/usr/share/polkit-1/actions/org.rufux.policy`
- Icon: `/usr/share/icons/hicolor/128x128/apps/rufux.png`

---

## Command Line Usage

Rufux can also be invoked as an automated backend worker:

```bash
# Worker mode syntax:
rufux --worker <DEVICE> <MODE: iso|dd> <SCHEME: MBR|GPT> <FS: FAT32|NTFS|exFAT|ext4> [IMAGE_PATH]

# Example:
pkexec rufux --worker /dev/sdb iso GPT NTFS /path/to/windows11.iso
```

---

## Acknowledgments

* **Pete Batard** and contributors to [Rufus](https://github.com/pbatard/rufus) for creating the original utility, format specifications, and `uefi-ntfs.img`.
* Built with [Rust](https://www.rust-lang.org/) and [egui](https://github.com/emilk/egui).

---

## License

This project is licensed under the **GNU General Public License v3.0** (GPLv3). See [LICENSE](LICENSE) for details.
