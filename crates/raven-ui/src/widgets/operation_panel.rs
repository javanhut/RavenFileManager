use std::collections::HashMap;

use gtk4 as gtk;
use gtk::prelude::*;

use raven_core::operations::{OperationId, OperationProgress};

struct OperationRow {
    container: gtk::Box,
    label: gtk::Label,
    progress_bar: gtk::ProgressBar,
    file_label: gtk::Label,
}

/// Panel showing operation progress (copy, move, delete operations).
pub struct OperationPanel {
    pub revealer: gtk::Revealer,
    list_box: gtk::Box,
    operations: std::cell::RefCell<HashMap<OperationId, OperationRow>>,
}

impl OperationPanel {
    pub fn new() -> Self {
        let revealer = gtk::Revealer::new();
        revealer.set_transition_type(gtk::RevealerTransitionType::SlideUp);

        let outer = gtk::Box::new(gtk::Orientation::Vertical, 0);
        let separator = gtk::Separator::new(gtk::Orientation::Horizontal);
        outer.append(&separator);

        let header = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        header.set_margin_start(8);
        header.set_margin_end(8);
        header.set_margin_top(4);

        let title = gtk::Label::new(Some("Operations"));
        title.add_css_class("heading");
        title.set_halign(gtk::Align::Start);
        title.set_hexpand(true);
        header.append(&title);

        outer.append(&header);

        let list_box = gtk::Box::new(gtk::Orientation::Vertical, 4);
        list_box.set_margin_start(8);
        list_box.set_margin_end(8);
        list_box.set_margin_top(4);
        list_box.set_margin_bottom(8);

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
            operations: std::cell::RefCell::new(HashMap::new()),
        }
    }

    /// Add a new operation to the panel.
    pub fn add_operation(&self, id: OperationId, description: &str) {
        let container = gtk::Box::new(gtk::Orientation::Vertical, 2);

        let label = gtk::Label::new(Some(description));
        label.set_halign(gtk::Align::Start);
        label.set_ellipsize(gtk::pango::EllipsizeMode::End);
        container.append(&label);

        let progress_bar = gtk::ProgressBar::new();
        progress_bar.set_show_text(true);
        container.append(&progress_bar);

        let file_label = gtk::Label::new(None);
        file_label.set_halign(gtk::Align::Start);
        file_label.add_css_class("dim-label");
        file_label.add_css_class("caption");
        file_label.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
        container.append(&file_label);

        self.list_box.append(&container);

        let row = OperationRow {
            container,
            label,
            progress_bar,
            file_label,
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

    /// Remove an operation (completed or failed).
    pub fn remove_operation(&self, id: OperationId) {
        let mut ops = self.operations.borrow_mut();
        if let Some(row) = ops.remove(&id) {
            self.list_box.remove(&row.container);
        }

        if ops.is_empty() {
            self.revealer.set_reveal_child(false);
        }
    }
}
