//! Live Log Window (IDD_LOG)
//!
//! Recreates Rufus's Log console with timestamped entries, autoscroll,
//! clear, and save functionalities.

use egui::{ScrollArea, Window};
use std::fs;

#[derive(Default)]
pub struct LogDialog {
    pub is_open: bool,
    pub entries: Vec<String>,
}

impl LogDialog {
    pub fn log(&mut self, message: &str) {
        let timestamp = chrono::Local::now().format("%H:%M:%S").to_string();
        self.entries.push(format!("[{}] {}", timestamp, message));
    }

    pub fn show(&mut self, ctx: &egui::Context) {
        if !self.is_open {
            return;
        }

        Window::new("Rufus Log")
            .collapsible(true)
            .resizable(true)
            .default_size(egui::vec2(520.0, 420.0))
            .show(ctx, |ui| {
                ScrollArea::vertical()
                    .stick_to_bottom(true)
                    .max_height(340.0)
                    .show(ui, |ui| {
                        for entry in &self.entries {
                            ui.label(egui::RichText::new(entry).monospace().size(11.0));
                        }
                    });

                ui.add_space(8.0);
                ui.separator();
                ui.add_space(4.0);

                ui.horizontal(|ui| {
                    if ui.button("Clear").clicked() {
                        self.entries.clear();
                    }

                    if ui.button("Save to File").clicked() {
                        let text = self.entries.join("\n");
                        let _ = fs::write("rufux.log", text);
                    }

                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.button("Close").clicked() {
                            self.is_open = false;
                        }
                    });
                });
            });
    }
}
