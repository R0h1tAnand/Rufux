//! Rufux: The Reliable USB Formatting Utility for Linux
//!
//! A 1-to-1 blazingly fast Rust port of Rufus by Pete Batard.

pub mod core;
pub mod ui;

use std::env;
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use eframe::egui;

use crate::core::writer::{execute_burn, BurnMode, BurnOptions, BurnProgress};
use crate::ui::app::RufuxApp;
use crate::ui::theme::configure_rufus_theme;

fn main() -> eframe::Result<()> {
    let args: Vec<String> = env::args().collect();

    // Check if invoked as elevated background worker via pkexec
    if args.len() > 1 && args[1] == "--worker" {
        run_elevated_worker_mode(&args[2..]);
        return Ok(());
    }

    // Launch Rufus 1-to-1 GUI
    let mut native_options = eframe::NativeOptions::default();
    native_options.viewport = egui::ViewportBuilder::default()
        .with_inner_size([470.0, 680.0])
        .with_min_inner_size([440.0, 620.0])
        .with_title("Rufux 1.0.0 (Linux)")
        .with_resizable(true);

    eframe::run_native(
        "Rufux",
        native_options,
        Box::new(|cc| {
            configure_rufus_theme(&cc.egui_ctx);
            Ok(Box::new(RufuxApp::default()))
        }),
    )
}

/// Runs non-interactive elevated worker mode when invoked via pkexec
fn run_elevated_worker_mode(args: &[String]) {
    println!("[Rufux Worker] Elevated worker started with UID 0 (pkexec)");

    if args.len() < 4 {
        eprintln!("[Rufux Worker] Insufficient arguments for worker mode");
        return;
    }

    let dev_path = std::path::PathBuf::from(&args[0]);
    let mode_str = &args[1];
    let part_scheme = &args[2];
    let fs_name = &args[3];

    let image_path = if args.len() > 4 && !args[4].is_empty() {
        Some(std::path::PathBuf::from(&args[4]))
    } else {
        None
    };

    let burn_mode = if mode_str == "dd" {
        BurnMode::DdMode
    } else {
        BurnMode::IsoMode
    };

    let opts = BurnOptions {
        device_path: dev_path,
        image_path,
        burn_mode,
        partition_scheme: part_scheme.clone(),
        target_system: "UEFI (non CSM)".to_string(),
        file_system: fs_name.clone(),
        volume_label: "RUFUX".to_string(),
        quick_format: true,
        persistence_size_mb: 0,
        wue_options: None,
        wue_username: "User".to_string(),
    };

    let cancel_flag = Arc::new(AtomicBool::new(false));

    execute_burn(&opts, None, cancel_flag, |p| match p {
        BurnProgress::Status(s) => println!("STATUS: {}", s),
        BurnProgress::Progress { percent, speed_mb_s } => {
            println!("PROGRESS: {:.1}% ({:.1} MB/s)", percent * 100.0, speed_mb_s)
        }
        BurnProgress::Log(l) => println!("LOG: {}", l),
        BurnProgress::Finished => println!("FINISHED"),
        BurnProgress::Error(e) => eprintln!("ERROR: {}", e),
    });
}
