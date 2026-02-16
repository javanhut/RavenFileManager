use std::cell::RefCell;
use std::path::Path;
use std::rc::Rc;

use gtk4 as gtk;
use gtk::prelude::*;
use gtk::glib;
use gtk::subclass::prelude::*;

use raven_core::commands::AppCommand;
use raven_core::config::ViewMode;
use raven_core::entry::{EntryKind, FileEntry};
use raven_core::path::RavenPath;

use crate::state::AppState;

// GObject wrapper for FileEntry to use with ColumnView/GridView
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

/// File list supporting List (ColumnView), Icons (GridView), and Previews (GridView) modes.
pub struct FileListView {
    pub widget: gtk::Stack,
    pub column_view: gtk::ColumnView,
    pub icon_grid_view: gtk::GridView,
    pub preview_grid_view: gtk::GridView,
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

        // === List mode: ColumnView ===
        let column_view = Self::build_column_view(
            &selection, &state, &command_tx, pane_id,
        );

        // === Icon mode: GridView with 64px icons ===
        let icon_grid_view = Self::build_icon_grid_view(
            &selection, &state, &command_tx, pane_id,
        );

        // === Preview mode: GridView with 128px thumbnails ===
        let preview_grid_view = Self::build_preview_grid_view(
            &selection, &state, &command_tx, pane_id,
        );

        // === Stack assembly ===
        let stack = gtk::Stack::new();
        stack.set_transition_type(gtk::StackTransitionType::Crossfade);
        stack.set_hexpand(true);
        stack.set_vexpand(true);

        let list_scroll = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Automatic)
            .vscrollbar_policy(gtk::PolicyType::Automatic)
            .child(&column_view)
            .build();
        stack.add_named(&list_scroll, Some("list"));

        let icon_scroll = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vscrollbar_policy(gtk::PolicyType::Automatic)
            .child(&icon_grid_view)
            .build();
        stack.add_named(&icon_scroll, Some("icons"));

        let preview_scroll = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vscrollbar_policy(gtk::PolicyType::Automatic)
            .child(&preview_grid_view)
            .build();
        stack.add_named(&preview_scroll, Some("previews"));

        // Set initial view mode from state
        {
            let s = state.borrow();
            match s.view_mode {
                ViewMode::List => stack.set_visible_child_name("list"),
                ViewMode::Icons => stack.set_visible_child_name("icons"),
                ViewMode::Previews => stack.set_visible_child_name("previews"),
            }
        }

        Self {
            widget: stack,
            column_view,
            icon_grid_view,
            preview_grid_view,
            model,
            selection,
        }
    }

    /// Switch between list/icon/preview view modes.
    pub fn set_view_mode(&self, mode: ViewMode) {
        match mode {
            ViewMode::List => self.widget.set_visible_child_name("list"),
            ViewMode::Icons => self.widget.set_visible_child_name("icons"),
            ViewMode::Previews => self.widget.set_visible_child_name("previews"),
        }
    }

    fn build_column_view(
        selection: &gtk::SingleSelection,
        state: &AppState,
        command_tx: &tokio::sync::mpsc::UnboundedSender<AppCommand>,
        pane_id: u32,
    ) -> gtk::ColumnView {
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

            // DragSource for each row
            let uri_cell: Rc<RefCell<String>> = Rc::new(RefCell::new(String::new()));
            let drag_source = gtk::DragSource::new();
            drag_source.set_actions(gtk::gdk::DragAction::COPY | gtk::gdk::DragAction::MOVE);
            let uri_for_prepare = uri_cell.clone();
            drag_source.connect_prepare(move |_source, _x, _y| {
                let uri = uri_for_prepare.borrow().clone();
                if uri.is_empty() {
                    return None;
                }
                Some(gtk::gdk::ContentProvider::for_value(&uri.to_value()))
            });
            hbox.add_controller(drag_source);

            unsafe {
                hbox.set_data("drag-uri-cell", uri_cell);
            }

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

            if let Some(entry) = entry_obj.entry() {
                let uri_cell: Option<std::ptr::NonNull<Rc<RefCell<String>>>> =
                    unsafe { hbox.data::<Rc<RefCell<String>>>("drag-uri-cell") };
                if let Some(ptr) = uri_cell {
                    let cell = unsafe { ptr.as_ref() };
                    if let Some(local) = entry.path.as_local_path() {
                        *cell.borrow_mut() = format!("file://{}\r\n", local.display());
                    } else {
                        *cell.borrow_mut() = String::new();
                    }
                }

                if entry.is_dir() {
                    if let Some(local) = entry.path.as_local_path() {
                        let tooltip = build_dir_preview_tooltip(local);
                        hbox.set_tooltip_text(Some(&tooltip));
                    }
                } else {
                    hbox.set_tooltip_text(None);
                }
            }
        });
        name_factory.connect_unbind(|_, item| {
            let item = item.downcast_ref::<gtk::ListItem>().unwrap();
            if let Some(hbox) = item.child().and_downcast::<gtk::Box>() {
                hbox.set_tooltip_text(None);
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
            activate_entry(&sel_model, pos, &state_for_activate, &cmd_tx, pane_id);
        });

        // Drop target on column view
        {
            let drop_target = gtk::DropTarget::new(glib::types::Type::STRING, gtk::gdk::DragAction::COPY | gtk::gdk::DragAction::MOVE);
            let cmd_tx = command_tx.clone();
            let state_for_drop = state.clone();
            drop_target.connect_drop(move |_target, value, _x, _y| {
                handle_file_drop(value, &state_for_drop, &cmd_tx)
            });
            column_view.add_controller(drop_target);
        }

        column_view
    }

    fn build_icon_grid_view(
        selection: &gtk::SingleSelection,
        state: &AppState,
        command_tx: &tokio::sync::mpsc::UnboundedSender<AppCommand>,
        pane_id: u32,
    ) -> gtk::GridView {
        let factory = gtk::SignalListItemFactory::new();

        factory.connect_setup(|_, item| {
            let item = item.downcast_ref::<gtk::ListItem>().unwrap();
            let vbox = gtk::Box::new(gtk::Orientation::Vertical, 4);
            vbox.set_halign(gtk::Align::Center);
            vbox.set_valign(gtk::Align::Start);
            vbox.set_margin_top(8);
            vbox.set_margin_bottom(8);
            vbox.set_margin_start(4);
            vbox.set_margin_end(4);

            let icon = gtk::Image::new();
            icon.set_pixel_size(64);
            vbox.append(&icon);

            let label = gtk::Label::new(None);
            label.set_max_width_chars(12);
            label.set_ellipsize(gtk::pango::EllipsizeMode::End);
            label.set_wrap(true);
            label.set_wrap_mode(gtk::pango::WrapMode::WordChar);
            label.set_lines(2);
            label.set_halign(gtk::Align::Center);
            vbox.append(&label);

            // DragSource
            let uri_cell: Rc<RefCell<String>> = Rc::new(RefCell::new(String::new()));
            let drag_source = gtk::DragSource::new();
            drag_source.set_actions(gtk::gdk::DragAction::COPY | gtk::gdk::DragAction::MOVE);
            let uri_for_prepare = uri_cell.clone();
            drag_source.connect_prepare(move |_source, _x, _y| {
                let uri = uri_for_prepare.borrow().clone();
                if uri.is_empty() { return None; }
                Some(gtk::gdk::ContentProvider::for_value(&uri.to_value()))
            });
            vbox.add_controller(drag_source);
            unsafe { vbox.set_data("drag-uri-cell", uri_cell); }

            item.set_child(Some(&vbox));
        });

        factory.connect_bind(|_, item| {
            let item = item.downcast_ref::<gtk::ListItem>().unwrap();
            let entry_obj = item.item().and_downcast::<FileEntryObject>().unwrap();
            let vbox = item.child().and_downcast::<gtk::Box>().unwrap();
            let icon = vbox.first_child().and_downcast::<gtk::Image>().unwrap();
            let label = icon.next_sibling().and_downcast::<gtk::Label>().unwrap();

            icon.set_icon_name(Some(&entry_obj.icon_name()));
            label.set_text(&entry_obj.name());
            if entry_obj.is_hidden() {
                vbox.set_opacity(0.5);
            } else {
                vbox.set_opacity(1.0);
            }

            // Update drag URI
            if let Some(entry) = entry_obj.entry() {
                let uri_cell: Option<std::ptr::NonNull<Rc<RefCell<String>>>> =
                    unsafe { vbox.data::<Rc<RefCell<String>>>("drag-uri-cell") };
                if let Some(ptr) = uri_cell {
                    let cell = unsafe { ptr.as_ref() };
                    if let Some(local) = entry.path.as_local_path() {
                        *cell.borrow_mut() = format!("file://{}\r\n", local.display());
                    } else {
                        *cell.borrow_mut() = String::new();
                    }
                }
            }
        });

        factory.connect_unbind(|_, item| {
            let item = item.downcast_ref::<gtk::ListItem>().unwrap();
            if let Some(vbox) = item.child().and_downcast::<gtk::Box>() {
                vbox.set_opacity(1.0);
            }
        });

        let grid_view = gtk::GridView::new(Some(selection.clone()), Some(factory));
        grid_view.set_max_columns(10);
        grid_view.set_min_columns(2);

        // Activation (double-click)
        let cmd_tx = command_tx.clone();
        let sel = selection.clone();
        let state_for_activate = state.clone();
        grid_view.connect_activate(move |_, pos| {
            activate_entry(&sel, pos, &state_for_activate, &cmd_tx, pane_id);
        });

        // Drop target
        {
            let drop_target = gtk::DropTarget::new(glib::types::Type::STRING, gtk::gdk::DragAction::COPY | gtk::gdk::DragAction::MOVE);
            let cmd_tx = command_tx.clone();
            let state_for_drop = state.clone();
            drop_target.connect_drop(move |_target, value, _x, _y| {
                handle_file_drop(value, &state_for_drop, &cmd_tx)
            });
            grid_view.add_controller(drop_target);
        }

        grid_view
    }

    fn build_preview_grid_view(
        selection: &gtk::SingleSelection,
        state: &AppState,
        command_tx: &tokio::sync::mpsc::UnboundedSender<AppCommand>,
        pane_id: u32,
    ) -> gtk::GridView {
        let factory = gtk::SignalListItemFactory::new();

        factory.connect_setup(|_, item| {
            let item = item.downcast_ref::<gtk::ListItem>().unwrap();
            let vbox = gtk::Box::new(gtk::Orientation::Vertical, 4);
            vbox.set_halign(gtk::Align::Center);
            vbox.set_valign(gtk::Align::Start);
            vbox.set_margin_top(8);
            vbox.set_margin_bottom(8);
            vbox.set_margin_start(4);
            vbox.set_margin_end(4);
            vbox.set_width_request(140);

            // Thumbnail picture (for images)
            let picture = gtk::Picture::new();
            picture.set_can_shrink(true);
            picture.set_width_request(128);
            picture.set_height_request(128);
            vbox.append(&picture);

            // Fallback icon (for non-images)
            let icon_fallback = gtk::Image::new();
            icon_fallback.set_pixel_size(128);
            icon_fallback.set_visible(false);
            vbox.append(&icon_fallback);

            let label = gtk::Label::new(None);
            label.set_max_width_chars(14);
            label.set_ellipsize(gtk::pango::EllipsizeMode::End);
            label.set_wrap(true);
            label.set_wrap_mode(gtk::pango::WrapMode::WordChar);
            label.set_lines(2);
            label.set_halign(gtk::Align::Center);
            vbox.append(&label);

            // DragSource
            let uri_cell: Rc<RefCell<String>> = Rc::new(RefCell::new(String::new()));
            let drag_source = gtk::DragSource::new();
            drag_source.set_actions(gtk::gdk::DragAction::COPY | gtk::gdk::DragAction::MOVE);
            let uri_for_prepare = uri_cell.clone();
            drag_source.connect_prepare(move |_source, _x, _y| {
                let uri = uri_for_prepare.borrow().clone();
                if uri.is_empty() { return None; }
                Some(gtk::gdk::ContentProvider::for_value(&uri.to_value()))
            });
            vbox.add_controller(drag_source);
            unsafe { vbox.set_data("drag-uri-cell", uri_cell); }

            item.set_child(Some(&vbox));
        });

        factory.connect_bind(|_, item| {
            let item = item.downcast_ref::<gtk::ListItem>().unwrap();
            let entry_obj = item.item().and_downcast::<FileEntryObject>().unwrap();
            let vbox = item.child().and_downcast::<gtk::Box>().unwrap();

            let picture = vbox.first_child().and_downcast::<gtk::Picture>().unwrap();
            let icon_fallback = picture.next_sibling().and_downcast::<gtk::Image>().unwrap();
            let label = icon_fallback.next_sibling().and_downcast::<gtk::Label>().unwrap();

            label.set_text(&entry_obj.name());

            if entry_obj.is_hidden() {
                vbox.set_opacity(0.5);
            } else {
                vbox.set_opacity(1.0);
            }

            // Determine if this is an image file
            let is_image = entry_obj
                .entry()
                .and_then(|e| e.extension().map(|s| s.to_lowercase()))
                .map(|ext| matches!(
                    ext.as_str(),
                    "png" | "jpg" | "jpeg" | "gif" | "svg" | "webp" | "bmp"
                ))
                .unwrap_or(false);

            if is_image {
                if let Some(entry) = entry_obj.entry() {
                    if let Some(local) = entry.path.as_local_path() {
                        picture.set_filename(Some(local));
                        picture.set_visible(true);
                        icon_fallback.set_visible(false);
                    } else {
                        picture.set_visible(false);
                        icon_fallback.set_icon_name(Some(&entry_obj.icon_name()));
                        icon_fallback.set_visible(true);
                    }
                }
            } else {
                picture.set_visible(false);
                picture.set_filename(None::<&std::path::Path>);
                icon_fallback.set_icon_name(Some(&entry_obj.icon_name()));
                icon_fallback.set_visible(true);
            }

            // Update drag URI
            if let Some(entry) = entry_obj.entry() {
                let uri_cell: Option<std::ptr::NonNull<Rc<RefCell<String>>>> =
                    unsafe { vbox.data::<Rc<RefCell<String>>>("drag-uri-cell") };
                if let Some(ptr) = uri_cell {
                    let cell = unsafe { ptr.as_ref() };
                    if let Some(local) = entry.path.as_local_path() {
                        *cell.borrow_mut() = format!("file://{}\r\n", local.display());
                    } else {
                        *cell.borrow_mut() = String::new();
                    }
                }
            }
        });

        factory.connect_unbind(|_, item| {
            let item = item.downcast_ref::<gtk::ListItem>().unwrap();
            if let Some(vbox) = item.child().and_downcast::<gtk::Box>() {
                vbox.set_opacity(1.0);
                // Clear the picture to free memory
                if let Some(picture) = vbox.first_child().and_downcast::<gtk::Picture>() {
                    picture.set_filename(None::<&std::path::Path>);
                }
            }
        });

        let grid_view = gtk::GridView::new(Some(selection.clone()), Some(factory));
        grid_view.set_max_columns(8);
        grid_view.set_min_columns(2);

        // Activation (double-click)
        let cmd_tx = command_tx.clone();
        let sel = selection.clone();
        let state_for_activate = state.clone();
        grid_view.connect_activate(move |_, pos| {
            activate_entry(&sel, pos, &state_for_activate, &cmd_tx, pane_id);
        });

        // Drop target
        {
            let drop_target = gtk::DropTarget::new(glib::types::Type::STRING, gtk::gdk::DragAction::COPY | gtk::gdk::DragAction::MOVE);
            let cmd_tx = command_tx.clone();
            let state_for_drop = state.clone();
            drop_target.connect_drop(move |_target, value, _x, _y| {
                handle_file_drop(value, &state_for_drop, &cmd_tx)
            });
            grid_view.add_controller(drop_target);
        }

        grid_view
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
                    // Remove and re-insert to force rebind
                    self.model.remove(i);
                    self.model.insert(i, &obj);
                    return;
                }
            }
        }
    }
}

/// Shared activation handler for all view modes (double-click to navigate/open).
fn activate_entry(
    selection: &gtk::SingleSelection,
    pos: u32,
    state: &AppState,
    cmd_tx: &tokio::sync::mpsc::UnboundedSender<AppCommand>,
    pane_id: u32,
) {
    if let Some(item) = selection.item(pos) {
        if let Some(entry_obj) = item.downcast_ref::<FileEntryObject>() {
            if let Some(entry) = entry_obj.entry() {
                if entry.is_dir() {
                    {
                        let mut s = state.borrow_mut();
                        if let Some(pane) = s.pane_by_id_mut(pane_id) {
                            pane.navigate_to(entry.path.clone());
                        }
                    }
                    let _ = cmd_tx.send(AppCommand::Navigate {
                        path: entry.path.clone(),
                        pane_id,
                    });
                } else {
                    let config = state.borrow().config.clone();
                    crate::file_opener::open_file(&entry.path, &config);
                }
            }
        }
    }
}

/// Shared file drop handler for all view modes.
fn handle_file_drop(
    value: &glib::Value,
    state: &AppState,
    cmd_tx: &tokio::sync::mpsc::UnboundedSender<AppCommand>,
) -> bool {
    if let Ok(uri_list) = value.get::<String>() {
        let sources: Vec<RavenPath> = uri_list
            .lines()
            .filter(|line| !line.is_empty() && !line.starts_with('#'))
            .filter_map(|line| {
                let line = line.trim().trim_end_matches('\r');
                line.strip_prefix("file://")
                    .map(|p| RavenPath::local(std::path::PathBuf::from(p)))
            })
            .collect();

        if sources.is_empty() {
            return false;
        }

        let destination = {
            let s = state.borrow();
            s.active_tab().active_pane().current_path.clone()
        };

        let _ = cmd_tx.send(AppCommand::MoveFiles {
            sources,
            destination,
        });
        return true;
    }
    false
}

/// Build a plain-text tooltip showing directory contents preview.
fn build_dir_preview_tooltip(path: &Path) -> String {
    let max_entries = 12;

    let entries = match std::fs::read_dir(path) {
        Ok(rd) => {
            let mut items: Vec<(String, bool)> = Vec::new();
            for entry in rd.flatten() {
                let name: String = entry
                    .file_name()
                    .to_string_lossy()
                    .chars()
                    .filter(|c| *c != '\0')
                    .collect();
                let is_dir = entry.file_type().map(|ft| ft.is_dir()).unwrap_or(false);
                items.push((name, is_dir));
            }
            items
        }
        Err(_) => return "(cannot read directory)".to_string(),
    };

    if entries.is_empty() {
        return "(empty directory)".to_string();
    }

    let mut sorted = entries;
    sorted.sort_by(|a, b| {
        b.1.cmp(&a.1)
            .then_with(|| a.0.to_lowercase().cmp(&b.0.to_lowercase()))
    });

    let total = sorted.len();
    let show = sorted.into_iter().take(max_entries);
    let mut lines: Vec<String> = Vec::new();

    for (name, is_dir) in show {
        if is_dir {
            lines.push(format!("{}/", name));
        } else {
            lines.push(name);
        }
    }

    if total > max_entries {
        lines.push(format!("...and {} more", total - max_entries));
    }

    lines.join("\n")
}
