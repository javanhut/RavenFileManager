use std::cell::RefCell;
use std::collections::HashMap;
use std::path::PathBuf;
use std::rc::Rc;

use gtk4 as gtk;
use gtk::prelude::*;

use raven_core::commands::AppCommand;
use raven_core::config::Bookmark;
use raven_core::path::RavenPath;

use crate::state::{AppState, PaneResolver};
use crate::widgets::file_list::format_size;

/// Sidebar with bookmarks, mounted volumes, and tags.
pub struct Sidebar {
    pub widget: gtk::Box,
    bookmarks_list: gtk::ListBox,
    tags_label: gtk::Label,
    tags_list: gtk::ListBox,
    volume_list: gtk::ListBox,
    volume_monitor: gio::VolumeMonitor,
    command_tx: tokio::sync::mpsc::UnboundedSender<AppCommand>,
    /// The pane sidebar clicks navigate: whichever one is active.
    pane: PaneResolver,
    state: AppState,
    /// Tag currently filtering the pane, so a second click on the same row clears it.
    active_tag: Rc<RefCell<Option<String>>>,
    /// Network section: one row per open remote connection, then the
    /// "Connect to Server" row.
    remote_list: gtk::ListBox,
    remote_rows: RefCell<HashMap<String, gtk::ListBoxRow>>,
    /// What "Connect to Server" does; the window supplies the dialog.
    connect_handler: Rc<RefCell<Option<Box<dyn Fn()>>>>,
    /// What the "Recent" row does; the window supplies the listing.
    recent_handler: Rc<RefCell<Option<Box<dyn Fn()>>>>,
}

impl Sidebar {
    pub fn new(
        state: AppState,
        command_tx: tokio::sync::mpsc::UnboundedSender<AppCommand>,
        pane: PaneResolver,
    ) -> Self {
        let widget = gtk::Box::new(gtk::Orientation::Vertical, 0);
        // The scrolled window around this carries Raven's `.sidebar` and its
        // padding; this is only the column of sections.
        widget.set_width_request(184);

        // Bookmarks section
        widget.append(&section_heading("Places", true));

        let bookmarks_list = gtk::ListBox::new();
        bookmarks_list.set_selection_mode(gtk::SelectionMode::Single);
        bookmarks_list.add_css_class("navigation-sidebar");

        let bookmarks = {
            let s = state.borrow();
            s.config.bookmarks.clone()
        };

        for bookmark in &bookmarks {
            let row = Self::create_bookmark_row(bookmark, &command_tx, &pane, &state);
            bookmarks_list.append(&row);
        }

        // --- Drop target on bookmarks_list: drag folders to pin as bookmarks ---
        {
            let pin_drop_target = gtk::DropTarget::new(
                gtk::glib::types::Type::INVALID,
                gtk::gdk::DragAction::COPY | gtk::gdk::DragAction::MOVE,
            );
            // Other applications offer a file list; Raven's listings also a URI string.
            pin_drop_target
                .set_types(&[gtk::gdk::FileList::static_type(), gtk::glib::Type::STRING]);

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
            let pane_for_pin = pane.clone();
            pin_drop_target.connect_drop(move |_target, value, _x, _y| {
                {
                    let paths: Vec<PathBuf> = crate::dnd::paths_from_value(value);

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
                                    &pane_for_pin,
                                    &state_for_pin,
                                );
                                bl_for_drop.append(&row);
                                pinned_any = true;
                            }
                        }
                    }
                    pinned_any
                }
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

        // Recent files: not a folder, so the window lists them itself.
        let recent_handler: Rc<RefCell<Option<Box<dyn Fn()>>>> = Rc::new(RefCell::new(None));
        {
            let hbox = gtk::Box::new(gtk::Orientation::Horizontal, 8);
            hbox.set_margin_start(0);
            hbox.set_margin_end(0);
            hbox.set_margin_top(1);
            hbox.set_margin_bottom(1);
            hbox.append(&nav_tile(crate::widgets::pane_view::RECENT_ICON));
            let lbl = gtk::Label::new(Some("Recent"));
            lbl.set_halign(gtk::Align::Start);
            hbox.append(&lbl);
            let row = gtk::ListBoxRow::new();
            row.set_child(Some(&hbox));

            let handler = recent_handler.clone();
            let gesture = gtk::GestureClick::new();
            gesture.set_button(1);
            gesture.connect_released(move |_, _, _, _| {
                if let Some(cb) = handler.borrow().as_ref() {
                    cb();
                }
            });
            row.add_controller(gesture);
            trash_list.append(&row);
        }

        let trash_hbox = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        trash_hbox.set_margin_start(0);
        trash_hbox.set_margin_end(0);
        trash_hbox.set_margin_top(1);
        trash_hbox.set_margin_bottom(1);
        trash_hbox.append(&nav_tile("user-trash-symbolic"));
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
        let pane_for_trash = pane.clone();
        let trash_gesture = gtk::GestureClick::new();
        trash_gesture.set_button(1);
        trash_gesture.connect_released(move |_, _, _, _| {
            let _ = cmd_tx_trash.send(AppCommand::Navigate {
                path: RavenPath::local(trash_path.clone()),
                pane_id: pane_for_trash(),
            });
        });
        trash_row.add_controller(trash_gesture);
        trash_list.append(&trash_row);
        widget.append(&trash_list);

        // Volumes section. Sections are parted by their headings and air,
        // not by rules.
        widget.append(&section_heading("Devices", false));

        let volume_list = gtk::ListBox::new();
        volume_list.set_selection_mode(gtk::SelectionMode::Single);
        volume_list.add_css_class("navigation-sidebar");

        let volume_monitor = gio::VolumeMonitor::get();

        // Populate volume list
        Self::populate_volume_list(&volume_list, &volume_monitor, &command_tx, &pane);

        widget.append(&volume_list);

        // Network section
        widget.append(&section_heading("Network", false));

        let remote_list = gtk::ListBox::new();
        remote_list.set_selection_mode(gtk::SelectionMode::None);
        remote_list.add_css_class("navigation-sidebar");

        let connect_handler: Rc<RefCell<Option<Box<dyn Fn()>>>> = Rc::new(RefCell::new(None));
        {
            let connect_hbox = gtk::Box::new(gtk::Orientation::Horizontal, 8);
            connect_hbox.set_margin_start(0);
            connect_hbox.set_margin_end(0);
            connect_hbox.set_margin_top(1);
            connect_hbox.set_margin_bottom(1);
            connect_hbox.append(&nav_tile("network-server-symbolic"));
            let lbl = gtk::Label::new(Some("Connect to Server..."));
            lbl.set_halign(gtk::Align::Start);
            connect_hbox.append(&lbl);
            let connect_row = gtk::ListBoxRow::new();
            connect_row.set_child(Some(&connect_hbox));
            connect_row.set_activatable(true);

            let handler = connect_handler.clone();
            let gesture = gtk::GestureClick::new();
            gesture.set_button(1);
            gesture.connect_released(move |_, _, _, _| {
                if let Some(cb) = handler.borrow().as_ref() {
                    cb();
                }
            });
            connect_row.add_controller(gesture);
            remote_list.append(&connect_row);
        }
        widget.append(&remote_list);

        // Tags section, its heading shown only while there are tags to list.
        let tags_label = section_heading("Tags", false);
        tags_label.set_visible(false);
        widget.append(&tags_label);

        let tags_list = gtk::ListBox::new();
        tags_list.set_selection_mode(gtk::SelectionMode::Single);
        tags_list.add_css_class("navigation-sidebar");
        widget.append(&tags_list);

        Self {
            widget,
            bookmarks_list,
            tags_label,
            tags_list,
            volume_list,
            volume_monitor,
            command_tx,
            pane,
            state,
            active_tag: Rc::new(RefCell::new(None)),
            remote_list,
            remote_rows: RefCell::new(HashMap::new()),
            connect_handler,
            recent_handler,
        }
    }

    /// Set what the "Recent" row shows.
    pub fn set_recent_handler(&self, cb: impl Fn() + 'static) {
        *self.recent_handler.borrow_mut() = Some(Box::new(cb));
    }

    /// Set what the "Connect to Server" row opens.
    pub fn set_connect_handler(&self, cb: impl Fn() + 'static) {
        *self.connect_handler.borrow_mut() = Some(Box::new(cb));
    }

    /// Show an open remote connection: clicking it browses `path`, the
    /// eject button disconnects it.
    pub fn add_remote(&self, id: &str, label: &str, path: &RavenPath) {
        self.remove_remote(id);

        let hbox = gtk::Box::new(gtk::Orientation::Horizontal, 4);
        hbox.set_margin_start(4);
        hbox.set_margin_end(4);

        let open_btn = gtk::Button::new();
        open_btn.add_css_class("flat");
        open_btn.add_css_class("remote");
        open_btn.set_hexpand(true);
        open_btn.set_tooltip_text(Some(&path.to_string()));
        let open_content = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        open_content.append(&nav_tile("folder-remote-symbolic"));
        let lbl = gtk::Label::new(Some(label));
        lbl.set_halign(gtk::Align::Start);
        lbl.set_hexpand(true);
        lbl.set_ellipsize(gtk::pango::EllipsizeMode::End);
        open_content.append(&lbl);
        open_btn.set_child(Some(&open_content));
        {
            let cmd_tx = self.command_tx.clone();
            let pane = self.pane.clone();
            let path = path.clone();
            open_btn.connect_clicked(move |_| {
                let _ = cmd_tx.send(AppCommand::Navigate {
                    path: path.clone(),
                    pane_id: pane(),
                });
            });
        }
        hbox.append(&open_btn);

        let eject_btn = gtk::Button::from_icon_name("media-eject-symbolic");
        eject_btn.add_css_class("flat");
        eject_btn.set_tooltip_text(Some("Disconnect"));
        eject_btn.set_valign(gtk::Align::Center);
        {
            let cmd_tx = self.command_tx.clone();
            let id = id.to_string();
            eject_btn.connect_clicked(move |_| {
                let _ = cmd_tx.send(AppCommand::DisconnectRemote { id: id.clone() });
            });
        }
        hbox.append(&eject_btn);

        let row = gtk::ListBoxRow::new();
        row.set_child(Some(&hbox));
        row.set_activatable(false);

        // Connections go above the "Connect to Server" row, which stays last.
        let position = self.remote_rows.borrow().len() as i32;
        self.remote_list.insert(&row, position);
        self.remote_rows.borrow_mut().insert(id.to_string(), row);
    }

    /// The ids of the connections currently listed.
    pub fn remote_ids(&self) -> Vec<String> {
        self.remote_rows.borrow().keys().cloned().collect()
    }

    /// Drop the row for a connection that is gone.
    pub fn remove_remote(&self, id: &str) {
        if let Some(row) = self.remote_rows.borrow_mut().remove(id) {
            self.remote_list.remove(&row);
        }
    }

    /// Connect VolumeMonitor signals so the Devices section updates dynamically.
    pub fn connect_volume_signals(&self) {
        let vl = self.volume_list.clone();
        let vm = self.volume_monitor.clone();
        let cmd_tx = self.command_tx.clone();
        let pane = self.pane.clone();

        let refresh = {
            let vl = vl.clone();
            let vm = vm.clone();
            let cmd_tx = cmd_tx.clone();
            let pane = pane.clone();
            move || {
                Self::populate_volume_list(&vl, &vm, &cmd_tx, &pane);
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
        pane: &PaneResolver,
    ) {
        // Clear existing rows
        while let Some(child) = volume_list.first_child() {
            volume_list.remove(&child);
        }

        // Add filesystem root with disk space
        let root_row = Self::create_device_row("Computer", "computer-symbolic", "/", command_tx, pane);
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
                    pane,
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
        pane: &PaneResolver,
    ) -> gtk::ListBoxRow {
        let vbox = gtk::Box::new(gtk::Orientation::Vertical, 2);
        vbox.set_margin_start(0);
        vbox.set_margin_end(0);
        vbox.set_margin_top(1);
        vbox.set_margin_bottom(1);

        let hbox = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        hbox.append(&nav_tile(icon_name));

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
                space_label.add_css_class("caption");
                vbox.append(&space_label);

                let level_bar = gtk::LevelBar::new();
                level_bar.set_min_value(0.0);
                level_bar.set_max_value(1.0);
                level_bar.set_value(fraction);
                level_bar.set_height_request(3);
                level_bar.add_offset_value("low", 0.6);
                level_bar.add_offset_value("high", 0.8);
                level_bar.add_offset_value("full", 0.95);
                vbox.append(&level_bar);
            }
        }

        let row = gtk::ListBoxRow::new();
        row.set_child(Some(&vbox));

        let cmd_tx = command_tx.clone();
        let pane = pane.clone();
        let target_path = path_str.to_string();
        let gesture = gtk::GestureClick::new();
        gesture.set_button(1);
        gesture.connect_released(move |_, _, _, _| {
            let _ = cmd_tx.send(AppCommand::Navigate {
                path: RavenPath::local(PathBuf::from(&target_path)),
                pane_id: pane(),
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
        pane: &PaneResolver,
        mount: &gio::Mount,
    ) -> gtk::ListBoxRow {
        let row = Self::create_device_row(name, icon_name, path_str, command_tx, pane);

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
        hbox.set_margin_start(0);
        hbox.set_margin_end(0);
        hbox.set_margin_top(1);
        hbox.set_margin_bottom(1);

        // The tile and the label are both dimmed by colour (`.unmounted` in
        // style.css), not opacity, so neither stacks with anything else that
        // dims.
        let tile = nav_tile("drive-harddisk-symbolic");
        hbox.append(&tile);

        let lbl = gtk::Label::new(Some(&format!("{} (unmounted)", name)));
        lbl.set_halign(gtk::Align::Start);
        lbl.set_hexpand(true);
        lbl.set_ellipsize(gtk::pango::EllipsizeMode::End);
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
        row.add_css_class("unmounted");
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
        self.tags_label
            .set_visible(counts.iter().any(|(_, count)| *count > 0));

        for (tag_name, count) in counts {
            if *count == 0 {
                continue;
            }
            let hbox = gtk::Box::new(gtk::Orientation::Horizontal, 8);
            hbox.set_margin_start(0);
            hbox.set_margin_end(0);
            hbox.set_margin_top(1);
            hbox.set_margin_bottom(1);

            hbox.append(&nav_tile("tag-symbolic"));

            let lbl = gtk::Label::new(Some(tag_name));
            lbl.set_halign(gtk::Align::Start);
            lbl.set_hexpand(true);
            lbl.set_ellipsize(gtk::pango::EllipsizeMode::End);
            hbox.append(&lbl);

            let count_lbl = gtk::Label::new(Some(&count.to_string()));
            count_lbl.add_css_class("count");
            hbox.append(&count_lbl);

            let row = gtk::ListBoxRow::new();
            row.set_child(Some(&hbox));

            // Click to filter by tag; clicking the active tag again clears the filter.
            let cmd_tx = self.command_tx.clone();
            let tag = tag_name.clone();
            let pane = self.pane.clone();
            let state = self.state.clone();
            let active_tag = self.active_tag.clone();
            let tags_list = self.tags_list.clone();
            let row_ref = row.clone();
            let gesture = gtk::GestureClick::new();
            gesture.set_button(1);
            gesture.connect_released(move |_, _, _, _| {
                let pane_id = pane();
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
        let row = Self::create_bookmark_row(bookmark, &self.command_tx, &self.pane, &self.state);
        self.bookmarks_list.append(&row);
    }

    fn create_bookmark_row(
        bookmark: &Bookmark,
        command_tx: &tokio::sync::mpsc::UnboundedSender<AppCommand>,
        pane: &PaneResolver,
        state: &AppState,
    ) -> gtk::ListBoxRow {
        let icon_name = bookmark
            .icon
            .as_deref()
            .unwrap_or("folder-symbolic");

        let hbox = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        hbox.set_margin_start(0);
        hbox.set_margin_end(0);
        hbox.set_margin_top(1);
        hbox.set_margin_bottom(1);

        hbox.append(&nav_tile(icon_name));

        let lbl = gtk::Label::new(Some(&bookmark.name));
        lbl.set_halign(gtk::Align::Start);
        lbl.set_ellipsize(gtk::pango::EllipsizeMode::End);
        hbox.append(&lbl);

        let row = gtk::ListBoxRow::new();
        row.set_child(Some(&hbox));
        row.set_widget_name(&bookmark.path);

        // Navigate on left-click
        let cmd_tx = command_tx.clone();
        let pane = pane.clone();
        let target_path = bookmark.path.clone();
        let gesture = gtk::GestureClick::new();
        gesture.set_button(1);
        gesture.connect_released(move |_, _, _, _| {
            let _ = cmd_tx.send(AppCommand::Navigate {
                path: RavenPath::local(PathBuf::from(&target_path)),
                pane_id: pane(),
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
        let drop_dest = RavenPath::local(PathBuf::from(&bookmark.path));
        let drop_target = {
            let dest = drop_dest.clone();
            crate::dnd::file_drop_target(Some("drop-highlight"), move || Some(dest.clone()))
        };
        let cmd_tx_drop = command_tx.clone();
        drop_target.connect_drop(move |target, value, _x, _y| {
            crate::dnd::perform_drop(target, value, &drop_dest, &cmd_tx_drop)
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

/// A section heading in Raven's eyebrow style: small, heavy, tracked out and
/// in capitals, so it labels the rows under it without competing with them.
pub fn section_heading(text: &str, first: bool) -> gtk::Label {
    let label = gtk::Label::new(Some(&text.to_uppercase()));
    label.set_halign(gtk::Align::Start);
    label.add_css_class("eyebrow");
    if first {
        label.add_css_class("first");
    }
    label
}

/// An icon on the tinted tile the Raven apps put beside sidebar rows. The
/// tint says what kind of place a row is and never follows the accent, which
/// is kept for the selected row.
pub fn nav_tile(icon_name: &str) -> gtk::Box {
    let tile = gtk::Box::new(gtk::Orientation::Vertical, 0);
    tile.add_css_class("nav-icon");
    tile.add_css_class(nav_tint(icon_name));
    tile.set_halign(gtk::Align::Center);
    tile.set_valign(gtk::Align::Center);
    let icon = gtk::Image::from_icon_name(icon_name);
    icon.set_halign(gtk::Align::Center);
    icon.set_valign(gtk::Align::Center);
    icon.set_vexpand(true);
    tile.append(&icon);
    tile
}

/// The `.nav-icon` tint for a place, by its icon.
fn nav_tint(icon_name: &str) -> &'static str {
    match icon_name {
        "user-home-symbolic" => "blue",
        "user-desktop-symbolic" => "indigo",
        "folder-documents-symbolic" => "orange",
        "folder-download-symbolic" => "green",
        "folder-music-symbolic" => "pink",
        "folder-pictures-symbolic" => "purple",
        "folder-videos-symbolic" => "red",
        "document-open-recent-symbolic" => "teal",
        "user-trash-symbolic" => "graphite",
        "network-server-symbolic" | "folder-remote-symbolic" => "cyan",
        "tag-symbolic" => "yellow",
        name if name.starts_with("computer")
            || name.starts_with("drive-")
            || name.starts_with("media-") =>
        {
            "gray"
        }
        _ => "blue",
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
