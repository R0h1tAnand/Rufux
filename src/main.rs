//! Rufux: The Reliable USB Formatting Utility for Linux
//!
//! A native GTK4 take on Rufus by Pete Batard.

pub mod core;
pub mod ui;

use std::env;

const VERSION: &str = env!("CARGO_PKG_VERSION");

fn main() -> gtk4::glib::ExitCode {
    let args: Vec<String> = env::args().skip(1).collect();

    match args.first().map(String::as_str) {
        // Elevated background worker, started by the GUI through pkexec.
        Some("--worker") => std::process::exit(core::worker::run_worker(&args[1..])),
        Some("--version") | Some("-V") => {
            println!("rufux {}", VERSION);
            return gtk4::glib::ExitCode::SUCCESS;
        }
        Some("--help") | Some("-h") => {
            println!("Rufux {VERSION}\n\nUsage:\n  rufux                      start the graphical interface\n  rufux --version            print the version\n  pkexec rufux --worker <DEVICE> <iso|dd> <MBR|GPT> <FAT32|NTFS|exFAT|ext4> [IMAGE] [--allow-fixed]\n                             write a drive without the GUI (root required)");
            return gtk4::glib::ExitCode::SUCCESS;
        }
        _ => {}
    }

    ui::window::run()
}
