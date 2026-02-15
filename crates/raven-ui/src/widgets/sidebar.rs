use std::path::PathBuf;

use gtk4 as gtk;
use gtk::prelude::*;

use raven_core::commands::AppCommand;
use raven_core::config::Bookmark;
use raven_core::path::RavenPath;

use crate::state::AppState;

/// Sidebar with bookmarks, mounted volumes, and tags.
pub struct Sidebar {
    pub widget: gtk::Box,
    bookmarks_list: gtk::ListBox,
    tags_list: gtk::ListBox,
    command_tx: tokio::sync::mpsc::UnboundedSender<AppCommand>,
    pane_id: u32,
    state: AppState,
}

impl Sidebar {
    pub fn new(
        state: AppState,
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

        let bookmarks_list = gtk::ListBox::new();
        bookmarks_list.set_selection_mode(gtk::SelectionMode::Single);
        bookmarks_list.add_css_class("navigation-sidebar");

        let bookmarks = {
            let s = state.borrow();
            s.config.bookmarks.clone()
        };

        for bookmark in &bookmarks {
            let row = Self::create_bookmark_row(bookmark, &command_tx, pane_id, &state);
            bookmarks_list.append(&row);
        }

        widget.append(&bookmarks_list);

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
        let root_row = Self::make_nav_row("Computer", "computer-symbolic", "/", &command_tx, pane_id);
        volume_list.append(&root_row);

        // Try to detect mounted volumes via gio
        let volume_monitor = gio::VolumeMonitor::get();
        for mount in volume_monitor.mounts() {
            let name = mount.name().to_string();
            let root = mount.root();
            if let Some(path) = root.path() {
                let path_str = path.to_string_lossy().to_string();
                let row = Self::make_nav_row(
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

        // Separator before tags
        let sep2 = gtk::Separator::new(gtk::Orientation::Horizontal);
        sep2.set_margin_top(12);
        sep2.set_margin_bottom(12);
        widget.append(&sep2);

        // Tags section
        let tags_label = gtk::Label::new(Some("Tags"));
        tags_label.set_halign(gtk::Align::Start);
        tags_label.add_css_class("heading");
        tags_label.set_margin_start(12);
        tags_label.set_margin_bottom(6);
        widget.append(&tags_label);

        let tags_list = gtk::ListBox::new();
        tags_list.set_selection_mode(gtk::SelectionMode::Single);
        tags_list.add_css_class("navigation-sidebar");
        widget.append(&tags_list);

        Self {
            widget,
            bookmarks_list,
            tags_list,
            command_tx,
            pane_id,
            state,
        }
    }

    /// Update the tags section with new counts.
    pub fn update_tag_counts(&self, counts: &[(String, usize)]) {
        // Clear existing tag rows
        while let Some(child) = self.tags_list.first_child() {
            self.tags_list.remove(&child);
        }

        for (tag_name, count) in counts {
            if *count == 0 {
                continue;
            }
            let hbox = gtk::Box::new(gtk::Orientation::Horizontal, 8);
            hbox.set_margin_start(8);
            hbox.set_margin_end(8);
            hbox.set_margin_top(4);
            hbox.set_margin_bottom(4);

            let icon = gtk::Image::from_icon_name("tag-symbolic");
            icon.set_pixel_size(16);
            hbox.append(&icon);

            let lbl = gtk::Label::new(Some(tag_name));
            lbl.set_halign(gtk::Align::Start);
            lbl.set_hexpand(true);
            lbl.set_ellipsize(gtk::pango::EllipsizeMode::End);
            hbox.append(&lbl);

            let count_lbl = gtk::Label::new(Some(&count.to_string()));
            count_lbl.add_css_class("dim-label");
            hbox.append(&count_lbl);

            let row = gtk::ListBoxRow::new();
            row.set_child(Some(&hbox));

            // Click to filter by tag
            let cmd_tx = self.command_tx.clone();
            let tag = tag_name.clone();
            let pane_id = self.pane_id;
            let gesture = gtk::GestureClick::new();
            gesture.set_button(1);
            gesture.connect_released(move |_, _, _, _| {
                let _ = cmd_tx.send(AppCommand::FilterByTag {
                    tag: tag.clone(),
                    pane_id,
                });
            });
            row.add_controller(gesture);

            self.tags_list.append(&row);
        }
    }

    /// Add a new bookmark to the sidebar and persist to config.
    pub fn add_bookmark(&self, bookmark: &Bookmark) {
        // Check for duplicates
        {
            let s = self.state.borrow();
            if s.config.bookmarks.iter().any(|b| b.path == bookmark.path) {
                return;
            }
        }

        // Add to config and save
        {
            let mut s = self.state.borrow_mut();
            s.config.bookmarks.push(bookmark.clone());
            if let Err(e) = s.config.save() {
                tracing::warn!("Failed to save config after adding bookmark: {}", e);
            }
        }

        // Add row to UI
        let row = Self::create_bookmark_row(bookmark, &self.command_tx, self.pane_id, &self.state);
        self.bookmarks_list.append(&row);
    }

    fn create_bookmark_row(
        bookmark: &Bookmark,
        command_tx: &tokio::sync::mpsc::UnboundedSender<AppCommand>,
        pane_id: u32,
        state: &AppState,
    ) -> gtk::ListBoxRow {
        let icon_name = bookmark
            .icon
            .as_deref()
            .unwrap_or("folder-symbolic");

        let hbox = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        hbox.set_margin_start(8);
        hbox.set_margin_end(8);
        hbox.set_margin_top(4);
        hbox.set_margin_bottom(4);

        let icon = gtk::Image::from_icon_name(icon_name);
        icon.set_pixel_size(16);
        hbox.append(&icon);

        let lbl = gtk::Label::new(Some(&bookmark.name));
        lbl.set_halign(gtk::Align::Start);
        lbl.set_ellipsize(gtk::pango::EllipsizeMode::End);
        hbox.append(&lbl);

        let row = gtk::ListBoxRow::new();
        row.set_child(Some(&hbox));
        row.set_widget_name(&bookmark.path);

        // Navigate on left-click
        let cmd_tx = command_tx.clone();
        let target_path = bookmark.path.clone();
        let gesture = gtk::GestureClick::new();
        gesture.set_button(1);
        gesture.connect_released(move |_, _, _, _| {
            let _ = cmd_tx.send(AppCommand::Navigate {
                path: RavenPath::local(PathBuf::from(&target_path)),
                pane_id,
            });
        });
        row.add_controller(gesture);

        // Right-click: "Remove from Sidebar" popover
        let remove_popover = Self::build_remove_popover(&row, state, &bookmark.path);
        remove_popover.set_parent(&row);

        let right_click = gtk::GestureClick::new();
        right_click.set_button(3);
        let popover = remove_popover;
        right_click.connect_released(move |_, _, x, y| {
            let rect = gtk::gdk::Rectangle::new(x as i32, y as i32, 1, 1);
            popover.set_pointing_to(Some(&rect));
            popover.popup();
        });
        row.add_controller(right_click);

        // Drop target: accept files dropped onto this bookmark
        let drop_target = gtk::DropTarget::new(
            gtk::glib::types::Type::STRING,
            gtk::gdk::DragAction::COPY | gtk::gdk::DragAction::MOVE,
        );
        let drop_dest_path = bookmark.path.clone();
        let cmd_tx_drop = command_tx.clone();
        let row_for_enter = row.clone();
        let row_for_leave = row.clone();

        drop_target.connect_enter(move |_target, _x, _y| {
            row_for_enter.add_css_class("drop-highlight");
            gtk::gdk::DragAction::MOVE
        });
        drop_target.connect_leave(move |_target| {
            row_for_leave.remove_css_class("drop-highlight");
        });
        drop_target.connect_drop(move |_target, value, _x, _y| {
            if let Ok(uri_list) = value.get::<String>() {
                let sources: Vec<RavenPath> = uri_list
                    .lines()
                    .filter(|line| !line.is_empty() && !line.starts_with('#'))
                    .filter_map(|line| {
                        let line = line.trim().trim_end_matches('\r');
                        line.strip_prefix("file://")
                            .map(|p| RavenPath::local(PathBuf::from(p)))
                    })
                    .collect();

                if sources.is_empty() {
                    return false;
                }

                let destination = RavenPath::local(PathBuf::from(&drop_dest_path));
                let _ = cmd_tx_drop.send(AppCommand::MoveFiles {
                    sources,
                    destination,
                });
                return true;
            }
            false
        });
        row.add_controller(drop_target);

        row
    }

    fn build_remove_popover(
        row: &gtk::ListBoxRow,
        state: &AppState,
        bookmark_path: &str,
    ) -> gtk::PopoverMenu {
        let menu = gio::Menu::new();
        menu.append(Some("Remove from Sidebar"), Some("sidebar.remove"));

        let popover = gtk::PopoverMenu::from_model(Some(&menu));
        popover.set_has_arrow(false);

        let action_group = gio::SimpleActionGroup::new();
        let action = gio::SimpleAction::new("remove", None);

        let state_for_remove = state.clone();
        let path_for_remove = bookmark_path.to_string();
        let row_ref = row.clone();
        action.connect_activate(move |_, _| {
            // Remove from config
            {
                let mut s = state_for_remove.borrow_mut();
                s.config.bookmarks.retain(|b| b.path != path_for_remove);
                if let Err(e) = s.config.save() {
                    tracing::warn!("Failed to save config after removing bookmark: {}", e);
                }
            }
            // Remove row from ListBox
            if let Some(parent) = row_ref.parent().and_then(|p| p.downcast::<gtk::ListBox>().ok()) {
                parent.remove(&row_ref);
            }
        });
        action_group.add_action(&action);
        row.insert_action_group("sidebar", Some(&action_group));

        popover
    }

    /// Simple navigation row (no remove option) for Devices section.
    fn make_nav_row(
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
