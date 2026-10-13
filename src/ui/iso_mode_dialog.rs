//! ISOHybrid Mode Selection Modal Dialog
//!
//! Replicates Rufus's prompt for selecting between ISO extraction mode and raw DD mode.

use crate::core::writer::BurnMode;
use egui::Window;

pub struct IsoModeDialog {
    pub is_open: bool,
    pub selected_mode: BurnMode,
    pub confirmed: bool,
}

impl Default for IsoModeDialog {
    fn default() -> Self {
        Self {
            is_open: false,
            selected_mode: BurnMode::IsoMode,
            confirmed: false,
        }
    }
}

impl IsoModeDialog {
    pub fn show(&mut self, ctx: &egui::Context) -> Option<BurnMode> {
        let mut result = None;

        if !self.is_open {
            return None;
        }

        Window::new("ISOHybrid image detected")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
            .show(ctx, |ui| {
                ui.set_width(420.0);
                ui.add_space(4.0);

                ui.label(
                    "The image you have selected is an 'ISOHybrid' image. This means it can be \
                     written either in ISO Image mode (file copy) or DD Image mode (raw disk write).\n\n\
                     Rufux recommends using ISO Image mode so that you always have full access to the \
                     drive after writing.",
                );

                ui.add_space(8.0);

                ui.radio_value(
                    &mut self.selected_mode,
                    BurnMode::IsoMode,
                    "Write in ISO Image mode (Recommended)",
                );

                ui.radio_value(&mut self.selected_mode, BurnMode::DdMode, "Write in DD Image mode");

                ui.add_space(12.0);
                ui.separator();
                ui.add_space(6.0);

                ui.horizontal(|ui| {
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.button("Cancel").clicked() {
                            self.is_open = false;
                            result = None;
                        }
                        if ui.button(egui::RichText::new("OK").strong()).clicked() {
                            self.is_open = false;
                            self.confirmed = true;
                            result = Some(self.selected_mode);
                        }
                    });
                });
            });

        result
    }
}
