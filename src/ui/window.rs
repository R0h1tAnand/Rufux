//! Main window: a libadwaita preferences-style form with the status and Start button docked
//! at the bottom.

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use adw::prelude::*;
use crossbeam_channel::{unbounded, Receiver, TryRecvError};
use gtk::{gio, glib};
use gtk4 as gtk;
use libadwaita as adw;

use crate::core::device::{enumerate_devices, DeviceInfo};
use crate::core::elevation::{is_root, run_job, WorkerRequest};
use crate::core::hasher::{compute_hashes, ImageHashes};
use crate::core::iso_scanner::{scan_image, ImageType, IsoReport};
use crate::core::layout::{FileSystem, PartitionScheme, TargetSystem, MIB};
use crate::core::writer::{BurnMode, BurnOptions, BurnProgress};
use crate::core::wue::{detect_locale, WueOptions};

use super::dialogs::{self, HashDialog};

const APP_ID: &str = "org.rufux.Rufux";
const FILE_SYSTEMS: [FileSystem; 4] = [FileSystem::Fat32, FileSystem::Ntfs, FileSystem::ExFat, FileSystem::Ext4];
const FILE_SYSTEM_NAMES: [&str; 4] = ["FAT32 (default)", "NTFS", "exFAT", "ext4"];
const CLUSTER_SIZES: [Option<u32>; 5] = [None, Some(8192), Some(16384), Some(32768), Some(65536)];
const CLUSTER_NAMES: [&str; 5] = ["Default", "8 KiB", "16 KiB", "32 KiB", "64 KiB"];
const PASS_NAMES: [&str; 4] = ["1 pass", "2 passes", "3 passes", "4 passes"];
const NO_DRIVE: &str = "No drive detected";

pub fn run() -> glib::ExitCode {
    let app = adw::Application::builder().application_id(APP_ID).build();
    app.connect_activate(|app| {
        if let Some(win) = app.active_window() {
            win.present();
            return;
        }
        Ui::build(app).window.present();
    });
    app.run_with_args::<&str>(&[])
}

#[derive(Default)]
struct State {
    devices: Vec<DeviceInfo>,
    image: Option<PathBuf>,
    report: Option<IsoReport>,
    wue: WueOptions,
    wue_username: String,
    busy: bool,
}

struct Ui {
    window: adw::ApplicationWindow,
    toasts: adw::ToastOverlay,
    state: RefCell<State>,
    cancel: Arc<AtomicBool>,
    locale: String,
    log: gtk::TextBuffer,

    device_row: adw::ComboRow,
    device_model: gtk::StringList,
    image_row: adw::ActionRow,
    select_btn: gtk::Button,
    hash_btn: gtk::Button,
    dump_btn: gtk::Button,
    persistence_row: adw::SpinRow,
    scheme_row: adw::ComboRow,
    target_row: adw::ComboRow,
    usb_hdd_row: adw::SwitchRow,

    label_row: adw::EntryRow,
    fs_row: adw::ComboRow,
    cluster_row: adw::ComboRow,
    quick_row: adw::SwitchRow,
    badblocks_row: adw::SwitchRow,
    passes_row: adw::ComboRow,

    status: gtk::Label,
    progress: gtk::ProgressBar,
    start_btn: gtk::Button,
    cancel_btn: gtk::Button,
}

fn combo(title: &str, items: &[&str], selected: u32) -> adw::ComboRow {
    let row = adw::ComboRow::builder()
        .title(title)
        .model(&gtk::StringList::new(items))
        .build();
    row.set_selected(selected);
    row
}

fn icon_button(icon: &str, tip: &str) -> gtk::Button {
    let b = gtk::Button::from_icon_name(icon);
    b.set_tooltip_text(Some(tip));
    b.set_valign(gtk::Align::Center);
    b.add_css_class("flat");
    b
}

impl Ui {
    fn build(app: &adw::Application) -> Rc<Self> {
        // ---- Drive properties
        let device_model = gtk::StringList::new(&[]);
        let device_row = adw::ComboRow::builder().title("Device").model(&device_model).build();
        let dump_btn = icon_button("document-save-symbolic", "Save the drive to an image file");
        device_row.add_suffix(&dump_btn);

        let image_row = adw::ActionRow::builder()
            .title("Boot image")
            .subtitle("Choose an ISO or disk image, or drop one here")
            .build();
        let hash_btn = icon_button("view-list-symbolic", "Compute checksums");
        hash_btn.set_sensitive(false);
        let select_btn = gtk::Button::with_label("Select…");
        select_btn.set_valign(gtk::Align::Center);
        select_btn.add_css_class("suggested-action");
        image_row.add_suffix(&hash_btn);
        image_row.add_suffix(&select_btn);
        image_row.set_activatable_widget(Some(&select_btn));

        let persistence_row = adw::SpinRow::new(Some(&gtk::Adjustment::new(0.0, 0.0, 0.0, 64.0, 512.0, 0.0)), 64.0, 0);
        persistence_row.set_title("Persistent storage (MiB)");
        persistence_row.set_subtitle("Keeps files and settings between boots. 0 disables it.");
        persistence_row.set_visible(false);

        let scheme_row = combo("Partition scheme", &["MBR", "GPT"], 1);
        let target_row = combo("Target system", &["BIOS or UEFI-CSM", "UEFI (non-CSM)"], 1);

        let usb_hdd_row = adw::SwitchRow::builder()
            .title("List USB hard drives")
            .subtitle("Also show non-removable external disks")
            .build();
        let advanced_drive = adw::ExpanderRow::builder().title("Advanced drive options").build();
        advanced_drive.add_row(&usb_hdd_row);

        let drive_group = adw::PreferencesGroup::builder().title("Drive").build();
        for w in [
            device_row.upcast_ref::<gtk::Widget>(),
            image_row.upcast_ref(),
            persistence_row.upcast_ref(),
            scheme_row.upcast_ref(),
            target_row.upcast_ref(),
            advanced_drive.upcast_ref(),
        ] {
            drive_group.add(w);
        }

        // ---- Format options
        let label_row = adw::EntryRow::builder().title("Volume label").text("RUFUX").build();
        let fs_row = combo("File system", &FILE_SYSTEM_NAMES, 0);
        let cluster_row = combo("Cluster size", &CLUSTER_NAMES, 0);
        let quick_row = adw::SwitchRow::builder().title("Quick format").active(true).build();
        let badblocks_row = adw::SwitchRow::builder()
            .title("Check device for bad blocks")
            .subtitle("Write and read-back test; slow, but exposes fake or failing drives")
            .build();
        let passes_row = combo("Test passes", &PASS_NAMES, 0);
        badblocks_row
            .bind_property("active", &passes_row, "visible")
            .sync_create()
            .build();
        let advanced_format = adw::ExpanderRow::builder().title("Advanced format options").build();
        for w in [
            quick_row.upcast_ref::<gtk::Widget>(),
            badblocks_row.upcast_ref(),
            passes_row.upcast_ref(),
        ] {
            advanced_format.add_row(w);
        }

        let format_group = adw::PreferencesGroup::builder().title("Format").build();
        for w in [
            label_row.upcast_ref::<gtk::Widget>(),
            fs_row.upcast_ref(),
            cluster_row.upcast_ref(),
            advanced_format.upcast_ref(),
        ] {
            format_group.add(w);
        }

        let page = adw::PreferencesPage::new();
        page.add(&drive_group);
        page.add(&format_group);

        // ---- Bottom bar
        let status = gtk::Label::builder()
            .label("Ready")
            .xalign(0.0)
            .hexpand(true)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .build();
        status.add_css_class("heading");
        let progress = gtk::ProgressBar::new();
        progress.set_visible(false);
        let start_btn = gtk::Button::with_label("Start");
        start_btn.add_css_class("suggested-action");
        start_btn.add_css_class("pill");
        let cancel_btn = gtk::Button::with_label("Cancel");
        cancel_btn.add_css_class("pill");
        cancel_btn.set_visible(false);

        let buttons = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        buttons.append(&status);
        buttons.append(&cancel_btn);
        buttons.append(&start_btn);
        let bottom = gtk::Box::new(gtk::Orientation::Vertical, 10);
        bottom.set_margin_top(10);
        bottom.set_margin_bottom(12);
        bottom.set_margin_start(18);
        bottom.set_margin_end(18);
        bottom.append(&progress);
        bottom.append(&buttons);

        // ---- Window chrome
        let log_btn = icon_button("utilities-terminal-symbolic", "Show log");
        let refresh_btn = icon_button("view-refresh-symbolic", "Rescan drives");
        let about_btn = icon_button("help-about-symbolic", "About Rufux");
        let header = adw::HeaderBar::new();
        header.pack_start(&refresh_btn);
        header.pack_end(&about_btn);
        header.pack_end(&log_btn);

        let toolbar = adw::ToolbarView::new();
        toolbar.add_top_bar(&header);
        toolbar.set_content(Some(&page));
        toolbar.add_bottom_bar(&bottom);
        toolbar.set_bottom_bar_style(adw::ToolbarStyle::Raised);
        let toasts = adw::ToastOverlay::new();
        toasts.set_child(Some(&toolbar));

        let window = adw::ApplicationWindow::builder()
            .application(app)
            .title("Rufux")
            .default_width(560)
            .default_height(780)
            .content(&toasts)
            .build();

        let ui = Rc::new(Self {
            window,
            toasts,
            state: RefCell::new(State {
                wue_username: std::env::var("USER").unwrap_or_else(|_| "user".into()),
                ..State::default()
            }),
            cancel: Arc::new(AtomicBool::new(false)),
            locale: detect_locale(),
            log: gtk::TextBuffer::new(None),
            device_row,
            device_model,
            image_row,
            select_btn,
            hash_btn,
            dump_btn,
            persistence_row,
            scheme_row,
            target_row,
            usb_hdd_row,
            label_row,
            fs_row,
            cluster_row,
            quick_row,
            badblocks_row,
            passes_row,
            status,
            progress,
            start_btn,
            cancel_btn,
        });
        ui.wire(&log_btn, &refresh_btn, &about_btn);

        ui.log("Rufux started");
        ui.log(if is_root() {
            "Running as root"
        } else {
            "Running unprivileged; authentication is requested when a drive is written"
        });
        ui.refresh_devices();
        ui
    }

    fn wire(self: &Rc<Self>, log_btn: &gtk::Button, refresh_btn: &gtk::Button, about_btn: &gtk::Button) {
        let ui = self;
        macro_rules! on {
            ($w:expr, $sig:ident, $f:expr) => {{
                let ui = ui.clone();
                #[allow(clippy::redundant_closure_call)]
                $w.$sig(move |_| ($f)(&ui));
            }};
        }
        on!(log_btn, connect_clicked, |u: &Rc<Self>| dialogs::show_log(
            &u.window, &u.log
        ));
        on!(about_btn, connect_clicked, |u: &Rc<Self>| dialogs::show_about(
            &u.window
        ));
        on!(refresh_btn, connect_clicked, |u: &Rc<Self>| u.refresh_devices());
        on!(self.select_btn, connect_clicked, |u: &Rc<Self>| u.choose_image());
        on!(self.hash_btn, connect_clicked, |u: &Rc<Self>| u.start_hashing());
        on!(self.dump_btn, connect_clicked, |u: &Rc<Self>| u.start_dump());
        on!(self.start_btn, connect_clicked, |u: &Rc<Self>| u.on_start());
        on!(self.cancel_btn, connect_clicked, |u: &Rc<Self>| {
            u.cancel.store(true, Ordering::Relaxed);
            u.log("Cancellation requested");
        });
        on!(self.device_row, connect_selected_notify, |u: &Rc<Self>| u
            .update_persistence_range());
        on!(self.usb_hdd_row, connect_active_notify, |u: &Rc<Self>| u
            .refresh_devices());

        // Drag an image onto the window.
        let drop = gtk::DropTarget::new(gio::File::static_type(), gtk::gdk::DragAction::COPY);
        drop.connect_drop({
            let ui = ui.clone();
            move |_, value, _, _| match value.get::<gio::File>().ok().and_then(|f| f.path()) {
                Some(path) if !ui.state.borrow().busy => {
                    ui.set_image(path);
                    true
                }
                _ => false,
            }
        });
        self.window.add_controller(drop);

        self.window.connect_close_request({
            let ui = ui.clone();
            move |_| {
                ui.cancel.store(true, Ordering::Relaxed);
                glib::Propagation::Proceed
            }
        });
    }

    // ------------------------------------------------------------------ helpers

    fn log(&self, message: &str) {
        let stamp = chrono::Local::now().format("%H:%M:%S");
        self.log
            .insert(&mut self.log.end_iter(), &format!("[{stamp}] {message}\n"));
    }

    fn set_status(&self, text: &str, error: bool) {
        self.status.set_text(text);
        if error {
            self.status.add_css_class("error");
        } else {
            self.status.remove_css_class("error");
        }
    }

    fn toast(&self, text: &str) {
        self.toasts.add_toast(adw::Toast::new(text));
    }

    fn selected_device(&self) -> Option<DeviceInfo> {
        let idx = self.device_row.selected() as usize;
        self.state.borrow().devices.get(idx).cloned()
    }

    fn set_busy(&self, busy: bool) {
        self.state.borrow_mut().busy = busy;
        self.start_btn.set_visible(!busy);
        self.cancel_btn.set_visible(busy);
        for w in [
            self.device_row.upcast_ref::<gtk::Widget>(),
            self.image_row.upcast_ref(),
            self.persistence_row.upcast_ref(),
            self.scheme_row.upcast_ref(),
            self.target_row.upcast_ref(),
            self.label_row.upcast_ref(),
            self.fs_row.upcast_ref(),
            self.cluster_row.upcast_ref(),
        ] {
            w.set_sensitive(!busy);
        }
    }

    fn parent_window(&self) -> &adw::ApplicationWindow {
        &self.window
    }

    // ------------------------------------------------------------------ devices

    fn refresh_devices(&self) {
        let previous = self.selected_device().map(|d| d.path);
        let list_fixed = self.usb_hdd_row.is_active();
        match enumerate_devices(list_fixed) {
            Ok(devices) => {
                let labels: Vec<String> = devices.iter().map(DeviceInfo::display_label).collect();
                let refs: Vec<&str> = if labels.is_empty() {
                    vec![NO_DRIVE]
                } else {
                    labels.iter().map(String::as_str).collect()
                };
                let keep = previous
                    .and_then(|p| devices.iter().position(|d| d.path == p))
                    .unwrap_or(0) as u32;
                self.state.borrow_mut().devices = devices;
                self.device_model.splice(0, self.device_model.n_items(), &refs);
                self.device_row.set_selected(keep);
                self.update_persistence_range();
            }
            Err(e) => self.log(&format!("Could not list drives: {e:#}")),
        }
    }

    fn update_persistence_range(&self) {
        let size_mb = self.selected_device().map_or(0, |d| d.size_bytes / MIB);
        let max = (size_mb / 2).min(64 * 1024) as f64;
        let adj = self.persistence_row.adjustment();
        adj.set_upper(max);
        if adj.value() > max {
            adj.set_value(max);
        }
    }

    // ------------------------------------------------------------------ image

    fn choose_image(self: &Rc<Self>) {
        let images = gtk::FileFilter::new();
        images.set_name(Some("Disk images"));
        for ext in ["iso", "img", "raw", "vhd", "dd"] {
            images.add_suffix(ext);
        }
        let all = gtk::FileFilter::new();
        all.set_name(Some("All files"));
        all.add_pattern("*");
        let filters = gio::ListStore::new::<gtk::FileFilter>();
        filters.append(&images);
        filters.append(&all);

        let chooser = gtk::FileDialog::builder()
            .title("Select a bootable ISO or disk image")
            .filters(&filters)
            .build();
        let ui = self.clone();
        chooser.open(Some(&self.window), gio::Cancellable::NONE, move |res| {
            if let Some(path) = res.ok().and_then(|f| f.path()) {
                ui.set_image(path);
            }
        });
    }

    fn set_image(&self, path: PathBuf) {
        self.log(&format!("Selected image: {}", path.display()));
        let name = path.file_name().unwrap_or_default().to_string_lossy().to_string();
        self.persistence_row.set_value(0.0);
        self.state.borrow_mut().wue = WueOptions::default();

        match scan_image(&path) {
            Ok(rep) => {
                self.log(&format!(
                    "Image scanned: type={:?}, label='{}', size={}, EFI={}, BIOS={}",
                    rep.image_type, rep.volume_label, rep.size_bytes, rep.is_efi_bootable, rep.is_bios_bootable
                ));
                if !rep.volume_label.is_empty() {
                    self.label_row.set_text(&rep.volume_label);
                }
                self.scheme_row
                    .set_selected(u32::from(rep.recommended_partition_scheme == PartitionScheme::Gpt));
                self.target_row
                    .set_selected(u32::from(rep.recommended_target_system == TargetSystem::Uefi));
                self.fs_row.set_selected(
                    FILE_SYSTEMS
                        .iter()
                        .position(|f| *f == rep.recommended_file_system)
                        .unwrap_or(0) as u32,
                );
                self.persistence_row.set_visible(rep.persistence.is_some());
                self.image_row.set_subtitle(&format!(
                    "{name} · {:?} · {}",
                    rep.image_type,
                    human_size(rep.size_bytes)
                ));
                self.state.borrow_mut().report = Some(rep);
            }
            Err(e) => {
                self.log(&format!("Could not analyse image: {e:#}"));
                self.persistence_row.set_visible(false);
                self.image_row.set_subtitle(&name);
                self.state.borrow_mut().report = None;
            }
        }
        self.state.borrow_mut().image = Some(path);
        self.hash_btn.set_sensitive(true);
        self.set_status("Ready", false);
    }

    fn start_hashing(self: &Rc<Self>) {
        let Some(path) = self.state.borrow().image.clone() else {
            return;
        };
        let name = path.file_name().unwrap_or_default().to_string_lossy().to_string();
        let dialog = HashDialog::present(&self.window, &name);

        let (tx, rx) = unbounded::<Result<ImageHashes, String>>();
        let (ptx, prx) = unbounded::<f32>();
        thread::spawn(move || {
            let res = compute_hashes(&path, |p| {
                let _ = ptx.send(p);
            });
            let _ = tx.send(res.map_err(|e| format!("{e:#}")));
        });

        let ui = self.clone();
        glib::timeout_add_local(Duration::from_millis(80), move || {
            if let Some(p) = prx.try_iter().last() {
                dialog.set_progress(f64::from(p));
            }
            match rx.try_recv() {
                Ok(Ok(h)) => {
                    dialog.set_hashes(&h);
                    glib::ControlFlow::Break
                }
                Ok(Err(e)) => {
                    ui.log(&format!("Checksum calculation failed: {e}"));
                    ui.toast(&format!("Checksum failed: {e}"));
                    dialog.fail();
                    glib::ControlFlow::Break
                }
                Err(TryRecvError::Empty) => glib::ControlFlow::Continue,
                Err(TryRecvError::Disconnected) => glib::ControlFlow::Break,
            }
        });
    }

    // ------------------------------------------------------------------ start flow

    fn on_start(self: &Rc<Self>) {
        if self.selected_device().is_none() {
            self.set_status("Select a target drive first", true);
            return;
        }
        let (image_type, hybrid) = {
            let st = self.state.borrow();
            (
                st.report.as_ref().map(|r| r.image_type),
                st.report.as_ref().is_some_and(|r| r.is_isohybrid),
            )
        };
        if self.state.borrow().image.is_none() {
            self.set_status("Select an image first", true);
            return;
        }

        match image_type {
            // Raw disk images can only be written block for block.
            Some(ImageType::RawDisk) => self.confirm_and_burn(BurnMode::DdMode),
            Some(ImageType::Windows) => {
                let (opts, user) = {
                    let st = self.state.borrow();
                    (st.wue, st.wue_username.clone())
                };
                let ui = self.clone();
                dialogs::show_wue(&self.window, opts, &user, move |result| {
                    if let Some((opts, user)) = result {
                        let mut st = ui.state.borrow_mut();
                        st.wue = opts;
                        st.wue_username = user;
                        drop(st);
                        ui.confirm_and_burn(BurnMode::IsoMode);
                    }
                });
            }
            _ if hybrid => {
                let ui = self.clone();
                dialogs::ask_iso_mode(&self.window, move |mode| {
                    if let Some(mode) = mode {
                        ui.confirm_and_burn(mode);
                    }
                });
            }
            _ => self.confirm_and_burn(BurnMode::IsoMode),
        }
    }

    fn confirm_and_burn(self: &Rc<Self>, mode: BurnMode) {
        let Some(dev) = self.selected_device() else { return };
        let ui = self.clone();
        dialogs::confirm_erase(self.parent_window(), &dev.display_label(), move |ok| {
            if ok {
                ui.launch_burn(mode);
            }
        });
    }

    fn launch_burn(self: &Rc<Self>, mode: BurnMode) {
        let Some(dev) = self.selected_device() else { return };
        let (image, is_windows, wue, username) = {
            let st = self.state.borrow();
            (
                st.image.clone(),
                st.report.as_ref().is_some_and(|r| r.image_type == ImageType::Windows),
                st.wue,
                st.wue_username.clone(),
            )
        };
        let opts = BurnOptions {
            device_path: dev.path.clone(),
            image_path: image,
            burn_mode: mode,
            partition_scheme: if self.scheme_row.selected() == 0 {
                PartitionScheme::Mbr
            } else {
                PartitionScheme::Gpt
            },
            target_system: if self.target_row.selected() == 0 {
                TargetSystem::BiosOrUefi
            } else {
                TargetSystem::Uefi
            },
            file_system: FILE_SYSTEMS[(self.fs_row.selected() as usize).min(FILE_SYSTEMS.len() - 1)],
            volume_label: self.label_row.text().to_string(),
            quick_format: self.quick_row.is_active(),
            cluster_size: CLUSTER_SIZES[(self.cluster_row.selected() as usize).min(CLUSTER_SIZES.len() - 1)],
            persistence_size_mb: if self.persistence_row.is_visible() {
                self.persistence_row.value() as u64
            } else {
                0
            },
            bad_blocks_passes: if self.badblocks_row.is_active() {
                self.passes_row.selected() + 1
            } else {
                0
            },
            allow_fixed_drives: self.usb_hdd_row.is_active(),
            wue_options: is_windows.then_some(wue),
            wue_username: username,
            wue_locale: self.locale.clone(),
        };
        self.log(&format!("Writing {} in {:?} mode", dev.path.display(), mode));
        self.spawn_job(WorkerRequest::Burn(opts));
    }

    fn start_dump(self: &Rc<Self>) {
        let Some(dev) = self.selected_device() else {
            self.set_status("Select a target drive first", true);
            return;
        };
        let chooser = gtk::FileDialog::builder()
            .title("Save drive image as")
            .initial_name("drive.img")
            .build();
        let ui = self.clone();
        chooser.save(Some(&self.window), gio::Cancellable::NONE, move |res| {
            if let Some(output) = res.ok().and_then(|f| f.path()) {
                ui.log(&format!("Dumping {} to {}", dev.path.display(), output.display()));
                ui.spawn_job(WorkerRequest::Dump {
                    device: dev.path.clone(),
                    output,
                });
            }
        });
    }

    fn spawn_job(self: &Rc<Self>, request: WorkerRequest) {
        let (tx, rx) = unbounded();
        self.cancel.store(false, Ordering::Relaxed);
        let cancel = self.cancel.clone();
        self.set_busy(true);
        self.progress.set_fraction(0.0);
        self.progress.set_show_text(false);
        self.progress.set_visible(true);
        self.set_status("Waiting for authorization…", false);

        thread::spawn(move || {
            run_job(&request, &cancel, |progress| {
                let _ = tx.send(progress);
            });
        });

        let ui = self.clone();
        glib::timeout_add_local(Duration::from_millis(100), move || ui.poll_job(&rx));
    }

    fn poll_job(&self, rx: &Receiver<BurnProgress>) -> glib::ControlFlow {
        loop {
            match rx.try_recv() {
                Ok(BurnProgress::Status(s)) => self.set_status(&s, false),
                Ok(BurnProgress::Progress { percent, speed_mb_s }) => {
                    self.progress.set_fraction(f64::from(percent / 100.0).clamp(0.0, 1.0));
                    self.progress.set_show_text(true);
                    self.progress
                        .set_text(Some(&format!("{percent:.0}% · {speed_mb_s:.1} MB/s")));
                }
                Ok(BurnProgress::Log(l)) => self.log(&l),
                Ok(BurnProgress::Finished) => {
                    self.log("Completed successfully");
                    self.set_status("Done", false);
                    self.progress.set_fraction(1.0);
                    self.toast("Drive written successfully");
                    return self.job_ended();
                }
                Ok(BurnProgress::Error(e)) => {
                    self.log(&format!("Failed: {e}"));
                    self.set_status(&format!("Failed: {e}"), true);
                    self.toast(&format!("Failed: {e}"));
                    return self.job_ended();
                }
                Err(TryRecvError::Empty) => return glib::ControlFlow::Continue,
                Err(TryRecvError::Disconnected) => return self.job_ended(),
            }
        }
    }

    fn job_ended(&self) -> glib::ControlFlow {
        self.progress.set_visible(false);
        self.set_busy(false);
        self.refresh_devices();
        glib::ControlFlow::Break
    }
}

fn human_size(bytes: u64) -> String {
    const UNITS: [&str; 5] = ["B", "KiB", "MiB", "GiB", "TiB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit < UNITS.len() - 1 {
        value /= 1024.0;
        unit += 1;
    }
    format!("{value:.1} {}", UNITS[unit])
}
