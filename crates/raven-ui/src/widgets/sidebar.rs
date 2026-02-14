use std::path::PathBuf;

use gtk4 as gtk;
use gtk::prelude::*;

use raven_core::commands::AppCommand;
use raven_core::config::Bookmark;
use raven_core::path::RavenPath;

/// Sidebar with bookmarks and mounted volumes.
pub struct Sidebar {
    pub widget: gtk::Box,
}

impl Sidebar {
    pub fn new(
        bookmarks: &[Bookmark],
        command_tx: tokio::sync::mpsc::UnboundedSender<AppCommand>,
        pane_id: u32,
    ) -> Self {
        let widget = gtk::Box::new(gtk::Orientation::Vertical, 0);
        widget.set_width_request(200);
        widget.add_css_class("navigation-sidebar");

        // Bookmarks section
        let bookmarks_label = gtk::Label::new(Some("Places"));
        bookmarks_label.set_halign(gtk::Align::Start);
        bookmarks_label.add_css_class("heading");
        bookmarks_label.set_margin_top(12);
        bookmarks_label.set_margin_start(12);
        bookmarks_label.set_margin_bottom(6);
        widget.append(&bookmarks_label);

        let list_box = gtk::ListBox::new();
        list_box.set_selection_mode(gtk::SelectionMode::Single);
        list_box.add_css_class("navigation-sidebar");

        for bookmark in bookmarks {
            let row = Self::create_bookmark_row(bookmark, &command_tx, pane_id);
            list_box.append(&row);
        }

        widget.append(&list_box);

        // Separator
        let sep = gtk::Separator::new(gtk::Orientation::Horizontal);
        sep.set_margin_top(12);
        sep.set_margin_bottom(12);
        widget.append(&sep);

        // Volumes section
        let volumes_label = gtk::Label::new(Some("Devices"));
        volumes_label.set_halign(gtk::Align::Start);
        volumes_label.add_css_class("heading");
        volumes_label.set_margin_start(12);
        volumes_label.set_margin_bottom(6);
        widget.append(&volumes_label);

        let volume_list = gtk::ListBox::new();
        volume_list.set_selection_mode(gtk::SelectionMode::Single);
        volume_list.add_css_class("navigation-sidebar");

        // Add filesystem root
        let root_row = Self::make_row("Computer", "computer-symbolic", "/", &command_tx, pane_id);
        volume_list.append(&root_row);

        // Try to detect mounted volumes via gio
        let volume_monitor = gio::VolumeMonitor::get();
        for mount in volume_monitor.mounts() {
            let name = mount.name().to_string();
            let root = mount.root();
            if let Some(path) = root.path() {
                let path_str = path.to_string_lossy().to_string();
                let row = Self::make_row(
                    &name,
                    "drive-harddisk-symbolic",
                    &path_str,
                    &command_tx,
                    pane_id,
                );
                volume_list.append(&row);
            }
        }

        widget.append(&volume_list);

        Self { widget }
    }

    fn create_bookmark_row(
        bookmark: &Bookmark,
        command_tx: &tokio::sync::mpsc::UnboundedSender<AppCommand>,
        pane_id: u32,
    ) -> gtk::ListBoxRow {
        let icon_name = bookmark
            .icon
            .as_deref()
            .unwrap_or("folder-symbolic");
        Self::make_row(&bookmark.name, icon_name, &bookmark.path, command_tx, pane_id)
    }

    fn make_row(
        label: &str,
        icon_name: &str,
        path_str: &str,
        command_tx: &tokio::sync::mpsc::UnboundedSender<AppCommand>,
        pane_id: u32,
    ) -> gtk::ListBoxRow {
        let hbox = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        hbox.set_margin_start(8);
        hbox.set_margin_end(8);
        hbox.set_margin_top(4);
        hbox.set_margin_bottom(4);

        let icon = gtk::Image::from_icon_name(icon_name);
        icon.set_pixel_size(16);
        hbox.append(&icon);

        let lbl = gtk::Label::new(Some(label));
        lbl.set_halign(gtk::Align::Start);
        lbl.set_ellipsize(gtk::pango::EllipsizeMode::End);
        hbox.append(&lbl);

        let row = gtk::ListBoxRow::new();
        row.set_child(Some(&hbox));

        // Navigate on click via gesture
        let cmd_tx = command_tx.clone();
        let target_path = path_str.to_string();
        let gesture = gtk::GestureClick::new();
        gesture.connect_released(move |_, _, _, _| {
            let _ = cmd_tx.send(AppCommand::Navigate {
                path: RavenPath::local(PathBuf::from(&target_path)),
                pane_id,
            });
        });
        row.add_controller(gesture);

        row
    }
}
