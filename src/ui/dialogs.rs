//! Secondary dialogs: Windows customisation, checksums, log and about.

use adw::prelude::*;
use gtk4 as gtk;
use libadwaita as adw;

use std::cell::RefCell;
use std::rc::Rc;

use crate::core::hasher::ImageHashes;
use crate::core::writer::BurnMode;
use crate::core::wue::WueOptions;

const VERSION: &str = env!("CARGO_PKG_VERSION");

fn dialog_shell(title: &str, width: i32, content: &impl IsA<gtk::Widget>) -> adw::Dialog {
    let toolbar = adw::ToolbarView::new();
    toolbar.add_top_bar(&adw::HeaderBar::new());
    toolbar.set_content(Some(content));
    adw::Dialog::builder()
        .title(title)
        .content_width(width)
        .child(&toolbar)
        .build()
}

fn copy_button(text: Rc<RefCell<String>>) -> gtk::Button {
    let button = gtk::Button::from_icon_name("edit-copy-symbolic");
    button.set_valign(gtk::Align::Center);
    button.set_tooltip_text(Some("Copy"));
    button.add_css_class("flat");
    button.connect_clicked(move |b| b.clipboard().set_text(&text.borrow()));
    button
}

// ---------------------------------------------------------------- Windows customisation

/// Asks how the Windows installer should be customised; `done` gets `None` when cancelled.
pub fn show_wue(
    parent: &impl IsA<gtk::Widget>,
    options: WueOptions,
    username: &str,
    done: impl Fn(Option<(WueOptions, String)>) + 'static,
) {
    let group = adw::PreferencesGroup::builder()
        .title("Customize Windows installation")
        .description("Applied through an autounattend.xml on the drive.")
        .build();

    let switch = |title: &str, active: bool| {
        let row = adw::SwitchRow::builder().title(title).active(active).build();
        group.add(&row);
        row
    };
    let tpm = switch(
        "Remove 4 GB RAM, Secure Boot and TPM 2.0 requirements",
        options.bypass_tpm_secureboot_ram,
    );
    let online = switch(
        "Remove online Microsoft account requirement",
        options.bypass_online_account,
    );
    let local = switch("Create a local account", options.create_local_account);
    let user = adw::EntryRow::builder().title("Username").text(username).build();
    local.bind_property("active", &user, "visible").sync_create().build();
    group.add(&user);
    let locale = switch("Use this computer's regional options", options.set_locale);
    let bitlocker = switch("Disable BitLocker automatic encryption", options.disable_bitlocker);
    let privacy = switch(
        "Skip privacy questions and data collection",
        options.disable_data_collection,
    );

    let ok = gtk::Button::with_label("Continue");
    ok.add_css_class("suggested-action");
    ok.add_css_class("pill");
    ok.set_halign(gtk::Align::Center);

    let page = gtk::Box::new(gtk::Orientation::Vertical, 18);
    page.set_margin_top(12);
    page.set_margin_bottom(18);
    page.set_margin_start(18);
    page.set_margin_end(18);
    page.append(&group);
    page.append(&ok);
    let scroll = gtk::ScrolledWindow::builder()
        .child(&page)
        .propagate_natural_height(true)
        .max_content_height(640)
        .build();

    let dialog = dialog_shell("Windows User Experience", 460, &scroll);
    let done = Rc::new(done);
    let accepted = Rc::new(std::cell::Cell::new(false));

    ok.connect_clicked({
        let dialog = dialog.clone();
        let done = done.clone();
        let accepted = accepted.clone();
        move |_| {
            accepted.set(true);
            let opts = WueOptions {
                bypass_tpm_secureboot_ram: tpm.is_active(),
                bypass_online_account: online.is_active(),
                create_local_account: local.is_active(),
                set_locale: locale.is_active(),
                disable_bitlocker: bitlocker.is_active(),
                disable_data_collection: privacy.is_active(),
            };
            done(Some((opts, user.text().to_string())));
            dialog.close();
        }
    });
    dialog.connect_closed(move |_| {
        if !accepted.get() {
            done(None);
        }
    });
    dialog.present(Some(parent));
}

// ---------------------------------------------------------------- ISO / DD choice

pub fn ask_iso_mode(parent: &impl IsA<gtk::Widget>, done: impl FnOnce(Option<BurnMode>) + 'static) {
    let dialog = adw::AlertDialog::builder()
        .heading("ISOHybrid image detected")
        .body(
            "This image can be written by copying its files (ISO mode) or block for block (DD mode).\n\n\
             ISO mode keeps the whole drive usable afterwards and is recommended.",
        )
        .build();
    dialog.add_responses(&[
        ("cancel", "Cancel"),
        ("dd", "Write in DD mode"),
        ("iso", "Write in ISO mode"),
    ]);
    dialog.set_response_appearance("iso", adw::ResponseAppearance::Suggested);
    dialog.set_default_response(Some("iso"));
    dialog.set_close_response("cancel");
    dialog.choose(Some(parent), gtk::gio::Cancellable::NONE, move |response| {
        done(match response.as_str() {
            "iso" => Some(BurnMode::IsoMode),
            "dd" => Some(BurnMode::DdMode),
            _ => None,
        });
    });
}

// ---------------------------------------------------------------- Erase confirmation

pub fn confirm_erase(parent: &impl IsA<gtk::Widget>, device: &str, done: impl FnOnce(bool) + 'static) {
    let dialog = adw::AlertDialog::builder()
        .heading("Erase this drive?")
        .body(format!(
            "All data on <b>{}</b> will be destroyed. This cannot be undone.",
            gtk::glib::markup_escape_text(device)
        ))
        .body_use_markup(true)
        .build();
    dialog.add_responses(&[("cancel", "Cancel"), ("erase", "Erase and Write")]);
    dialog.set_response_appearance("erase", adw::ResponseAppearance::Destructive);
    dialog.set_default_response(Some("cancel"));
    dialog.set_close_response("cancel");
    dialog.choose(Some(parent), gtk::gio::Cancellable::NONE, move |response| {
        done(response == "erase")
    });
}

// ---------------------------------------------------------------- Checksums

struct HashRow {
    row: adw::ActionRow,
    value: Rc<RefCell<String>>,
}

pub struct HashDialog {
    dialog: adw::Dialog,
    progress: gtk::ProgressBar,
    rows: Vec<HashRow>,
    verify: adw::EntryRow,
    verdict: gtk::Label,
}

impl HashDialog {
    pub fn present(parent: &impl IsA<gtk::Widget>, file_name: &str) -> Rc<Self> {
        let progress = gtk::ProgressBar::new();
        let group = adw::PreferencesGroup::builder().title(file_name).build();
        let mut rows = Vec::new();
        for name in ["MD5", "SHA-1", "SHA-256", "SHA-512"] {
            let value = Rc::new(RefCell::new(String::new()));
            let row = adw::ActionRow::builder()
                .title(name)
                .subtitle("Calculating…")
                .subtitle_selectable(true)
                .build();
            row.add_css_class("property");
            row.add_suffix(&copy_button(value.clone()));
            group.add(&row);
            rows.push(HashRow { row, value });
        }

        let verify_group = adw::PreferencesGroup::builder().title("Verify").build();
        let verify = adw::EntryRow::builder().title("Paste the published checksum").build();
        let verdict = gtk::Label::new(None);
        verdict.add_css_class("heading");
        verify.add_suffix(&verdict);
        verify_group.add(&verify);

        let page = gtk::Box::new(gtk::Orientation::Vertical, 18);
        page.set_margin_top(12);
        page.set_margin_bottom(18);
        page.set_margin_start(18);
        page.set_margin_end(18);
        page.append(&progress);
        page.append(&group);
        page.append(&verify_group);
        let scroll = gtk::ScrolledWindow::builder()
            .child(&page)
            .propagate_natural_height(true)
            .build();

        let this = Rc::new(Self {
            dialog: dialog_shell("Checksums", 520, &scroll),
            progress,
            rows,
            verify,
            verdict,
        });
        this.verify.connect_changed({
            let this = this.clone();
            move |_| this.check_match()
        });
        this.dialog.present(Some(parent));
        this
    }

    pub fn set_progress(&self, fraction: f64) {
        self.progress.set_fraction(fraction);
    }

    pub fn set_hashes(&self, h: &ImageHashes) {
        self.progress.set_visible(false);
        for (row, text) in self.rows.iter().zip([&h.md5, &h.sha1, &h.sha256, &h.sha512]) {
            *row.value.borrow_mut() = text.clone();
            row.row.set_subtitle(text);
        }
        self.check_match();
    }

    pub fn fail(&self) {
        self.dialog.close();
    }

    fn check_match(&self) {
        let wanted = self.verify.text().trim().to_lowercase();
        self.verdict.remove_css_class("success");
        self.verdict.remove_css_class("error");
        if wanted.is_empty() {
            self.verdict.set_text("");
        } else if self.rows.iter().any(|r| *r.value.borrow() == wanted) {
            self.verdict.set_text("Match");
            self.verdict.add_css_class("success");
        } else {
            self.verdict.set_text("No match");
            self.verdict.add_css_class("error");
        }
    }
}

// ---------------------------------------------------------------- Log

pub fn show_log(parent: &impl IsA<gtk::Widget>, buffer: &gtk::TextBuffer) {
    let view = gtk::TextView::with_buffer(buffer);
    view.set_editable(false);
    view.set_cursor_visible(false);
    view.set_monospace(true);
    view.set_wrap_mode(gtk::WrapMode::WordChar);
    view.set_left_margin(12);
    view.set_right_margin(12);
    view.set_top_margin(8);
    view.set_bottom_margin(8);
    let scroll = gtk::ScrolledWindow::builder().child(&view).vexpand(true).build();

    // Follow new output while the dialog is open.
    let end = buffer.create_mark(None, &buffer.end_iter(), false);
    let handler = buffer.connect_changed({
        let view = view.clone();
        let end = end.clone();
        move |b| {
            b.move_mark(&end, &b.end_iter());
            view.scroll_mark_onscreen(&end);
        }
    });

    let clear = gtk::Button::with_label("Clear");
    clear.connect_clicked({
        let buffer = buffer.clone();
        move |_| buffer.set_text("")
    });
    let save = gtk::Button::with_label("Save…");
    save.connect_clicked({
        let buffer = buffer.clone();
        let parent = parent.clone().upcast::<gtk::Widget>();
        move |_| {
            let chooser = gtk::FileDialog::builder()
                .title("Save log")
                .initial_name("rufux.log")
                .build();
            let buffer = buffer.clone();
            chooser.save(
                parent.root().and_downcast_ref::<gtk::Window>(),
                gtk::gio::Cancellable::NONE,
                move |res| {
                    if let Some(path) = res.ok().and_then(|f| f.path()) {
                        let text = buffer.text(&buffer.start_iter(), &buffer.end_iter(), false);
                        let _ = std::fs::write(path, text.as_str());
                    }
                },
            );
        }
    });

    let toolbar = adw::ToolbarView::new();
    let header = adw::HeaderBar::new();
    header.pack_start(&clear);
    header.pack_start(&save);
    toolbar.add_top_bar(&header);
    toolbar.set_content(Some(&scroll));
    let dialog = adw::Dialog::builder()
        .title("Log")
        .content_width(640)
        .content_height(460)
        .child(&toolbar)
        .build();
    let handler = RefCell::new(Some(handler));
    dialog.connect_closed({
        let buffer = buffer.clone();
        move |_| {
            if let Some(id) = handler.take() {
                buffer.disconnect(id);
            }
            buffer.delete_mark(&end);
        }
    });
    dialog.present(Some(parent));
}

// ---------------------------------------------------------------- About

pub fn show_about(parent: &impl IsA<gtk::Widget>) {
    let about = adw::AboutDialog::builder()
        .application_name("Rufux")
        .application_icon("rufux")
        .developer_name("Rohit Anand")
        .version(VERSION)
        .comments("Create bootable USB drives from ISO and disk images.\n\nA Linux take on Rufus by Pete Batard.")
        .website("https://github.com/R0h1tAnand/Rufux")
        .license_type(gtk::License::Gpl30)
        .copyright("© 2026 Rohit Anand / Pete Batard")
        .build();
    about.present(Some(parent));
}
