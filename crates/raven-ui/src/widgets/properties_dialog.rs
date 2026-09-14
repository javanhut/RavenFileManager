use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

use gtk4 as gtk;
use gtk::prelude::*;
use libadwaita as adw;
use libadwaita::prelude::*;

use raven_core::commands::AppCommand;
use raven_core::entry::FileEntry;
use raven_core::system_types::{PackageInfo, ProcessLock, SystemdUnit};

use crate::widgets::file_list::{format_local_time, format_size};

/// Modal properties dialog showing comprehensive file information with async-loaded system sections.
pub struct PropertiesDialog {
    pub window: adw::Window,
    file_path: Option<PathBuf>,
    // Async-loaded sections
    package_row: gtk::Label,
    package_spinner: gtk::Spinner,
    locks_box: gtk::Box,
    locks_spinner: gtk::Spinner,
    disk_usage_row: gtk::Label,
    disk_usage_spinner: gtk::Spinner,
    systemd_box: gtk::Box,
    systemd_spinner: gtk::Spinner,
    // Track which sections are visible
    disk_usage_section: gtk::Box,
    systemd_section: gtk::Box,
}

impl PropertiesDialog {
    pub fn new(
        parent: &adw::ApplicationWindow,
        entry: &FileEntry,
        command_tx: &tokio::sync::mpsc::UnboundedSender<AppCommand>,
    ) -> Rc<RefCell<Self>> {
        let window = adw::Window::builder()
            .title("Properties")
            .default_width(450)
            .default_height(550)
            .modal(true)
            .transient_for(parent)
            .build();

        let toolbar_view = adw::ToolbarView::new();
        let header = adw::HeaderBar::new();
        toolbar_view.add_top_bar(&header);

        let scroll = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vscrollbar_policy(gtk::PolicyType::Automatic)
            .hexpand(true)
            .vexpand(true)
            .build();

        let content = gtk::Box::new(gtk::Orientation::Vertical, 12);
        content.set_margin_start(16);
        content.set_margin_end(16);
        content.set_margin_top(12);
        content.set_margin_bottom(16);

        // --- Header: icon + name + type ---
        let header_box = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        header_box.set_halign(gtk::Align::Center);
        header_box.set_margin_bottom(8);

        let icon_name = crate::widgets::file_list::icon_for_entry(entry);
        let icon = gtk::Image::from_icon_name(&icon_name);
        icon.set_pixel_size(48);
        header_box.append(&icon);

        let name_box = gtk::Box::new(gtk::Orientation::Vertical, 2);
        let name_label = gtk::Label::new(Some(&entry.name));
        name_label.add_css_class("title-2");
        name_label.set_halign(gtk::Align::Start);
        name_label.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
        name_box.append(&name_label);

        let kind_str = match entry.kind {
            raven_core::entry::EntryKind::File => "Regular File",
            raven_core::entry::EntryKind::Directory => "Directory",
            raven_core::entry::EntryKind::Symlink => "Symbolic Link",
            raven_core::entry::EntryKind::BlockDevice => "Block Device",
            raven_core::entry::EntryKind::CharDevice => "Character Device",
            raven_core::entry::EntryKind::Fifo => "FIFO (Named Pipe)",
            raven_core::entry::EntryKind::Socket => "Socket",
            raven_core::entry::EntryKind::Unknown => "Unknown",
        };
        let kind_label = gtk::Label::new(Some(kind_str));
        kind_label.add_css_class("dim-label");
        kind_label.set_halign(gtk::Align::Start);
        name_box.append(&kind_label);

        header_box.append(&name_box);
        content.append(&header_box);

        // --- General section ---
        let (general_section, general_frame) = make_section("General");

        let path_str = format!("{}", entry.path);
        add_row(&general_frame, &make_info_row("Path", &path_str));

        if !entry.is_dir() {
            add_row(&general_frame, &make_info_row("Size", &format_size(entry.metadata.size)));
        }

        let perm_str = format!(
            "{} ({:o})",
            format_permissions_rwx(entry.metadata.permissions),
            entry.metadata.permissions & 0o7777
        );
        add_row(&general_frame, &make_info_row("Permissions", &perm_str));

        add_row(&general_frame, &make_info_row("Owner UID", &entry.metadata.owner_uid.to_string()));
        add_row(&general_frame, &make_info_row("Group GID", &entry.metadata.group_gid.to_string()));

        if let Some(dt) = entry.metadata.modified {
            add_row(&general_frame, &make_info_row("Modified", &format_local_time(&dt)));
        }
        if let Some(dt) = entry.metadata.created {
            add_row(&general_frame, &make_info_row("Created", &format_local_time(&dt)));
        }
        if let Some(ref mime) = entry.metadata.mime_type {
            add_row(&general_frame, &make_info_row("MIME Type", mime));
        }
        if let Some(ref target) = entry.metadata.symlink_target {
            add_row(&general_frame, &make_info_row("Link Target", &target.display().to_string()));
        }

        content.append(&general_section);

        // --- Package Owner section (async) ---
        let (package_section, package_row, package_spinner) = make_async_section("Package Owner");
        content.append(&package_section);

        // --- Process Locks section (async) ---
        let (locks_section, locks_list) = make_section("Open File Handles");
        let locks_spinner = gtk::Spinner::new();
        locks_spinner.set_spinning(true);
        locks_spinner.set_halign(gtk::Align::Center);
        let locks_box = gtk::Box::new(gtk::Orientation::Vertical, 4);
        let locks_content = gtk::Box::new(gtk::Orientation::Vertical, 4);
        locks_content.append(&locks_spinner);
        locks_content.append(&locks_box);
        add_row(&locks_list, &locks_content);
        content.append(&locks_section);

        // --- Disk Usage section (directories only, async) ---
        let (disk_usage_section, disk_usage_row, disk_usage_spinner) = make_async_section("Disk Usage");
        if entry.is_dir() {
            content.append(&disk_usage_section);
        }

        // --- Systemd Unit section (only for unit files) ---
        let (systemd_section_box, systemd_list) = make_section("Systemd Unit");
        let systemd_spinner = gtk::Spinner::new();
        systemd_spinner.set_spinning(true);
        systemd_spinner.set_halign(gtk::Align::Center);
        let systemd_box = gtk::Box::new(gtk::Orientation::Vertical, 4);
        let systemd_content = gtk::Box::new(gtk::Orientation::Vertical, 4);
        systemd_content.append(&systemd_spinner);
        systemd_content.append(&systemd_box);
        add_row(&systemd_list, &systemd_content);

        let is_systemd_unit = entry.name.ends_with(".service")
            || entry.name.ends_with(".timer")
            || entry.name.ends_with(".socket")
            || entry.name.ends_with(".mount")
            || entry.name.ends_with(".target");
        if is_systemd_unit {
            content.append(&systemd_section_box);
        }

        scroll.set_child(Some(&content));
        toolbar_view.set_content(Some(&scroll));
        window.set_content(Some(&toolbar_view));

        // Extract local path for backend commands
        let local_path = entry.path.as_local_path().map(|p| p.to_path_buf());

        // Send async queries to the backend
        if let Some(ref path) = local_path {
            let _ = command_tx.send(AppCommand::GetPackageOwner { path: path.clone() });
            let _ = command_tx.send(AppCommand::GetProcessLocks { path: path.clone() });
            if is_systemd_unit {
                let _ = command_tx.send(AppCommand::InspectSystemdUnit { path: path.clone() });
            }
        }
        if entry.is_dir() {
            let _ = command_tx.send(AppCommand::CalculateDiskUsage { path: entry.path.clone() });
        }

        let dialog = Rc::new(RefCell::new(Self {
            window,
            file_path: local_path,
            package_row,
            package_spinner,
            locks_box,
            locks_spinner,
            disk_usage_row,
            disk_usage_spinner,
            disk_usage_section,
            systemd_box,
            systemd_spinner,
            systemd_section: systemd_section_box,
        }));

        dialog
    }

    pub fn present(&self) {
        self.window.present();
        // Opened, nothing is being edited. The first selectable value takes
        // the initial focus as the window shows, which puts a caret in it and
        // selects all of its text; drop both, leaving the values selectable
        // and reachable with Tab.
        GtkWindowExt::set_focus(&self.window, None::<&gtk::Widget>);
        let mut pending: Vec<gtk::Widget> = self.window.first_child().into_iter().collect();
        while let Some(widget) = pending.pop() {
            if let Some(label) = widget.downcast_ref::<gtk::Label>() {
                if label.has_css_class("info-value") {
                    label.select_region(0, 0);
                }
            }
            pending.extend(widget.first_child());
            pending.extend(widget.next_sibling());
        }
    }

    pub fn file_path(&self) -> Option<&PathBuf> {
        self.file_path.as_ref()
    }

    pub fn update_package_info(&self, package: &Option<PackageInfo>) {
        self.package_spinner.set_spinning(false);
        self.package_spinner.set_visible(false);
        match package {
            Some(pkg) => {
                self.package_row.set_text(&format!(
                    "{} {} ({})",
                    pkg.name, pkg.version, pkg.manager
                ));
            }
            None => {
                self.package_row.set_text("Not owned by any package");
                self.package_row.add_css_class("dim-label");
            }
        }
        self.package_row.set_visible(true);
    }

    pub fn update_process_locks(&self, locks: &[ProcessLock]) {
        self.locks_spinner.set_spinning(false);
        self.locks_spinner.set_visible(false);

        if locks.is_empty() {
            let label = gtk::Label::new(Some("No open file handles"));
            label.add_css_class("dim-label");
            label.set_halign(gtk::Align::Start);
            self.locks_box.append(&label);
        } else {
            for lock in locks {
                let text = format!(
                    "{} (PID {}) - {}",
                    lock.process_name, lock.pid, lock.fd_type
                );
                let label = gtk::Label::new(Some(&text));
                label.set_halign(gtk::Align::Start);
                label.set_selectable(true);
                self.locks_box.append(&label);
            }
        }
    }

    pub fn update_disk_usage(&self, total_size: u64, total_items: u64) {
        self.disk_usage_spinner.set_spinning(false);
        self.disk_usage_spinner.set_visible(false);
        self.disk_usage_row.set_text(&format!(
            "{} items, {} total",
            total_items,
            format_size(total_size)
        ));
        self.disk_usage_row.set_visible(true);
    }

    pub fn update_systemd_unit(&self, unit: &SystemdUnit) {
        self.systemd_spinner.set_spinning(false);
        self.systemd_spinner.set_visible(false);

        if let Some(ref desc) = unit.description {
            let label = gtk::Label::new(Some(desc));
            label.set_halign(gtk::Align::Start);
            label.set_wrap(true);
            self.systemd_box.append(&label);
        }

        if let Some(ref state) = unit.active_state {
            let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
            let key = gtk::Label::new(Some("State:"));
            key.add_css_class("dim-label");
            row.append(&key);

            let val = gtk::Label::new(Some(state));
            match state.as_str() {
                "active" => val.add_css_class("success"),
                "failed" => val.add_css_class("error"),
                "inactive" => val.add_css_class("dim-label"),
                _ => {}
            }
            row.append(&val);
            self.systemd_box.append(&row);
        }

        for (key, value) in &unit.properties {
            self.systemd_box.append(&make_info_row(key, value));
        }
    }
}

/// A titled section: a heading over a rounded group of rows, the grouped
/// list Settings and Connect to Server use. Returns the section to place and
/// the group to fill with [`add_row`].
fn make_section(title: &str) -> (gtk::Box, gtk::ListBox) {
    let section = gtk::Box::new(gtk::Orientation::Vertical, 6);
    section.set_margin_top(4);

    let label = gtk::Label::new(Some(title));
    label.add_css_class("heading");
    label.set_halign(gtk::Align::Start);
    section.append(&label);

    let group = gtk::ListBox::new();
    group.add_css_class("boxed-list");
    group.set_selection_mode(gtk::SelectionMode::None);
    section.append(&group);

    (section, group)
}

/// Add `content` to a section's group as a row that only shows information:
/// not activatable and not a focus stop.
fn add_row(group: &gtk::ListBox, content: &impl IsA<gtk::Widget>) {
    content.set_margin_start(12);
    content.set_margin_end(12);
    content.set_margin_top(8);
    content.set_margin_bottom(8);
    content.set_valign(gtk::Align::Center);
    let row = gtk::ListBoxRow::new();
    row.set_activatable(false);
    row.set_focusable(false);
    row.set_child(Some(content));
    group.append(&row);
}

/// Create a section with a spinner and a result label (initially hidden),
/// both in one row so the spinner sits centred where the result will be.
fn make_async_section(title: &str) -> (gtk::Box, gtk::Label, gtk::Spinner) {
    let (section, group) = make_section(title);

    let content = gtk::Box::new(gtk::Orientation::Vertical, 0);
    let spinner = gtk::Spinner::new();
    spinner.set_spinning(true);
    spinner.set_halign(gtk::Align::Center);
    content.append(&spinner);

    let label = gtk::Label::new(None);
    label.set_halign(gtk::Align::Start);
    label.set_selectable(true);
    label.set_wrap(true);
    label.set_visible(false);
    content.append(&label);
    add_row(&group, &content);

    (section, label, spinner)
}

/// Create a key-value info row.
fn make_info_row(key: &str, value: &str) -> gtk::Box {
    let row = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    row.set_margin_top(2);
    row.set_margin_bottom(2);

    let key_label = gtk::Label::new(Some(key));
    key_label.add_css_class("dim-label");
    key_label.set_halign(gtk::Align::Start);
    key_label.set_width_chars(14);
    key_label.set_xalign(0.0);
    row.append(&key_label);

    let val_label = gtk::Label::new(Some(value));
    val_label.set_halign(gtk::Align::Start);
    val_label.set_hexpand(true);
    val_label.set_selectable(true);
    val_label.add_css_class("info-value");
    val_label.set_wrap(true);
    val_label.set_xalign(0.0);
    row.append(&val_label);

    row
}

fn format_permissions_rwx(mode: u32) -> String {
    let mut s = String::with_capacity(10);
    s.push(if mode & 0o40000 != 0 { 'd' } else { '-' });
    s.push(if mode & 0o400 != 0 { 'r' } else { '-' });
    s.push(if mode & 0o200 != 0 { 'w' } else { '-' });
    s.push(if mode & 0o100 != 0 { 'x' } else { '-' });
    s.push(if mode & 0o40 != 0 { 'r' } else { '-' });
    s.push(if mode & 0o20 != 0 { 'w' } else { '-' });
    s.push(if mode & 0o10 != 0 { 'x' } else { '-' });
    s.push(if mode & 0o4 != 0 { 'r' } else { '-' });
    s.push(if mode & 0o2 != 0 { 'w' } else { '-' });
    s.push(if mode & 0o1 != 0 { 'x' } else { '-' });
    s
}
