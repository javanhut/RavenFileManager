use std::collections::HashMap;

use gtk4 as gtk;
use gtk::prelude::*;

use raven_core::commands::AppCommand;
use raven_core::operations::{OperationId, OperationProgress};

struct OperationRow {
    container: gtk::Box,
    label: gtk::Label,
    progress_bar: gtk::ProgressBar,
    file_label: gtk::Label,
    pause_btn: gtk::ToggleButton,
    cancel_btn: gtk::Button,
}

/// Panel showing operation progress (copy, move, delete operations), with
/// pause and cancel controls for each one.
pub struct OperationPanel {
    pub revealer: gtk::Revealer,
    list_box: gtk::Box,
    command_tx: tokio::sync::mpsc::UnboundedSender<AppCommand>,
    operations: std::cell::RefCell<HashMap<OperationId, OperationRow>>,
}

impl OperationPanel {
    pub fn new(command_tx: tokio::sync::mpsc::UnboundedSender<AppCommand>) -> Self {
        let revealer = gtk::Revealer::new();
        revealer.set_transition_type(gtk::RevealerTransitionType::SlideUp);

        let outer = gtk::Box::new(gtk::Orientation::Vertical, 6);
        outer.add_css_class("operation-panel");

        let header = gtk::Box::new(gtk::Orientation::Horizontal, 8);

        let title = gtk::Label::new(Some("OPERATIONS"));
        title.add_css_class("eyebrow");
        title.set_halign(gtk::Align::Start);
        title.set_hexpand(true);
        header.append(&title);

        outer.append(&header);

        // Each operation is a card of its own.
        let list_box = gtk::Box::new(gtk::Orientation::Vertical, 6);

        let scroll = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vscrollbar_policy(gtk::PolicyType::Automatic)
            .max_content_height(150)
            .child(&list_box)
            .build();
        outer.append(&scroll);

        revealer.set_child(Some(&outer));

        Self {
            revealer,
            list_box,
            command_tx,
            operations: std::cell::RefCell::new(HashMap::new()),
        }
    }

    /// Add a new operation to the panel.
    pub fn add_operation(&self, id: OperationId, description: &str) {
        let container = gtk::Box::new(gtk::Orientation::Vertical, 4);
        container.add_css_class("operation-row");

        let title_row = gtk::Box::new(gtk::Orientation::Horizontal, 4);

        let label = gtk::Label::new(Some(description));
        label.add_css_class("operation-title");
        label.set_halign(gtk::Align::Start);
        label.set_hexpand(true);
        label.set_ellipsize(gtk::pango::EllipsizeMode::End);
        title_row.append(&label);

        let pause_btn = gtk::ToggleButton::new();
        pause_btn.set_icon_name("media-playback-pause-symbolic");
        pause_btn.set_tooltip_text(Some("Pause"));
        pause_btn.add_css_class("flat");
        pause_btn.set_valign(gtk::Align::Center);
        {
            let cmd_tx = self.command_tx.clone();
            pause_btn.connect_toggled(move |btn| {
                if btn.is_active() {
                    btn.set_icon_name("media-playback-start-symbolic");
                    btn.set_tooltip_text(Some("Resume"));
                    let _ = cmd_tx.send(AppCommand::PauseOperation { id });
                } else {
                    btn.set_icon_name("media-playback-pause-symbolic");
                    btn.set_tooltip_text(Some("Pause"));
                    let _ = cmd_tx.send(AppCommand::ResumeOperation { id });
                }
            });
        }
        title_row.append(&pause_btn);

        let cancel_btn = gtk::Button::from_icon_name("process-stop-symbolic");
        cancel_btn.set_tooltip_text(Some("Cancel"));
        cancel_btn.add_css_class("flat");
        cancel_btn.set_valign(gtk::Align::Center);
        {
            let cmd_tx = self.command_tx.clone();
            let label = label.clone();
            let pause_btn = pause_btn.clone();
            cancel_btn.connect_clicked(move |btn| {
                // A paused operation cannot notice the cancellation; let it run
                // into the check.
                if pause_btn.is_active() {
                    pause_btn.set_active(false);
                }
                let _ = cmd_tx.send(AppCommand::CancelOperation { id });
                label.set_text("Cancelling...");
                btn.set_sensitive(false);
                pause_btn.set_sensitive(false);
            });
        }
        title_row.append(&cancel_btn);

        container.append(&title_row);

        let progress_bar = gtk::ProgressBar::new();
        progress_bar.set_show_text(true);
        container.append(&progress_bar);

        let file_label = gtk::Label::new(None);
        file_label.set_halign(gtk::Align::Start);
        file_label.add_css_class("caption");
        file_label.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
        container.append(&file_label);

        self.list_box.append(&container);

        let row = OperationRow {
            container,
            label,
            progress_bar,
            file_label,
            pause_btn,
            cancel_btn,
        };
        self.operations.borrow_mut().insert(id, row);

        self.revealer.set_reveal_child(true);
    }

    /// Update progress for an operation.
    pub fn update_progress(&self, progress: &OperationProgress) {
        let ops = self.operations.borrow();
        if let Some(row) = ops.get(&progress.id) {
            row.progress_bar.set_fraction(progress.fraction());
            let text = format!(
                "{}/{} files",
                progress.files_done, progress.files_total
            );
            row.progress_bar.set_text(Some(&text));
            if let Some(ref file) = progress.current_file {
                row.file_label.set_text(file);
            }
        }
    }

    /// Show that an operation is waiting on a conflict answer. Pausing or
    /// cancelling still work; the executor notices a cancel while it waits.
    pub fn set_waiting_on_conflict(&self, id: OperationId, file_name: &str) {
        let ops = self.operations.borrow();
        if let Some(row) = ops.get(&id) {
            row.file_label
                .set_text(&format!("Waiting: \"{}\" already exists", file_name));
        }
    }

    /// Remove an operation (completed, failed or cancelled).
    pub fn remove_operation(&self, id: OperationId) {
        let mut ops = self.operations.borrow_mut();
        if let Some(row) = ops.remove(&id) {
            // The row is gone; a stale toggle must not fire on teardown.
            row.pause_btn.set_sensitive(false);
            row.cancel_btn.set_sensitive(false);
            self.list_box.remove(&row.container);
        }

        if ops.is_empty() {
            self.revealer.set_reveal_child(false);
        }
    }
}
