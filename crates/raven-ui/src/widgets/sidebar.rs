use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

use gtk4 as gtk;
use gtk::prelude::*;

use raven_core::commands::AppCommand;
use raven_core::config::Bookmark;
use raven_core::path::RavenPath;

use crate::state::AppState;
use crate::widgets::file_list::format_size;

/// Sidebar with bookmarks, mounted volumes, and tags.
pub struct Sidebar {
    pub widget: gtk::Box,
    bookmarks_list: gtk::ListBox,
    tags_list: gtk::ListBox,
    volume_list: gtk::ListBox,
    volume_monitor: gio::VolumeMonitor,
    command_tx: tokio::sync::mpsc::UnboundedSender<AppCommand>,
    pane_id: u32,
    state: AppState,
    /// Tag currently filtering the pane, so a second click on the same row clears it.
    active_tag: Rc<RefCell<Option<String>>>,
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

        // --- Drop target on bookmarks_list: drag folders to pin as bookmarks ---
        {
            let pin_drop_target = gtk::DropTarget::new(
                gtk::glib::types::Type::STRING,
                gtk::gdk::DragAction::COPY | gtk::gdk::DragAction::MOVE,
            );

            let bl_for_enter = bookmarks_list.clone();
            let bl_for_leave = bookmarks_list.clone();
            pin_drop_target.connect_enter(move |_target, _x, _y| {
                bl_for_enter.add_css_class("drop-highlight");
                gtk::gdk::DragAction::COPY
            });
            pin_drop_target.connect_leave(move |_target| {
                bl_for_leave.remove_css_class("drop-highlight");
            });

            let state_for_pin = state.clone();
            let bl_for_drop = bookmarks_list.clone();
            let cmd_for_pin = command_tx.clone();
            pin_drop_target.connect_drop(move |_target, value, _x, _y| {
                if let Ok(uri_list) = value.get::<String>() {
                    let paths: Vec<PathBuf> = uri_list
                        .lines()
                        .filter(|line| !line.is_empty() && !line.starts_with('#'))
                        .filter_map(|line| {
                            let line = line.trim().trim_end_matches('\r');
                            line.strip_prefix("file://").map(PathBuf::from)
                        })
                        .collect();

                    let mut pinned_any = false;
                    for p in paths {
                        if p.is_dir() {
                            let name = p
                                .file_name()
                                .map(|n| n.to_string_lossy().to_string())
                                .unwrap_or_else(|| p.to_string_lossy().to_string());
                            let bookmark = Bookmark {
                                name,
                                path: p.to_string_lossy().to_string(),
                                icon: Some("folder-symbolic".to_string()),
                            };
                            let is_dup = {
                                let s = state_for_pin.borrow();
                                s.config.bookmarks.iter().any(|b| b.path == bookmark.path)
                            };
                            if !is_dup {
                                {
                                    let mut s = state_for_pin.borrow_mut();
                                    s.config.bookmarks.push(bookmark.clone());
                                    let _ = s.config.save();
                                }
                                let row = Self::create_bookmark_row(
                                    &bookmark,
                                    &cmd_for_pin,
                                    pane_id,
                                    &state_for_pin,
                                );
                                bl_for_drop.append(&row);
                                pinned_any = true;
                            }
                        }
                    }
                    return pinned_any;
                }
                false
            });
            bookmarks_list.add_controller(pin_drop_target);
        }

        widget.append(&bookmarks_list);

        // Permanent Trash entry (always visible, not user-removable).
        // SelectionMode::None prevents the row from staying highlighted when
        // the user navigates to another folder in a different ListBox.
        let trash_list = gtk::ListBox::new();
        trash_list.set_selection_mode(gtk::SelectionMode::None);
        trash_list.add_css_class("navigation-sidebar");

        let trash_hbox = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        trash_hbox.set_margin_start(8);
        trash_hbox.set_margin_end(8);
        trash_hbox.set_margin_top(4);
        trash_hbox.set_margin_bottom(4);
        let trash_icon = gtk::Image::from_icon_name("user-trash-symbolic");
        trash_icon.set_pixel_size(16);
        trash_hbox.append(&trash_icon);
        let trash_lbl = gtk::Label::new(Some("Trash"));
        trash_lbl.set_halign(gtk::Align::Start);
        trash_hbox.append(&trash_lbl);
        let trash_row = gtk::ListBoxRow::new();
        trash_row.set_child(Some(&trash_hbox));

        let trash_path = PathBuf::from(
            std::env::var("HOME").unwrap_or_else(|_| "/root".to_string()),
        )
        .join(".local/share/Trash/files");
        let cmd_tx_trash = command_tx.clone();
        let trash_gesture = gtk::GestureClick::new();
        trash_gesture.set_button(1);
        trash_gesture.connect_released(move |_, _, _, _| {
            let _ = cmd_tx_trash.send(AppCommand::Navigate {
                path: RavenPath::local(trash_path.clone()),
                pane_id,
            });
        });
        trash_row.add_controller(trash_gesture);
        trash_list.append(&trash_row);
        widget.append(&trash_list);

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

        let volume_monitor = gio::VolumeMonitor::get();

        // Populate volume list
        Self::populate_volume_list(&volume_list, &volume_monitor, &command_tx, pane_id);

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
            volume_list,
            volume_monitor,
            command_tx,
            pane_id,
            state,
            active_tag: Rc::new(RefCell::new(None)),
        }
    }

    /// Connect VolumeMonitor signals so the Devices section updates dynamically.
    pub fn connect_volume_signals(&self) {
        let vl = self.volume_list.clone();
        let vm = self.volume_monitor.clone();
        let cmd_tx = self.command_tx.clone();
        let pane_id = self.pane_id;

        let refresh = {
            let vl = vl.clone();
            let vm = vm.clone();
            let cmd_tx = cmd_tx.clone();
            move || {
                Self::populate_volume_list(&vl, &vm, &cmd_tx, pane_id);
            }
        };

        {
            let refresh = refresh.clone();
            self.volume_monitor.connect_mount_added(move |_, _| {
                refresh();
            });
        }
        {
            let refresh = refresh.clone();
            self.volume_monitor.connect_mount_removed(move |_, _| {
                refresh();
            });
        }
        {
            let refresh = refresh.clone();
            self.volume_monitor.connect_volume_added(move |_, _| {
                refresh();
            });
        }
        {
            let refresh = refresh.clone();
            self.volume_monitor.connect_volume_removed(move |_, _| {
                refresh();
            });
        }
    }

    /// Populate (or refresh) the volume list with mounted and unmounted volumes.
    fn populate_volume_list(
        volume_list: &gtk::ListBox,
        volume_monitor: &gio::VolumeMonitor,
        command_tx: &tokio::sync::mpsc::UnboundedSender<AppCommand>,
        pane_id: u32,
    ) {
        // Clear existing rows
        while let Some(child) = volume_list.first_child() {
            volume_list.remove(&child);
        }

        // Add filesystem root with disk space
        let root_row = Self::create_device_row("Computer", "computer-symbolic", "/", command_tx, pane_id);
        volume_list.append(&root_row);

        // Mounted volumes
        for mount in volume_monitor.mounts() {
            let name = mount.name().to_string();
            let root = mount.root();
            if let Some(path) = root.path() {
                let path_str = path.to_string_lossy().to_string();
                let icon_name = mount
                    .symbolic_icon()
                    .downcast::<gio::ThemedIcon>()
                    .ok()
                    .and_then(|themed| {
                        themed.names().into_iter()
                            .find(|n| n.ends_with("-symbolic"))
                            .map(|n| n.to_string())
                    })
                    .unwrap_or_else(|| "drive-harddisk-symbolic".to_string());
                let row = Self::create_mounted_device_row(
                    &name,
                    &icon_name,
                    &path_str,
                    command_tx,
                    pane_id,
                    &mount,
                );
                volume_list.append(&row);
            }
        }

        // Unmounted volumes
        for volume in volume_monitor.volumes() {
            if volume.get_mount().is_some() {
                continue; // Already shown as mounted
            }
            let name = volume.name().to_string();
            let row = Self::create_unmounted_volume_row(&name, &volume);
            volume_list.append(&row);
        }
    }

    /// Create a device row with disk space info (LevelBar + free/total label).
    fn create_device_row(
        name: &str,
        icon_name: &str,
        path_str: &str,
        command_tx: &tokio::sync::mpsc::UnboundedSender<AppCommand>,
        pane_id: u32,
    ) -> gtk::ListBoxRow {
        let vbox = gtk::Box::new(gtk::Orientation::Vertical, 2);
        vbox.set_margin_start(8);
        vbox.set_margin_end(8);
        vbox.set_margin_top(4);
        vbox.set_margin_bottom(4);

        let hbox = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        let icon = gtk::Image::from_icon_name(icon_name);
        icon.set_pixel_size(16);
        hbox.append(&icon);

        let lbl = gtk::Label::new(Some(name));
        lbl.set_halign(gtk::Align::Start);
        lbl.set_hexpand(true);
        lbl.set_ellipsize(gtk::pango::EllipsizeMode::End);
        hbox.append(&lbl);
        vbox.append(&hbox);

        // Disk space info
        if let Some((free, total)) = get_fs_space(path_str) {
            if total > 0 {
                let used = total.saturating_sub(free);
                let fraction = used as f64 / total as f64;

                let space_label = gtk::Label::new(Some(&format!(
                    "{} free of {}",
                    format_size(free),
                    format_size(total)
                )));
                space_label.set_halign(gtk::Align::Start);
                space_label.add_css_class("dim-label");
                space_label.add_css_class("caption");
                vbox.append(&space_label);

                let level_bar = gtk::LevelBar::new();
                level_bar.set_min_value(0.0);
                level_bar.set_max_value(1.0);
                level_bar.set_value(fraction);
                level_bar.set_height_request(4);
                level_bar.add_offset_value("low", 0.6);
                level_bar.add_offset_value("high", 0.8);
                level_bar.add_offset_value("full", 0.95);
                vbox.append(&level_bar);
            }
        }

        let row = gtk::ListBoxRow::new();
        row.set_child(Some(&vbox));

        let cmd_tx = command_tx.clone();
        let target_path = path_str.to_string();
        let gesture = gtk::GestureClick::new();
        gesture.set_button(1);
        gesture.connect_released(move |_, _, _, _| {
            let _ = cmd_tx.send(AppCommand::Navigate {
                path: RavenPath::local(PathBuf::from(&target_path)),
                pane_id,
            });
        });
        row.add_controller(gesture);

        row
    }

    /// Create a mounted device row with disk space and unmount context menu.
    fn create_mounted_device_row(
        name: &str,
        icon_name: &str,
        path_str: &str,
        command_tx: &tokio::sync::mpsc::UnboundedSender<AppCommand>,
        pane_id: u32,
        mount: &gio::Mount,
    ) -> gtk::ListBoxRow {
        let row = Self::create_device_row(name, icon_name, path_str, command_tx, pane_id);

        // Right-click: unmount context menu
        let menu = gio::Menu::new();
        menu.append(Some("Unmount"), Some("device.unmount"));

        let popover = gtk::PopoverMenu::from_model(Some(&menu));
        popover.set_has_arrow(false);
        popover.set_parent(&row);

        let action_group = gio::SimpleActionGroup::new();
        let action = gio::SimpleAction::new("unmount", None);

        let mount_for_unmount = mount.clone();
        action.connect_activate(move |_, _| {
            let mount_op = gio::MountOperation::new();
            mount_for_unmount.unmount_with_operation(
                gio::MountUnmountFlags::NONE,
                Some(&mount_op),
                gio::Cancellable::NONE,
                |result| {
                    match result {
                        Ok(()) => tracing::info!("Volume unmounted"),
                        Err(e) => tracing::error!("Unmount failed: {}", e),
                    }
                },
            );
        });
        action_group.add_action(&action);
        row.insert_action_group("device", Some(&action_group));

        let right_click = gtk::GestureClick::new();
        right_click.set_button(3);
        right_click.connect_released(move |_, _, x, y| {
            let rect = gtk::gdk::Rectangle::new(x as i32, y as i32, 1, 1);
            popover.set_pointing_to(Some(&rect));
            popover.popup();
        });
        row.add_controller(right_click);

        row
    }

    /// Create a dimmed row for unmounted volumes with click-to-mount.
    fn create_unmounted_volume_row(name: &str, volume: &gio::Volume) -> gtk::ListBoxRow {
        let hbox = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        hbox.set_margin_start(8);
        hbox.set_margin_end(8);
        hbox.set_margin_top(4);
        hbox.set_margin_bottom(4);

        let icon = gtk::Image::from_icon_name("drive-harddisk-symbolic");
        icon.set_pixel_size(16);
        icon.set_opacity(0.5);
        hbox.append(&icon);

        let lbl = gtk::Label::new(Some(&format!("{} (unmounted)", name)));
        lbl.set_halign(gtk::Align::Start);
        lbl.set_hexpand(true);
        lbl.set_ellipsize(gtk::pango::EllipsizeMode::End);
        lbl.set_opacity(0.5);
        hbox.append(&lbl);

        let mount_btn = gtk::Button::from_icon_name("media-mount-symbolic");
        mount_btn.add_css_class("flat");
        mount_btn.add_css_class("circular");
        mount_btn.set_tooltip_text(Some("Mount"));

        let volume_for_btn = volume.clone();
        mount_btn.connect_clicked(move |_| {
            let mount_op = gio::MountOperation::new();
            volume_for_btn.mount(
                gio::MountMountFlags::NONE,
                Some(&mount_op),
                gio::Cancellable::NONE,
                |result| {
                    match result {
                        Ok(()) => tracing::info!("Volume mounted"),
                        Err(e) => tracing::error!("Mount failed: {}", e),
                    }
                },
            );
        });
        hbox.append(&mount_btn);

        let row = gtk::ListBoxRow::new();
        row.set_child(Some(&hbox));

        // Click on the row also triggers mount
        let volume_for_click = volume.clone();
        let gesture = gtk::GestureClick::new();
        gesture.set_button(1);
        gesture.connect_released(move |_, _, _, _| {
            let mount_op = gio::MountOperation::new();
            volume_for_click.mount(
                gio::MountMountFlags::NONE,
                Some(&mount_op),
                gio::Cancellable::NONE,
                |result| {
                    match result {
                        Ok(()) => tracing::info!("Volume mounted"),
                        Err(e) => tracing::error!("Mount failed: {}", e),
                    }
                },
            );
        });
        row.add_controller(gesture);

        row
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

            // Click to filter by tag; clicking the active tag again clears the filter.
            let cmd_tx = self.command_tx.clone();
            let tag = tag_name.clone();
            let pane_id = self.pane_id;
            let state = self.state.clone();
            let active_tag = self.active_tag.clone();
            let tags_list = self.tags_list.clone();
            let row_ref = row.clone();
            let gesture = gtk::GestureClick::new();
            gesture.set_button(1);
            gesture.connect_released(move |_, _, _, _| {
                let Some(path) = state
                    .borrow()
                    .pane_by_id(pane_id)
                    .map(|p| p.current_path.clone())
                else {
                    return;
                };

                let mut active = active_tag.borrow_mut();
                let next = if active.as_deref() == Some(tag.as_str()) {
                    None
                } else {
                    Some(tag.clone())
                };
                *active = next.clone();

                if next.is_some() {
                    tags_list.select_row(Some(&row_ref));
                } else {
                    tags_list.unselect_all();
                }

                let _ = cmd_tx.send(AppCommand::FilterByTag {
                    tag: next,
                    pane_id,
                    path,
                });
            });
            row.add_controller(gesture);

            self.tags_list.append(&row);

            // Rows are rebuilt on every recount; keep the active tag visibly selected.
            if self.active_tag.borrow().as_deref() == Some(tag_name.as_str()) {
                self.tags_list.select_row(Some(&row));
            }
        }
    }

    /// Drop the tag filter without emitting a command. Used when the pane navigates
    /// away, since the new directory is listed unfiltered.
    pub fn clear_tag_filter(&self) {
        *self.active_tag.borrow_mut() = None;
        self.tags_list.unselect_all();
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
}

/// Get filesystem space info for a mount point: (free_bytes, total_bytes).
fn get_fs_space(path: &str) -> Option<(u64, u64)> {
    let c_path = std::ffi::CString::new(path).ok()?;
    let mut stat: libc::statvfs = unsafe { std::mem::zeroed() };
    let ret = unsafe { libc::statvfs(c_path.as_ptr(), &mut stat) };
    if ret == 0 {
        let total = stat.f_blocks as u64 * stat.f_frsize as u64;
        let free = stat.f_bavail as u64 * stat.f_frsize as u64;
        Some((free, total))
    } else {
        None
    }
}
