//! Rufus Win32 Dark & Light Theme Styling for egui
//!
//! Recreates the dense, clean, professional look-and-feel of Rufus.

use egui::{Color32, Stroke, Visuals};

pub fn configure_rufus_theme(ctx: &egui::Context) {
    let mut style = (*ctx.style()).clone();

    // Font sizing matching Segoe UI 9pt in Win32
    style.text_styles = [
        (
            egui::TextStyle::Heading,
            egui::FontId::new(14.0, egui::FontFamily::Proportional),
        ),
        (
            egui::TextStyle::Body,
            egui::FontId::new(12.5, egui::FontFamily::Proportional),
        ),
        (
            egui::TextStyle::Monospace,
            egui::FontId::new(11.5, egui::FontFamily::Monospace),
        ),
        (
            egui::TextStyle::Button,
            egui::FontId::new(12.0, egui::FontFamily::Proportional),
        ),
        (
            egui::TextStyle::Small,
            egui::FontId::new(10.5, egui::FontFamily::Proportional),
        ),
    ]
    .into();

    style.spacing.item_spacing = egui::vec2(6.0, 5.0);
    style.spacing.button_padding = egui::vec2(10.0, 4.0);
    style.spacing.combo_width = 180.0;

    let mut visuals = Visuals::dark();

    // Dark theme matching Rufus dark mode
    visuals.window_fill = Color32::from_rgb(32, 32, 32);
    visuals.panel_fill = Color32::from_rgb(32, 32, 32);
    visuals.faint_bg_color = Color32::from_rgb(40, 40, 40);
    visuals.extreme_bg_color = Color32::from_rgb(24, 24, 24);

    // Widget states
    visuals.widgets.noninteractive.bg_fill = Color32::from_rgb(38, 38, 38);
    visuals.widgets.noninteractive.bg_stroke = Stroke::new(1.0, Color32::from_rgb(55, 55, 55));
    visuals.widgets.noninteractive.fg_stroke = Stroke::new(1.0, Color32::from_rgb(220, 220, 220));

    visuals.widgets.inactive.bg_fill = Color32::from_rgb(45, 45, 45);
    visuals.widgets.inactive.bg_stroke = Stroke::new(1.0, Color32::from_rgb(65, 65, 65));
    visuals.widgets.inactive.fg_stroke = Stroke::new(1.0, Color32::from_rgb(230, 230, 230));

    visuals.widgets.hovered.bg_fill = Color32::from_rgb(60, 60, 60);
    visuals.widgets.hovered.bg_stroke = Stroke::new(1.0, Color32::from_rgb(100, 100, 100));
    visuals.widgets.hovered.fg_stroke = Stroke::new(1.0, Color32::from_rgb(255, 255, 255));

    visuals.widgets.active.bg_fill = Color32::from_rgb(70, 70, 70);
    visuals.widgets.active.bg_stroke = Stroke::new(1.0, Color32::from_rgb(0, 120, 215));
    visuals.widgets.active.fg_stroke = Stroke::new(1.0, Color32::from_rgb(255, 255, 255));

    visuals.selection.bg_fill = Color32::from_rgb(0, 120, 215); // Windows highlight blue
    visuals.selection.stroke = Stroke::new(1.0, Color32::WHITE);

    ctx.set_style(style);
    ctx.set_visuals(visuals);
}

/// Draws a section header matching Rufus (e.g. "Drive Properties" followed by a thin horizontal rule)
pub fn draw_section_header(ui: &mut egui::Ui, title: &str) {
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        ui.label(
            egui::RichText::new(title)
                .strong()
                .color(Color32::from_rgb(70, 160, 245)),
        );
        let rect = ui.available_rect_before_wrap();
        let y = rect.center().y;
        ui.painter().line_segment(
            [egui::pos2(rect.min.x + 8.0, y), egui::pos2(rect.max.x, y)],
            Stroke::new(1.0, Color32::from_rgb(60, 60, 60)),
        );
    });
    ui.add_space(2.0);
}
