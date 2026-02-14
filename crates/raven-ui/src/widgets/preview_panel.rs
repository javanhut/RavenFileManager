use gtk4 as gtk;
use gtk::prelude::*;

use raven_core::events::PreviewData;
use raven_core::path::RavenPath;

use crate::widgets::file_list::format_size;

/// Preview panel showing file previews (text, image, directory summary).
pub struct PreviewPanel {
    pub widget: gtk::Box,
    stack: gtk::Stack,
    text_view: gtk::TextView,
    image: gtk::Picture,
    dir_label: gtk::Label,
    unsupported_label: gtk::Label,
    title_label: gtk::Label,
}

impl PreviewPanel {
    pub fn new() -> Self {
        let widget = gtk::Box::new(gtk::Orientation::Vertical, 0);
        widget.set_width_request(300);
        widget.add_css_class("preview-panel");

        // Title
        let title_label = gtk::Label::new(Some("Preview"));
        title_label.add_css_class("heading");
        title_label.set_margin_top(8);
        title_label.set_margin_bottom(8);
        title_label.set_margin_start(8);
        title_label.set_halign(gtk::Align::Start);
        widget.append(&title_label);

        let separator = gtk::Separator::new(gtk::Orientation::Horizontal);
        widget.append(&separator);

        // Stack for different preview types
        let stack = gtk::Stack::new();
        stack.set_vexpand(true);

        // Text preview
        let text_view = gtk::TextView::new();
        text_view.set_editable(false);
        text_view.set_cursor_visible(false);
        text_view.set_wrap_mode(gtk::WrapMode::WordChar);
        text_view.set_monospace(true);
        text_view.set_margin_start(8);
        text_view.set_margin_end(8);
        text_view.set_margin_top(8);
        let text_scroll = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Automatic)
            .vscrollbar_policy(gtk::PolicyType::Automatic)
            .child(&text_view)
            .build();
        stack.add_named(&text_scroll, Some("text"));

        // Image preview
        let image = gtk::Picture::new();
        image.set_can_shrink(true);
        let image_scroll = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Automatic)
            .vscrollbar_policy(gtk::PolicyType::Automatic)
            .child(&image)
            .build();
        stack.add_named(&image_scroll, Some("image"));

        // Directory preview
        let dir_label = gtk::Label::new(None);
        dir_label.set_wrap(true);
        dir_label.set_margin_start(16);
        dir_label.set_margin_end(16);
        dir_label.set_margin_top(16);
        dir_label.set_valign(gtk::Align::Start);
        stack.add_named(&dir_label, Some("directory"));

        // Unsupported
        let unsupported_label = gtk::Label::new(Some("No preview available"));
        unsupported_label.add_css_class("dim-label");
        unsupported_label.set_valign(gtk::Align::Center);
        stack.add_named(&unsupported_label, Some("unsupported"));

        // Empty state
        let empty_label = gtk::Label::new(Some("Select a file to preview"));
        empty_label.add_css_class("dim-label");
        empty_label.set_valign(gtk::Align::Center);
        stack.add_named(&empty_label, Some("empty"));
        stack.set_visible_child_name("empty");

        widget.append(&stack);

        Self {
            widget,
            stack,
            text_view,
            image,
            dir_label,
            unsupported_label,
            title_label,
        }
    }

    /// Update the preview panel with new preview data.
    pub fn set_preview(&self, path: &RavenPath, data: &PreviewData) {
        let file_name = path.file_name().unwrap_or("Unknown");
        self.title_label.set_text(&file_name);

        match data {
            PreviewData::Text { content, language: _ } => {
                let buffer = self.text_view.buffer();
                buffer.set_text(content);
                self.stack.set_visible_child_name("text");
            }
            PreviewData::Image { path, width: _, height: _ } => {
                self.image.set_filename(Some(&path.display().to_string()));
                self.stack.set_visible_child_name("image");
            }
            PreviewData::Directory { item_count, total_size } => {
                self.dir_label.set_markup(&format!(
                    "<b>Directory</b>\n\nItems: {}\nTotal size: {}",
                    item_count,
                    format_size(*total_size)
                ));
                self.stack.set_visible_child_name("directory");
            }
            PreviewData::Unsupported { mime_type } => {
                self.unsupported_label
                    .set_text(&format!("No preview available\n({})", mime_type));
                self.stack.set_visible_child_name("unsupported");
            }
        }
    }

    pub fn clear(&self) {
        self.title_label.set_text("Preview");
        self.stack.set_visible_child_name("empty");
    }
}
