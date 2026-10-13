//! Rufux: The Reliable USB Formatting Utility for Linux
//!
//! A Rust port of Rufus by Pete Batard.

pub mod core;
pub mod ui;

use std::env;

use eframe::egui;

use crate::ui::app::RufuxApp;
use crate::ui::theme::configure_rufus_theme;

const VERSION: &str = env!("CARGO_PKG_VERSION");

fn main() -> eframe::Result<()> {
    let args: Vec<String> = env::args().skip(1).collect();

    match args.first().map(String::as_str) {
        // Elevated background worker, started by the GUI through pkexec.
        Some("--worker") => std::process::exit(core::worker::run_worker(&args[1..])),
        Some("--version") | Some("-V") => {
            println!("rufux {}", VERSION);
            return Ok(());
        }
        Some("--help") | Some("-h") => {
            println!("Rufux {VERSION}\n\nUsage:\n  rufux                      start the graphical interface\n  rufux --version            print the version\n  pkexec rufux --worker <DEVICE> <iso|dd> <MBR|GPT> <FAT32|NTFS|exFAT|ext4> [IMAGE] [--allow-fixed]\n                             write a drive without the GUI (root required)");
            return Ok(());
        }
        _ => {}
    }

    let viewport = egui::ViewportBuilder::default()
        .with_inner_size([470.0, 680.0])
        .with_min_inner_size([440.0, 620.0])
        .with_title(format!("Rufux {} (Linux)", VERSION))
        .with_resizable(true);
    let native_options = eframe::NativeOptions {
        viewport,
        ..Default::default()
    };

    eframe::run_native(
        "Rufux",
        native_options,
        Box::new(|cc| {
            configure_rufus_theme(&cc.egui_ctx);
            Ok(Box::new(RufuxApp::default()))
        }),
    )
}
