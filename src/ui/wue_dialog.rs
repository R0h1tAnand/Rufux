//! Windows User Experience Customization Modal Dialog
//!
//! Matches Rufus's popup dialog for customizing Windows 10/11 installations.

use egui::Window;
use crate::core::wue::WueOptions;

pub struct WueDialog {
    pub is_open: bool,
    pub options: WueOptions,
    pub username: String,
    pub confirmed: bool,
}

impl Default for WueDialog {
    fn default() -> Self {
        let default_user = std::env::var("USER").unwrap_or_else(|_| "User".to_string());
        Self {
            is_open: false,
            options: WueOptions::default(),
            username: default_user,
            confirmed: false,
        }
    }
}

impl WueDialog {
    pub fn show(&mut self, ctx: &egui::Context) -> Option<bool> {
        let mut result = None;

        if !self.is_open {
            return None;
        }

        Window::new("Windows User Experience")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
            .show(ctx, |ui| {
                ui.set_width(380.0);
                ui.add_space(4.0);

                ui.label(egui::RichText::new("Customize Windows installation:").strong());
                ui.add_space(8.0);

                ui.checkbox(
                    &mut self.options.bypass_tpm_secureboot_ram,
                    "Remove requirement for 4GB+ RAM, Secure Boot and TPM 2.0",
                );

                ui.checkbox(
                    &mut self.options.bypass_online_account,
                    "Remove requirement for an online Microsoft account",
                );

                ui.horizontal(|ui| {
                    ui.checkbox(&mut self.options.create_local_account, "Create a local account with username:");
                    if self.options.create_local_account {
                        ui.text_edit_singleline(&mut self.username);
                    }
                });

                ui.checkbox(
                    &mut self.options.set_locale,
                    "Set regional options to the same values as this user's",
                );

                ui.checkbox(
                    &mut self.options.disable_bitlocker,
                    "Disable BitLocker automatic device encryption",
                );

                ui.checkbox(
                    &mut self.options.disable_data_collection,
                    "Disable data collection (Skip privacy questions)",
                );

                ui.add_space(14.0);
                ui.separator();
                ui.add_space(6.0);

                ui.horizontal(|ui| {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.button("Cancel").clicked() {
                            self.is_open = false;
                            result = Some(false);
                        }
                        if ui.button(egui::RichText::new("OK").strong()).clicked() {
                            self.is_open = false;
                            self.confirmed = true;
                            result = Some(true);
                        }
                    });
                });
            });

        result
    }
}
