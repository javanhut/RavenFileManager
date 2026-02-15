use gtk4 as gtk;
use gtk::prelude::*;
use gtk::glib;
use gtk::subclass::prelude::*;

use raven_core::commands::AppCommand;
use raven_core::entry::{EntryKind, FileEntry};
use raven_core::path::RavenPath;

use crate::state::AppState;

// GObject wrapper for FileEntry to use with ColumnView
mod imp {
    use std::cell::RefCell;
    use gtk4::glib;
    use gtk4::glib::Properties;
    use gtk4::prelude::*;
    use gtk4::subclass::prelude::*;
    use raven_core::entry::FileEntry;

    #[derive(Default, Properties)]
    #[properties(wrapper_type = super::FileEntryObject)]
    pub struct FileEntryObject {
        #[property(get, set)]
        name: RefCell<String>,
        #[property(get, set)]
        size: RefCell<u64>,
        #[property(get, set)]
        modified: RefCell<String>,
        #[property(get, set)]
        permissions: RefCell<String>,
        #[property(get, set)]
        icon_name: RefCell<String>,
        #[property(get, set)]
        is_dir: RefCell<bool>,
        #[property(get, set)]
        is_hidden: RefCell<bool>,
        pub entry: RefCell<Option<FileEntry>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for FileEntryObject {
        const NAME: &'static str = "RavenFileEntryObject";
        type Type = super::FileEntryObject;
        type ParentType = glib::Object;
    }

    #[glib::derived_properties]
    impl ObjectImpl for FileEntryObject {}
}

glib::wrapper! {
    pub struct FileEntryObject(ObjectSubclass<imp::FileEntryObject>);
}

impl FileEntryObject {
    pub fn new(entry: &FileEntry) -> Self {
        let icon = icon_for_entry(entry);
        let permissions = format_permissions(entry.metadata.permissions);
        let modified = entry
            .metadata
            .modified
            .map(|dt| dt.format("%Y-%m-%d %H:%M").to_string())
            .unwrap_or_default();

        let obj: Self = glib::Object::builder()
            .property("name", &entry.name)
            .property("size", entry.metadata.size)
            .property("modified", &modified)
            .property("permissions", &permissions)
            .property("icon-name", &icon)
            .property("is-dir", entry.is_dir())
            .property("is-hidden", entry.is_hidden())
            .build();

        obj.imp().entry.replace(Some(entry.clone()));
        obj
    }

    pub fn entry(&self) -> Option<FileEntry> {
        self.imp().entry.borrow().clone()
    }
}

pub fn icon_for_entry(entry: &FileEntry) -> String {
    match entry.kind {
        EntryKind::Directory => "folder-symbolic".to_string(),
        EntryKind::Symlink => "emblem-symbolic-link-symbolic".to_string(),
        EntryKind::File => {
            if entry.metadata.is_executable {
                "application-x-executable-symbolic".to_string()
            } else {
                match entry.extension() {
                    Some("rs") | Some("py") | Some("js") | Some("ts") | Some("c") | Some("cpp")
                    | Some("h") | Some("go") | Some("java") | Some("rb") | Some("sh") => {
                        "text-x-script-symbolic".to_string()
                    }
                    Some("png") | Some("jpg") | Some("jpeg") | Some("gif") | Some("svg")
                    | Some("webp") | Some("bmp") => "image-x-generic-symbolic".to_string(),
                    Some("mp3") | Some("flac") | Some("ogg") | Some("wav") | Some("m4a") => {
                        "audio-x-generic-symbolic".to_string()
                    }
                    Some("mp4") | Some("mkv") | Some("avi") | Some("webm") | Some("mov") => {
                        "video-x-generic-symbolic".to_string()
                    }
                    Some("pdf") => "x-office-document-symbolic".to_string(),
                    Some("zip") | Some("tar") | Some("gz") | Some("xz") | Some("bz2")
                    | Some("7z") | Some("rar") => "package-x-generic-symbolic".to_string(),
                    Some("txt") | Some("md") | Some("toml") | Some("json") | Some("yaml")
                    | Some("yml") | Some("xml") | Some("csv") | Some("log") => {
                        "text-x-generic-symbolic".to_string()
                    }
                    _ => "text-x-generic-symbolic".to_string(),
                }
            }
        }
        _ => "text-x-generic-symbolic".to_string(),
    }
}

fn format_permissions(mode: u32) -> String {
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

pub fn format_size(size: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = 1024 * KB;
    const GB: u64 = 1024 * MB;
    const TB: u64 = 1024 * GB;

    if size >= TB {
        format!("{:.1} TB", size as f64 / TB as f64)
    } else if size >= GB {
        format!("{:.1} GB", size as f64 / GB as f64)
    } else if size >= MB {
        format!("{:.1} MB", size as f64 / MB as f64)
    } else if size >= KB {
        format!("{:.1} KB", size as f64 / KB as f64)
    } else {
        format!("{} B", size)
    }
}

/// Build a ColumnView for file listing with icon, name, size, modified, permissions columns.
pub struct FileListView {
    pub scrolled_window: gtk::ScrolledWindow,
    pub column_view: gtk::ColumnView,
    pub model: gio::ListStore,
    pub selection: gtk::SingleSelection,
}

impl FileListView {
    pub fn new(
        state: AppState,
        command_tx: tokio::sync::mpsc::UnboundedSender<AppCommand>,
        pane_id: u32,
    ) -> Self {
        let model = gio::ListStore::new::<FileEntryObject>();
        let selection = gtk::SingleSelection::new(Some(model.clone()));

        let column_view = gtk::ColumnView::new(Some(selection.clone()));
        column_view.set_show_column_separators(true);
        column_view.set_show_row_separators(false);
        column_view.add_css_class("data-table");

        // Name column (icon + name)
        let name_factory = gtk::SignalListItemFactory::new();
        name_factory.connect_setup(|_, item| {
            let item = item.downcast_ref::<gtk::ListItem>().unwrap();
            let hbox = gtk::Box::new(gtk::Orientation::Horizontal, 8);
            let icon = gtk::Image::new();
            icon.set_pixel_size(20);
            let label = gtk::Label::new(None);
            label.set_halign(gtk::Align::Start);
            label.set_ellipsize(gtk::pango::EllipsizeMode::End);
            hbox.append(&icon);
            hbox.append(&label);
            item.set_child(Some(&hbox));
        });
        name_factory.connect_bind(|_, item| {
            let item = item.downcast_ref::<gtk::ListItem>().unwrap();
            let entry_obj = item.item().and_downcast::<FileEntryObject>().unwrap();
            let hbox = item.child().and_downcast::<gtk::Box>().unwrap();
            let icon = hbox.first_child().and_downcast::<gtk::Image>().unwrap();
            let label = icon.next_sibling().and_downcast::<gtk::Label>().unwrap();
            icon.set_icon_name(Some(&entry_obj.icon_name()));
            label.set_text(&entry_obj.name());
            if entry_obj.is_hidden() {
                label.set_opacity(0.5);
            } else {
                label.set_opacity(1.0);
            }
        });
        let name_col = gtk::ColumnViewColumn::new(Some("Name"), Some(name_factory));
        name_col.set_expand(true);
        name_col.set_resizable(true);
        column_view.append_column(&name_col);

        // Size column
        let size_factory = gtk::SignalListItemFactory::new();
        size_factory.connect_setup(|_, item| {
            let item = item.downcast_ref::<gtk::ListItem>().unwrap();
            let label = gtk::Label::new(None);
            label.set_halign(gtk::Align::End);
            item.set_child(Some(&label));
        });
        size_factory.connect_bind(|_, item| {
            let item = item.downcast_ref::<gtk::ListItem>().unwrap();
            let entry_obj = item.item().and_downcast::<FileEntryObject>().unwrap();
            let label = item.child().and_downcast::<gtk::Label>().unwrap();
            let size = entry_obj.size();
            if entry_obj.is_dir() && size == 0 {
                label.set_text("...");
            } else {
                label.set_text(&format_size(size));
            }
        });
        let size_col = gtk::ColumnViewColumn::new(Some("Size"), Some(size_factory));
        size_col.set_fixed_width(100);
        size_col.set_resizable(true);
        column_view.append_column(&size_col);

        // Modified column
        let mod_factory = gtk::SignalListItemFactory::new();
        mod_factory.connect_setup(|_, item| {
            let item = item.downcast_ref::<gtk::ListItem>().unwrap();
            let label = gtk::Label::new(None);
            label.set_halign(gtk::Align::Start);
            item.set_child(Some(&label));
        });
        mod_factory.connect_bind(|_, item| {
            let item = item.downcast_ref::<gtk::ListItem>().unwrap();
            let entry_obj = item.item().and_downcast::<FileEntryObject>().unwrap();
            let label = item.child().and_downcast::<gtk::Label>().unwrap();
            label.set_text(&entry_obj.modified());
        });
        let mod_col = gtk::ColumnViewColumn::new(Some("Modified"), Some(mod_factory));
        mod_col.set_fixed_width(160);
        mod_col.set_resizable(true);
        column_view.append_column(&mod_col);

        // Permissions column
        let perm_factory = gtk::SignalListItemFactory::new();
        perm_factory.connect_setup(|_, item| {
            let item = item.downcast_ref::<gtk::ListItem>().unwrap();
            let label = gtk::Label::new(None);
            label.set_halign(gtk::Align::Start);
            label.add_css_class("monospace");
            item.set_child(Some(&label));
        });
        perm_factory.connect_bind(|_, item| {
            let item = item.downcast_ref::<gtk::ListItem>().unwrap();
            let entry_obj = item.item().and_downcast::<FileEntryObject>().unwrap();
            let label = item.child().and_downcast::<gtk::Label>().unwrap();
            label.set_text(&entry_obj.permissions());
        });
        let perm_col = gtk::ColumnViewColumn::new(Some("Permissions"), Some(perm_factory));
        perm_col.set_fixed_width(120);
        perm_col.set_resizable(true);
        column_view.append_column(&perm_col);

        // Handle double-click / activation to navigate into directories
        let cmd_tx = command_tx.clone();
        let sel_model = selection.clone();
        let state_for_activate = state.clone();
        column_view.connect_activate(move |_, pos| {
            if let Some(item) = sel_model.item(pos) {
                if let Some(entry_obj) = item.downcast_ref::<FileEntryObject>() {
                    if let Some(entry) = entry_obj.entry() {
                        if entry.is_dir() {
                            // Update navigation history
                            {
                                let mut s = state_for_activate.borrow_mut();
                                if let Some(pane) = s.pane_by_id_mut(pane_id) {
                                    pane.navigate_to(entry.path.clone());
                                }
                            }
                            let _ = cmd_tx.send(AppCommand::Navigate {
                                path: entry.path.clone(),
                                pane_id,
                            });
                        } else {
                            // Open file with default application
                            if let Some(local) = entry.path.as_local_path() {
                                let uri = format!("file://{}", local.display());
                                if let Err(e) = open::that(&uri) {
                                    tracing::warn!("Failed to open {}: {}", uri, e);
                                }
                            }
                        }
                    }
                }
            }
        });

        let scrolled_window = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Automatic)
            .vscrollbar_policy(gtk::PolicyType::Automatic)
            .hexpand(true)
            .vexpand(true)
            .child(&column_view)
            .build();

        Self {
            scrolled_window,
            column_view,
            model,
            selection,
        }
    }

    pub fn set_entries(&self, entries: &[FileEntry], show_hidden: bool) {
        self.model.remove_all();
        for entry in entries {
            if !show_hidden && entry.is_hidden() {
                continue;
            }
            self.model.append(&FileEntryObject::new(entry));
        }
    }

    /// Update a directory entry's displayed size after async calculation.
    pub fn update_dir_size(&self, path: &RavenPath, size: u64) {
        let n = self.model.n_items();
        for i in 0..n {
            if let Some(obj) = self.model.item(i).and_then(|o| o.downcast::<FileEntryObject>().ok()) {
                let matches = obj
                    .entry()
                    .map(|e| e.is_dir() && &e.path == path)
                    .unwrap_or(false);
                if matches {
                    obj.set_size(size);
                    // Remove and re-insert to force the column to rebind
                    self.model.remove(i);
                    self.model.insert(i, &obj);
                    return;
                }
            }
        }
    }
}
