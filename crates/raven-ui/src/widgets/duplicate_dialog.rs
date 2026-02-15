use std::cell::RefCell;
use std::collections::HashSet;
use std::rc::Rc;

use gtk4 as gtk;
use gtk::prelude::*;
use libadwaita as adw;
use libadwaita::prelude::*;

use raven_core::ai_types::{DuplicateGroup, DuplicateScanProgress, ScanPhase};
use raven_core::commands::AppCommand;
use raven_core::path::RavenPath;

/// Dialog for displaying duplicate file scan results.
pub struct DuplicateDialog {
    pub window: adw::Window,
    progress_bar: gtk::ProgressBar,
    progress_label: gtk::Label,
    progress_box: gtk::Box,
    results_scroll: gtk::ScrolledWindow,
    results_box: gtk::Box,
    action_bar: gtk::Box,
    selected_paths: Rc<RefCell<HashSet<String>>>,
    command_tx: tokio::sync::mpsc::UnboundedSender<AppCommand>,
}

impl DuplicateDialog {
    pub fn new(
        parent: &adw::ApplicationWindow,
        command_tx: tokio::sync::mpsc::UnboundedSender<AppCommand>,
    ) -> Self {
        let window = adw::Window::builder()
            .title("Find Duplicates")
            .default_width(700)
            .default_height(500)
            .modal(true)
            .transient_for(parent)
            .build();

        let toolbar_view = adw::ToolbarView::new();
        let header = adw::HeaderBar::new();

        // Cancel scan button in header
        let cancel_btn = gtk::Button::with_label("Cancel Scan");
        cancel_btn.add_css_class("destructive-action");
        {
            let cmd_tx = command_tx.clone();
            cancel_btn.connect_clicked(move |_| {
                let _ = cmd_tx.send(AppCommand::CancelDuplicateScan);
            });
        }
        header.pack_end(&cancel_btn);

        toolbar_view.add_top_bar(&header);

        let content = gtk::Box::new(gtk::Orientation::Vertical, 0);

        // Progress section
        let progress_box = gtk::Box::new(gtk::Orientation::Vertical, 12);
        progress_box.set_margin_start(24);
        progress_box.set_margin_end(24);
        progress_box.set_margin_top(24);
        progress_box.set_margin_bottom(24);
        progress_box.set_valign(gtk::Align::Center);
        progress_box.set_vexpand(true);

        let progress_label = gtk::Label::new(Some("Scanning for duplicate files..."));
        progress_label.add_css_class("title-3");
        progress_box.append(&progress_label);

        let progress_bar = gtk::ProgressBar::new();
        progress_bar.set_show_text(true);
        progress_box.append(&progress_bar);

        content.append(&progress_box);

        // Results section (initially hidden)
        let results_scroll = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vscrollbar_policy(gtk::PolicyType::Automatic)
            .vexpand(true)
            .visible(false)
            .build();

        let results_box = gtk::Box::new(gtk::Orientation::Vertical, 12);
        results_box.set_margin_start(16);
        results_box.set_margin_end(16);
        results_box.set_margin_top(12);
        results_box.set_margin_bottom(12);
        results_scroll.set_child(Some(&results_box));
        content.append(&results_scroll);

        // Action bar (initially hidden)
        let action_bar = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        action_bar.set_margin_start(16);
        action_bar.set_margin_end(16);
        action_bar.set_margin_top(8);
        action_bar.set_margin_bottom(12);
        action_bar.set_halign(gtk::Align::End);
        action_bar.set_visible(false);

        let close_btn = gtk::Button::with_label("Close");
        {
            let window = window.clone();
            close_btn.connect_clicked(move |_| {
                window.close();
            });
        }
        action_bar.append(&close_btn);

        let selected_paths: Rc<RefCell<HashSet<String>>> = Rc::new(RefCell::new(HashSet::new()));

        let trash_btn = gtk::Button::with_label("Move to Trash");
        trash_btn.add_css_class("destructive-action");
        {
            let selected = selected_paths.clone();
            let cmd_tx = command_tx.clone();
            let window = window.clone();
            trash_btn.connect_clicked(move |_| {
                let paths: Vec<RavenPath> = selected
                    .borrow()
                    .iter()
                    .map(|p| RavenPath::Local(std::path::PathBuf::from(p)))
                    .collect();
                if !paths.is_empty() {
                    let _ = cmd_tx.send(AppCommand::TrashFiles { paths });
                }
                window.close();
            });
        }
        action_bar.append(&trash_btn);

        content.append(&action_bar);

        toolbar_view.set_content(Some(&content));
        window.set_content(Some(&toolbar_view));

        Self {
            window,
            progress_bar,
            progress_label,
            progress_box,
            results_scroll,
            results_box,
            action_bar,
            selected_paths,
            command_tx,
        }
    }

    pub fn present(&self) {
        self.window.present();
    }

    /// Update the progress display during scanning.
    pub fn update_progress(&self, progress: &DuplicateScanProgress) {
        match progress.phase {
            ScanPhase::Walking => {
                self.progress_label
                    .set_text(&format!("Walking... {} files found", progress.files_scanned));
                self.progress_bar.pulse();
            }
            ScanPhase::Hashing => {
                let fraction = if progress.total_files > 0 {
                    progress.files_scanned as f64 / progress.total_files as f64
                } else {
                    0.0
                };
                self.progress_bar.set_fraction(fraction);
                self.progress_label.set_text(&format!(
                    "Hashing... {}/{} files",
                    progress.files_scanned, progress.total_files
                ));
            }
            ScanPhase::Complete => {
                self.progress_bar.set_fraction(1.0);
                self.progress_label.set_text("Scan complete");
            }
        }
    }

    /// Display the scan results.
    pub fn set_results(&self, groups: &[DuplicateGroup]) {
        self.progress_box.set_visible(false);
        self.results_scroll.set_visible(true);
        self.action_bar.set_visible(true);

        // Clear previous results
        while let Some(child) = self.results_box.first_child() {
            self.results_box.remove(&child);
        }

        if groups.is_empty() {
            let label = gtk::Label::new(Some("No duplicate files found."));
            label.add_css_class("title-3");
            label.set_margin_top(48);
            label.set_margin_bottom(48);
            self.results_box.append(&label);
            return;
        }

        // Summary
        let total_dupes: usize = groups.iter().map(|g| g.entries.len() - 1).sum();
        let total_wasted: u64 = groups.iter().map(|g| g.size * (g.entries.len() as u64 - 1)).sum();
        let summary = gtk::Label::new(Some(&format!(
            "Found {} duplicate groups ({} extra files, {} wasted)",
            groups.len(),
            total_dupes,
            format_size(total_wasted)
        )));
        summary.add_css_class("title-4");
        summary.set_margin_bottom(8);
        self.results_box.append(&summary);

        for (group_idx, group) in groups.iter().enumerate() {
            let frame = gtk::Frame::new(None);
            frame.set_margin_top(4);
            frame.set_margin_bottom(4);

            let vbox = gtk::Box::new(gtk::Orientation::Vertical, 4);
            vbox.set_margin_start(12);
            vbox.set_margin_end(12);
            vbox.set_margin_top(8);
            vbox.set_margin_bottom(8);

            // Group header
            let header = gtk::Label::new(Some(&format!(
                "Group {} — {} files, {} each",
                group_idx + 1,
                group.entries.len(),
                format_size(group.size)
            )));
            header.set_halign(gtk::Align::Start);
            header.add_css_class("heading");
            vbox.append(&header);

            for (entry_idx, entry) in group.entries.iter().enumerate() {
                let hbox = gtk::Box::new(gtk::Orientation::Horizontal, 8);
                hbox.set_margin_top(2);

                let check = gtk::CheckButton::new();
                // Pre-select all except the first (keep first)
                if entry_idx > 0 {
                    check.set_active(true);
                    let path_str = entry.path.to_string();
                    self.selected_paths.borrow_mut().insert(path_str);
                }

                let selected = self.selected_paths.clone();
                let path_str = entry.path.to_string();
                check.connect_toggled(move |btn| {
                    if btn.is_active() {
                        selected.borrow_mut().insert(path_str.clone());
                    } else {
                        selected.borrow_mut().remove(&path_str);
                    }
                });

                hbox.append(&check);

                let path_label = gtk::Label::new(Some(&entry.path.to_string()));
                path_label.set_halign(gtk::Align::Start);
                path_label.set_ellipsize(gtk::pango::EllipsizeMode::Start);
                path_label.set_hexpand(true);
                if entry_idx == 0 {
                    path_label.add_css_class("dim-label");
                    let keep_label = gtk::Label::new(Some("(keep)"));
                    keep_label.add_css_class("dim-label");
                    hbox.append(&path_label);
                    hbox.append(&keep_label);
                } else {
                    hbox.append(&path_label);
                }

                vbox.append(&hbox);
            }

            frame.set_child(Some(&vbox));
            self.results_box.append(&frame);
        }
    }

    pub fn set_error(&self, error: &str) {
        self.progress_box.set_visible(false);
        self.results_scroll.set_visible(true);

        while let Some(child) = self.results_box.first_child() {
            self.results_box.remove(&child);
        }

        let label = gtk::Label::new(Some(&format!("Scan error: {}", error)));
        label.add_css_class("error");
        label.set_margin_top(48);
        self.results_box.append(&label);
    }
}

fn format_size(bytes: u64) -> String {
    const KIB: u64 = 1024;
    const MIB: u64 = 1024 * KIB;
    const GIB: u64 = 1024 * MIB;

    if bytes >= GIB {
        format!("{:.1} GiB", bytes as f64 / GIB as f64)
    } else if bytes >= MIB {
        format!("{:.1} MiB", bytes as f64 / MIB as f64)
    } else if bytes >= KIB {
        format!("{:.1} KiB", bytes as f64 / KIB as f64)
    } else {
        format!("{} B", bytes)
    }
}
