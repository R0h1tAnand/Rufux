//! About Modal Dialog (IDD_ABOUTBOX)
//!
//! Recreates Rufus's About dialog with licensing and project information.

use egui::Window;

#[derive(Default)]
pub struct AboutDialog {
    pub is_open: bool,
}

impl AboutDialog {
    pub fn show(&mut self, ctx: &egui::Context) {
        if !self.is_open {
            return;
        }

        Window::new("About Rufux")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
            .show(ctx, |ui| {
                ui.set_width(380.0);
                ui.add_space(4.0);

                ui.heading("Rufux 1.0.0 (Linux)");
                ui.label(egui::RichText::new("The Reliable USB Formatting Utility for Linux").italics());
                ui.add_space(6.0);

                ui.label("Rufux is a native, blazingly fast 1-to-1 Rust port of the famous Windows Rufus utility created by Pete Batard.");
                ui.add_space(6.0);

                ui.label("Features: UEFI:NTFS, Windows User Experience (TPM/SecureBoot/RAM/Account bypass), ISOHybrid/DD flashing, Bad Blocks testing, Checksums, and Polkit (pkexec) elevation.");
                ui.add_space(6.0);

                ui.label(egui::RichText::new("License: GNU General Public License v3.0").strong());
                ui.label("Copyright © 2026 Rohit Anand / Pete Batard");

                ui.add_space(10.0);
                ui.separator();
                ui.add_space(4.0);

                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button("OK").clicked() {
                        self.is_open = false;
                    }
                });
            });
    }
}
