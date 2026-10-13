//! Main Application Window - 1-to-1 Exact Clone of Rufus
//!
//! Replicates the iconic layout, control groupings, options, and workflows of Rufus.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use crossbeam_channel::{unbounded, Receiver};
use egui::Color32;

use crate::core::device::{enumerate_devices, DeviceInfo};
use crate::core::elevation::is_root;
use crate::core::hasher::compute_hashes;
use crate::core::iso_scanner::{scan_image, ImageType, IsoReport};
use crate::core::writer::{execute_burn, BurnMode, BurnOptions, BurnProgress};

use super::about_dialog::AboutDialog;
use super::hash_dialog::HashDialog;
use super::iso_mode_dialog::IsoModeDialog;
use super::log_dialog::LogDialog;
use super::theme::draw_section_header;
use super::wue_dialog::WueDialog;

pub struct RufuxApp {
    // Hardware & Devices
    pub devices: Vec<DeviceInfo>,
    pub selected_device_idx: usize,
    pub list_usb_hard_drives: bool,
    pub old_bios_fixes: bool,
    pub uefi_media_validation: bool,
    pub show_advanced_drive_properties: bool,

    // Boot selection & ISO
    pub boot_selection_idx: usize,
    pub image_path: Option<PathBuf>,
    pub iso_report: Option<IsoReport>,
    pub image_option_idx: usize,
    pub persistence_size_mb: u64,

    // Partition & Target
    pub partition_scheme_idx: usize,
    pub target_system_idx: usize,

    // Format Options
    pub volume_label: String,
    pub file_system_idx: usize,
    pub cluster_size_idx: usize,
    pub quick_format: bool,
    pub extended_label: bool,
    pub check_bad_blocks: bool,
    pub bad_blocks_passes: usize,
    pub show_advanced_format_options: bool,

    // Progress & Status
    pub status_text: String,
    pub progress_percent: f32,
    pub transfer_speed: f32,
    pub is_burning: bool,
    pub cancel_flag: Arc<AtomicBool>,

    // Channels for async operations
    pub burn_rx: Option<Receiver<BurnProgress>>,
    pub hash_rx: Option<Receiver<(f32, Option<crate::core::hasher::ImageHashes>)>>,

    // Modals
    pub wue_dialog: WueDialog,
    pub hash_dialog: HashDialog,
    pub iso_mode_dialog: IsoModeDialog,
    pub log_dialog: LogDialog,
    pub about_dialog: AboutDialog,
}

impl Default for RufuxApp {
    fn default() -> Self {
        let mut app = Self {
            devices: Vec::new(),
            selected_device_idx: 0,
            list_usb_hard_drives: false,
            old_bios_fixes: false,
            uefi_media_validation: false,
            show_advanced_drive_properties: false,

            boot_selection_idx: 0,
            image_path: None,
            iso_report: None,
            image_option_idx: 0,
            persistence_size_mb: 0,

            partition_scheme_idx: 1, // GPT default
            target_system_idx: 1,    // UEFI default

            volume_label: "RUFUX".to_string(),
            file_system_idx: 0,      // FAT32 / Large FAT32
            cluster_size_idx: 0,     // Default
            quick_format: true,
            extended_label: false,
            check_bad_blocks: false,
            bad_blocks_passes: 0,    // 1 pass
            show_advanced_format_options: false,

            status_text: "READY".to_string(),
            progress_percent: 0.0,
            transfer_speed: 0.0,
            is_burning: false,
            cancel_flag: Arc::new(AtomicBool::new(false)),
            burn_rx: None,
            hash_rx: None,

            wue_dialog: WueDialog::default(),
            hash_dialog: HashDialog::default(),
            iso_mode_dialog: IsoModeDialog::default(),
            log_dialog: LogDialog::default(),
            about_dialog: AboutDialog::default(),
        };

        app.refresh_devices();
        app.log_dialog.log("Rufux initialized successfully (Linux Port of Rufus)");
        if is_root() {
            app.log_dialog.log("Running with elevated root privileges.");
        } else {
            app.log_dialog.log("Running in standard user mode (Polkit pkexec will prompt on write).");
        }
        app
    }
}

impl RufuxApp {
    pub fn refresh_devices(&mut self) {
        match enumerate_devices(self.list_usb_hard_drives) {
            Ok(devs) => {
                self.devices = devs;
                if self.selected_device_idx >= self.devices.len() {
                    self.selected_device_idx = 0;
                }
            }
            Err(e) => {
                self.log_dialog.log(&format!("Error enumerating devices: {}", e));
            }
        }
    }

    pub fn select_image_file(&mut self) {
        let output = std::process::Command::new("zenity")
            .args([
                "--file-selection",
                "--title=Select Bootable ISO or Disk Image",
                "--file-filter=Disk Images (*.iso, *.img, *.raw, *.vhd) | *.iso *.img *.raw *.vhd *.dd *.gz *.xz",
                "--file-filter=All Files | *",
            ])
            .output();

        if let Ok(out) = output {
            if out.status.success() {
                let path_str = String::from_utf8_lossy(&out.stdout).trim().to_string();
                if !path_str.is_empty() {
                    self.set_image(PathBuf::from(path_str));
                }
            }
        }
    }

    pub fn set_image(&mut self, path: PathBuf) {
        self.log_dialog.log(&format!("Selected image: {}", path.display()));
        match scan_image(&path) {
            Ok(rep) => {
                self.log_dialog.log(&format!(
                    "Image scanned: Type={:?}, Label='{}', Size={}, EFI={}, BIOS={}",
                    rep.image_type, rep.volume_label, rep.size_bytes, rep.is_efi_bootable, rep.is_bios_bootable
                ));

                if !rep.volume_label.is_empty() {
                    self.volume_label = rep.volume_label.clone();
                }

                if rep.recommended_partition_scheme == "GPT" {
                    self.partition_scheme_idx = 1;
                    self.target_system_idx = 1;
                } else {
                    self.partition_scheme_idx = 0;
                    self.target_system_idx = 0;
                }

                if rep.recommended_file_system == "NTFS" {
                    self.file_system_idx = 1; // NTFS
                } else {
                    self.file_system_idx = 0; // FAT32
                }

                self.iso_report = Some(rep);
                self.image_path = Some(path);
                self.status_text = "READY".to_string();
            }
            Err(e) => {
                self.log_dialog.log(&format!("Error analyzing image: {}", e));
                self.image_path = Some(path);
            }
        }
    }

    pub fn start_hash_computation(&mut self) {
        if let Some(ref img_path) = self.image_path {
            let path = img_path.clone();
            self.hash_dialog.is_open = true;
            self.hash_dialog.is_computing = true;
            self.hash_dialog.progress = 0.0;
            self.hash_dialog.hashes = None;

            let (tx, rx) = unbounded();
            thread::spawn(move || {
                let res = compute_hashes(&path, |p| {
                    let _ = tx.send((p, None));
                });
                if let Ok(hashes) = res {
                    let _ = tx.send((1.0, Some(hashes)));
                }
            });

            self.hash_rx = Some(rx);
        }
    }

    pub fn trigger_start(&mut self) {
        if self.devices.is_empty() {
            self.status_text = "ERROR: No target device selected".to_string();
            return;
        }

        let is_windows = self.iso_report.as_ref().map(|r| r.image_type == ImageType::Windows).unwrap_or(false);
        let is_isohybrid = self.iso_report.as_ref().map(|r| r.is_isohybrid).unwrap_or(false);

        if is_windows {
            self.wue_dialog.is_open = true;
            return;
        }

        if is_isohybrid {
            self.iso_mode_dialog.is_open = true;
            return;
        }

        self.launch_flashing(BurnMode::IsoMode);
    }

    pub fn launch_flashing(&mut self, mode: BurnMode) {
        let dev = match self.devices.get(self.selected_device_idx) {
            Some(d) => d.clone(),
            None => return,
        };

        let part_scheme = if self.partition_scheme_idx == 0 { "MBR" } else { "GPT" };
        let target_sys = if self.target_system_idx == 0 { "BIOS (or UEFI-CSM)" } else { "UEFI (non CSM)" };
        let fs_name = match self.file_system_idx {
            0 => "FAT32",
            1 => "NTFS",
            2 => "exFAT",
            3 => "ext4",
            _ => "FAT32",
        };

        let wue_opts = if self.wue_dialog.confirmed {
            Some(self.wue_dialog.options)
        } else {
            None
        };

        let opts = BurnOptions {
            device_path: dev.path.clone(),
            image_path: self.image_path.clone(),
            burn_mode: mode,
            partition_scheme: part_scheme.to_string(),
            target_system: target_sys.to_string(),
            file_system: fs_name.to_string(),
            volume_label: self.volume_label.clone(),
            quick_format: self.quick_format,
            persistence_size_mb: self.persistence_size_mb,
            wue_options: wue_opts,
            wue_username: self.wue_dialog.username.clone(),
        };

        let (tx, rx) = unbounded();
        self.burn_rx = Some(rx);
        self.is_burning = true;
        self.cancel_flag.store(false, Ordering::Relaxed);
        let cancel = self.cancel_flag.clone();
        let report_clone = self.iso_report.clone();

        self.log_dialog.log(&format!("Starting burn process in {:?} on {}", mode, dev.path.display()));

        thread::spawn(move || {
            let tx_clone = tx.clone();
            execute_burn(&opts, report_clone.as_ref(), cancel, move |progress| {
                let _ = tx_clone.send(progress);
            });
        });
    }
}

impl eframe::App for RufuxApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        // Poll worker messages
        if let Some(ref rx) = self.burn_rx {
            while let Ok(msg) = rx.try_recv() {
                match msg {
                    BurnProgress::Status(s) => self.status_text = s,
                    BurnProgress::Progress { percent, speed_mb_s } => {
                        self.progress_percent = percent;
                        self.transfer_speed = speed_mb_s;
                    }
                    BurnProgress::Log(l) => self.log_dialog.log(&l),
                    BurnProgress::Finished => {
                        self.is_burning = false;
                        self.status_text = "READY".to_string();
                        self.log_dialog.log("Burn process completed successfully!");
                    }
                    BurnProgress::Error(e) => {
                        self.is_burning = false;
                        self.status_text = format!("ERROR: {}", e);
                        self.log_dialog.log(&format!("FATAL ERROR: {}", e));
                    }
                }
            }
        }

        // Poll hash computation messages
        if let Some(ref rx) = self.hash_rx {
            while let Ok((progress, hashes)) = rx.try_recv() {
                self.hash_dialog.progress = progress;
                if let Some(h) = hashes {
                    self.hash_dialog.hashes = Some(h);
                    self.hash_dialog.is_computing = false;
                }
            }
        }

        // Handle modal completions
        if let Some(confirmed) = self.wue_dialog.show(ctx) {
            if confirmed {
                self.launch_flashing(BurnMode::IsoMode);
            }
        }

        if let Some(mode) = self.iso_mode_dialog.show(ctx) {
            self.launch_flashing(mode);
        }

        self.hash_dialog.show(ctx);
        self.log_dialog.show(ctx);
        self.about_dialog.show(ctx);

        // Main Rufus Window Content
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.set_max_width(460.0);

            // ================= 1. DRIVE PROPERTIES =================
            draw_section_header(ui, "Drive Properties");

            // Device
            ui.label(egui::RichText::new("Device").size(11.0));
            ui.horizontal(|ui| {
                let dev_label = if let Some(d) = self.devices.get(self.selected_device_idx) {
                    d.display_label()
                } else {
                    "No drive detected".to_string()
                };

                egui::ComboBox::from_id_salt("device_combo")
                    .selected_text(&dev_label)
                    .width(380.0)
                    .show_ui(ui, |ui| {
                        for (i, d) in self.devices.iter().enumerate() {
                            ui.selectable_value(&mut self.selected_device_idx, i, d.display_label());
                        }
                    });

                if ui.button("...").on_hover_text("Dump drive to image file").clicked() {
                    // Physical drive dumper
                }
            });

            // Boot selection
            ui.add_space(2.0);
            ui.label(egui::RichText::new("Boot selection").size(11.0));
            ui.horizontal(|ui| {
                let boot_text = if let Some(ref path) = self.image_path {
                    path.file_name().unwrap_or_default().to_string_lossy().to_string()
                } else {
                    "Disk or ISO image (Please select)".to_string()
                };

                let boot_modes = [
                    "Disk or ISO image (Please select)",
                    "FreeDOS",
                    "MS-DOS",
                    "Non bootable",
                    "Syslinux 4.x",
                    "Syslinux 6.x",
                    "UEFI:NTFS",
                ];

                egui::ComboBox::from_id_salt("boot_combo")
                    .selected_text(if self.image_path.is_some() { &boot_text } else { boot_modes[self.boot_selection_idx] })
                    .width(280.0)
                    .show_ui(ui, |ui| {
                        for (i, mode) in boot_modes.iter().enumerate() {
                            ui.selectable_value(&mut self.boot_selection_idx, i, *mode);
                        }
                    });

                if ui.button("#").on_hover_text("Compute MD5, SHA1, SHA256 checksums").clicked() {
                    self.start_hash_computation();
                }

                if ui.button(egui::RichText::new("SELECT").strong()).clicked() {
                    self.select_image_file();
                }
            });

            // Image Option
            let image_options = ["Standard Windows installation", "Windows To Go"];
            ui.add_space(2.0);
            ui.label(egui::RichText::new("Image option").size(11.0));
            egui::ComboBox::from_id_salt("image_option_combo")
                .selected_text(image_options[self.image_option_idx])
                .width(ui.available_width())
                .show_ui(ui, |ui| {
                    for (i, opt) in image_options.iter().enumerate() {
                        ui.selectable_value(&mut self.image_option_idx, i, *opt);
                    }
                });

            // Partition Scheme & Target System (Side by Side)
            ui.add_space(2.0);
            ui.columns(2, |cols| {
                cols[0].label(egui::RichText::new("Partition scheme").size(11.0));
                let schemes = ["MBR", "GPT"];
                egui::ComboBox::from_id_salt("partition_scheme_combo")
                    .selected_text(schemes[self.partition_scheme_idx])
                    .width(cols[0].available_width())
                    .show_ui(&mut cols[0], |ui| {
                        for (i, s) in schemes.iter().enumerate() {
                            ui.selectable_value(&mut self.partition_scheme_idx, i, *s);
                        }
                    });

                cols[1].label(egui::RichText::new("Target system").size(11.0));
                let targets = ["BIOS (or UEFI-CSM)", "UEFI (non CSM)"];
                egui::ComboBox::from_id_salt("target_system_combo")
                    .selected_text(targets[self.target_system_idx])
                    .width(cols[1].available_width())
                    .show_ui(&mut cols[1], |ui| {
                        for (i, t) in targets.iter().enumerate() {
                            ui.selectable_value(&mut self.target_system_idx, i, *t);
                        }
                    });
            });

            // Advanced drive properties toggle
            ui.add_space(2.0);
            ui.collapsing("Show advanced drive properties", |ui| {
                if ui.checkbox(&mut self.list_usb_hard_drives, "List USB Hard Drives").changed() {
                    self.refresh_devices();
                }
                ui.checkbox(&mut self.old_bios_fixes, "Add fixes for old BIOSes (extra partition, align, etc.)");
                ui.checkbox(&mut self.uefi_media_validation, "Enable runtime UEFI media validation");
            });

            // ================= 2. FORMAT OPTIONS =================
            ui.add_space(4.0);
            draw_section_header(ui, "Format Options");

            // Volume Label
            ui.label(egui::RichText::new("Volume label").size(11.0));
            ui.add(egui::TextEdit::singleline(&mut self.volume_label).desired_width(ui.available_width()));

            // File System & Cluster Size (Side by Side)
            ui.add_space(2.0);
            ui.columns(2, |cols| {
                cols[0].label(egui::RichText::new("File system").size(11.0));
                let filesystems = ["Large FAT32 (Default)", "NTFS", "exFAT", "ext4"];
                egui::ComboBox::from_id_salt("file_system_combo")
                    .selected_text(filesystems[self.file_system_idx])
                    .width(cols[0].available_width())
                    .show_ui(&mut cols[0], |ui| {
                        for (i, fs) in filesystems.iter().enumerate() {
                            ui.selectable_value(&mut self.file_system_idx, i, *fs);
                        }
                    });

                cols[1].label(egui::RichText::new("Cluster size").size(11.0));
                let clusters = [
                    "Default (4096 bytes)",
                    "8192 bytes",
                    "16 kilobytes",
                    "32 kilobytes",
                    "64 kilobytes",
                ];
                egui::ComboBox::from_id_salt("cluster_size_combo")
                    .selected_text(clusters[self.cluster_size_idx])
                    .width(cols[1].available_width())
                    .show_ui(&mut cols[1], |ui| {
                        for (i, c) in clusters.iter().enumerate() {
                            ui.selectable_value(&mut self.cluster_size_idx, i, *c);
                        }
                    });
            });

            // Advanced format options toggle
            ui.add_space(2.0);
            ui.collapsing("Show advanced format options", |ui| {
                ui.checkbox(&mut self.quick_format, "Quick format");
                ui.checkbox(&mut self.extended_label, "Create extended label and icon files");
                ui.horizontal(|ui| {
                    ui.checkbox(&mut self.check_bad_blocks, "Check device for bad blocks");
                    if self.check_bad_blocks {
                        let passes = ["1 pass", "2 passes", "3 passes", "4 passes"];
                        egui::ComboBox::from_id_salt("bad_blocks_passes")
                            .selected_text(passes[self.bad_blocks_passes])
                            .show_ui(ui, |ui| {
                                for (i, p) in passes.iter().enumerate() {
                                    ui.selectable_value(&mut self.bad_blocks_passes, i, *p);
                                }
                            });
                    }
                });
            });

            // ================= 3. STATUS AREA =================
            ui.add_space(4.0);
            draw_section_header(ui, "Status");

            ui.label(egui::RichText::new(&self.status_text).size(11.5).strong());

            ui.add(
                egui::ProgressBar::new(self.progress_percent)
                    .show_percentage()
                    .desired_width(ui.available_width())
                    .desired_height(14.0),
            );

            ui.add_space(10.0);
            ui.separator();
            ui.add_space(4.0);

            // Bottom Buttons Bar
            ui.horizontal(|ui| {
                // Settings / About icon button
                if ui.button("⚙").on_hover_text("About Rufux & Settings").clicked() {
                    self.about_dialog.is_open = true;
                }

                // Log console button
                if ui.button("📜 Log").on_hover_text("Open Live Rufus Log").clicked() {
                    self.log_dialog.is_open = true;
                }

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    // CLOSE button
                    let close_label = if self.is_burning { "CANCEL" } else { "CLOSE" };
                    if ui.button(close_label).clicked() {
                        if self.is_burning {
                            self.cancel_flag.store(true, Ordering::Relaxed);
                            self.log_dialog.log("Cancellation requested by user.");
                        } else {
                            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                        }
                    }

                    // START button (Accented blue)
                    let start_btn = egui::Button::new(
                        egui::RichText::new("START").strong().color(Color32::WHITE),
                    )
                    .fill(Color32::from_rgb(0, 120, 215));

                    if ui.add_enabled(!self.is_burning, start_btn).clicked() {
                        self.trigger_start();
                    }
                });
            });
        });

        // Request repaint while burning for smooth animation
        if self.is_burning {
            ctx.request_repaint();
        }
    }
}
