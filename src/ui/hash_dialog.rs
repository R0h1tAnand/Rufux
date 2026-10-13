//! Checksums Modal Dialog (# Button)
//!
//! Recreates Rufus's Checksums dialog for computing and verifying image hashes.

use crate::core::hasher::ImageHashes;
use egui::{Color32, Window};

#[derive(Default)]
pub struct HashDialog {
    pub is_open: bool,
    pub is_computing: bool,
    pub progress: f32,
    pub hashes: Option<ImageHashes>,
    pub compare_text: String,
}

impl HashDialog {
    pub fn show(&mut self, ctx: &egui::Context) {
        if !self.is_open {
            return;
        }

        Window::new("Checksums")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
            .show(ctx, |ui| {
                ui.set_width(450.0);
                ui.add_space(4.0);

                if self.is_computing {
                    ui.label("Computing image checksums (MD5, SHA1, SHA256, SHA512)...");
                    ui.add(egui::ProgressBar::new(self.progress).show_percentage());
                    ui.add_space(8.0);
                }

                let (md5_str, sha1_str, sha256_str, sha512_str) = if let Some(ref h) = self.hashes {
                    (h.md5.as_str(), h.sha1.as_str(), h.sha256.as_str(), h.sha512.as_str())
                } else {
                    ("...", "...", "...", "...")
                };

                // MD5
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new("MD5:").strong());
                    ui.add(egui::TextEdit::singleline(&mut md5_str.to_string()).desired_width(340.0));
                    if ui.button("📋").on_hover_text("Copy MD5").clicked() {
                        ui.output_mut(|o| o.copied_text = md5_str.to_string());
                    }
                });

                // SHA1
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new("SHA1:").strong());
                    ui.add(egui::TextEdit::singleline(&mut sha1_str.to_string()).desired_width(340.0));
                    if ui.button("📋").on_hover_text("Copy SHA1").clicked() {
                        ui.output_mut(|o| o.copied_text = sha1_str.to_string());
                    }
                });

                // SHA256
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new("SHA256:").strong());
                    ui.add(egui::TextEdit::singleline(&mut sha256_str.to_string()).desired_width(340.0));
                    if ui.button("📋").on_hover_text("Copy SHA256").clicked() {
                        ui.output_mut(|o| o.copied_text = sha256_str.to_string());
                    }
                });

                // SHA512
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new("SHA512:").strong());
                    ui.add(egui::TextEdit::singleline(&mut sha512_str.to_string()).desired_width(340.0));
                    if ui.button("📋").on_hover_text("Copy SHA512").clicked() {
                        ui.output_mut(|o| o.copied_text = sha512_str.to_string());
                    }
                });

                ui.add_space(10.0);
                ui.separator();
                ui.add_space(4.0);

                // Verification check
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new("Verify:").strong());
                    ui.add(
                        egui::TextEdit::singleline(&mut self.compare_text)
                            .hint_text("Paste official checksum to verify...")
                            .desired_width(280.0),
                    );

                    let clean_compare = self.compare_text.trim().to_lowercase();
                    if !clean_compare.is_empty() {
                        if clean_compare == md5_str
                            || clean_compare == sha1_str
                            || clean_compare == sha256_str
                            || clean_compare == sha512_str
                        {
                            ui.colored_label(Color32::from_rgb(40, 200, 40), "MATCH ✓");
                        } else {
                            ui.colored_label(Color32::from_rgb(230, 60, 60), "MISMATCH ✗");
                        }
                    }
                });

                ui.add_space(10.0);
                ui.separator();
                ui.add_space(4.0);

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button("Close").clicked() {
                        self.is_open = false;
                    }
                });
            });
    }
}
