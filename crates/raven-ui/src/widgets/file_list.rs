use std::cell::RefCell;
use std::collections::HashMap;
use std::path::Path;
use std::rc::Rc;

use gtk::glib;
use gtk::prelude::*;
use gtk::subclass::prelude::*;
use gtk4 as gtk;

use raven_core::commands::AppCommand;
use raven_core::config::ViewMode;
use raven_core::entry::{EntryKind, FileEntry};
use raven_core::events::GitFileStatus;
use raven_core::path::RavenPath;

use crate::state::{AppState, PaneResolver};
use crate::thumbnails::{self, Slot};

// GObject wrapper for FileEntry to use with ColumnView/GridView
mod imp {
    use gtk4::glib;
    use gtk4::glib::Properties;
    use gtk4::prelude::*;
    use gtk4::subclass::prelude::*;
    use raven_core::entry::FileEntry;
    use std::cell::RefCell;

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
        /// One-letter version-control mark, empty when clean or unknown.
        #[property(get, set)]
        vcs_status: RefCell<String>,
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
            .map(|dt| format_local_time(&dt))
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

    /// Show `status` in the version-control column; `None` clears it.
    pub fn set_vcs(&self, status: Option<GitFileStatus>) {
        let code = status.map(|s| vcs_mark(s).0).unwrap_or("");
        if self.vcs_status() != code {
            self.set_vcs_status(code);
        }
    }
}

/// How a status is shown: its mark, the CSS class colouring it, and the
/// tooltip spelling it out.
pub fn vcs_mark(status: GitFileStatus) -> (&'static str, &'static str, &'static str) {
    match status {
        GitFileStatus::Modified => ("M", "git-modified", "Modified"),
        GitFileStatus::Added => ("A", "git-added", "Added"),
        GitFileStatus::Deleted => ("D", "git-deleted", "Deleted"),
        GitFileStatus::Renamed => ("R", "git-modified", "Renamed"),
        GitFileStatus::Untracked => ("?", "git-untracked", "Untracked"),
        GitFileStatus::Ignored => ("!", "dim-label", "Ignored"),
        GitFileStatus::Conflict => ("U", "git-conflict", "Conflict"),
        GitFileStatus::Clean => ("", "", ""),
    }
}

/// The reverse of [`vcs_mark`]: the status a mark stands for.
fn status_for_mark(mark: &str) -> Option<GitFileStatus> {
    [
        GitFileStatus::Modified,
        GitFileStatus::Added,
        GitFileStatus::Deleted,
        GitFileStatus::Renamed,
        GitFileStatus::Untracked,
        GitFileStatus::Ignored,
        GitFileStatus::Conflict,
    ]
    .into_iter()
    .find(|s| vcs_mark(*s).0 == mark)
}

const VCS_CLASSES: [&str; 6] = [
    "git-modified",
    "git-added",
    "git-deleted",
    "git-untracked",
    "git-conflict",
    "dim-label",
];

/// Paint a version-control mark into its column label.
fn render_vcs_mark(label: &gtk::Label, mark: &str) {
    for class in VCS_CLASSES {
        label.remove_css_class(class);
    }
    match status_for_mark(mark) {
        Some(status) => {
            let (code, class, tooltip) = vcs_mark(status);
            label.set_text(code);
            label.add_css_class(class);
            label.set_tooltip_text(Some(tooltip));
        }
        None => {
            label.set_text("");
            label.set_tooltip_text(None);
        }
    }
}

/// Every timestamp the app shows goes through here: local time, one pattern.
/// Entries carry UTC, and formatting that directly put the list hours away
/// from the preview panel and the portal picker.
pub fn format_local_time(dt: &chrono::DateTime<chrono::Utc>) -> String {
    dt.with_timezone(&chrono::Local).format("%Y-%m-%d %H:%M").to_string()
}

/// The full-colour icon for one of [`icon_for_entry`]'s symbolic names, with
/// the symbolic one as the fallback. The grids draw icons at thumbnail size,
/// where a flat glyph reads as a placeholder next to real thumbnails; the list
/// and sidebar keep the symbolic set at text size.
pub fn full_colour_icon(symbolic: &str) -> gtk::gio::ThemedIcon {
    match symbolic.strip_suffix("-symbolic") {
        Some(base) => gtk::gio::ThemedIcon::from_names(&[base, symbolic]),
        None => gtk::gio::ThemedIcon::new(symbolic),
    }
}

/// Align a column's header title with its right-aligned values.
fn align_header_end(column_view: &gtk::ColumnView, title: &str) {
    let Some(header) = column_view.first_child() else {
        return;
    };
    let mut child = header.first_child();
    while let Some(button) = child {
        child = button.next_sibling();
        let Some(content) = button.first_child() else {
            continue;
        };
        let mut part = content.first_child();
        while let Some(widget) = part {
            part = widget.next_sibling();
            if widget.downcast_ref::<gtk::Label>().is_some_and(|l| l.label() == title) {
                content.set_halign(gtk::Align::End);
                return;
            }
        }
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
    /// Every entry of the listing. The views show it through `filtered`, so
    /// positions in `selection` are positions in the filtered list, not here.
    pub model: gio::ListStore,
    /// `model` narrowed by the pane's quick filter.
    pub filtered: gtk::FilterListModel,
    pub selection: gtk::MultiSelection,
    quick_filter: gtk::CustomFilter,
    /// The quick filter's text, lowercased; empty shows everything.
    quick_query: Rc<RefCell<String>>,
}

/// Whether a file named `name` survives the quick filter `query`: a
/// case-insensitive substring match, with an empty query matching everything.
pub fn quick_filter_matches(name: &str, query: &str) -> bool {
    query.is_empty() || name.to_lowercase().contains(&query.to_lowercase())
}

impl FileListView {
    pub fn new(
        state: AppState,
        command_tx: tokio::sync::mpsc::UnboundedSender<AppCommand>,
        pane: PaneResolver,
    ) -> Self {
        let model = gio::ListStore::new::<FileEntryObject>();
        let quick_query: Rc<RefCell<String>> = Rc::new(RefCell::new(String::new()));
        let quick_filter = {
            let query = quick_query.clone();
            gtk::CustomFilter::new(move |obj| {
                let query = query.borrow();
                query.is_empty()
                    || obj
                        .downcast_ref::<FileEntryObject>()
                        .is_some_and(|o| quick_filter_matches(&o.name(), &query))
            })
        };
        let filtered = gtk::FilterListModel::new(Some(model.clone()), Some(quick_filter.clone()));
        let selection = gtk::MultiSelection::new(Some(filtered.clone()));

        // === List mode: ColumnView ===
        let column_view = Self::build_column_view(&selection, &state, &command_tx, &pane);

        // === Icon mode: GridView with 64px icons ===
        let icon_grid_view = Self::build_icon_grid_view(&selection, &state, &command_tx, &pane);

        // === Preview mode: GridView with 128px thumbnails ===
        let preview_grid_view =
            Self::build_preview_grid_view(&selection, &state, &command_tx, &pane);

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
        // Inset from the pane edges so the selected row reads as a rounded
        // pill, like the sidebar's. On the scroller rather than the rows,
        // which must stay exactly as wide as the header's columns.
        list_scroll.set_margin_start(6);
        list_scroll.set_margin_end(6);
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
            filtered,
            selection,
            quick_filter,
            quick_query,
        }
    }

    /// Narrow the listing to names containing `query`, ignoring case. An
    /// empty query shows everything again.
    pub fn set_quick_filter(&self, query: &str) {
        let new = query.to_lowercase();
        let change = {
            let old = self.quick_query.borrow();
            if *old == new {
                return;
            }
            // Telling the model which way the filter moved spares it
            // re-checking rows the change cannot affect.
            if new.contains(old.as_str()) {
                gtk::FilterChange::MoreStrict
            } else if old.contains(new.as_str()) {
                gtk::FilterChange::LessStrict
            } else {
                gtk::FilterChange::Different
            }
        };
        *self.quick_query.borrow_mut() = new;
        self.quick_filter.changed(change);
    }

    /// Whether the full listing, ignoring the quick filter, holds any of
    /// `paths`. Tells a reveal the filter hid from one still on its way.
    pub fn holds_any(&self, paths: &[RavenPath]) -> bool {
        (0..self.model.n_items()).any(|i| {
            self.model
                .item(i)
                .and_then(|o| o.downcast::<FileEntryObject>().ok())
                .and_then(|obj| obj.entry())
                .is_some_and(|entry| paths.iter().any(|p| p == &entry.path))
        })
    }

    /// Whether the quick filter is narrowing the listing.
    pub fn is_quick_filtered(&self) -> bool {
        !self.quick_query.borrow().is_empty()
    }

    /// Give the keyboard to whichever of the three views is on screen.
    pub fn focus_view(&self) {
        match self.widget.visible_child_name().as_deref() {
            Some("icons") => self.icon_grid_view.grab_focus(),
            Some("previews") => self.preview_grid_view.grab_focus(),
            _ => self.column_view.grab_focus(),
        };
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
        selection: &gtk::MultiSelection,
        state: &AppState,
        command_tx: &tokio::sync::mpsc::UnboundedSender<AppCommand>,
        pane: &PaneResolver,
    ) -> gtk::ColumnView {
        let column_view = gtk::ColumnView::new(Some(selection.clone()));
        // Columns are told apart by alignment and air, not rules.
        column_view.set_show_column_separators(false);
        column_view.set_show_row_separators(false);
        column_view.set_enable_rubberband(true);
        column_view.add_css_class("data-table");

        // Name column (icon + name)
        let name_factory = gtk::SignalListItemFactory::new();
        let drag_selection = selection.clone();
        name_factory.connect_setup(move |_, item| {
            let item = item.downcast_ref::<gtk::ListItem>().unwrap();
            let hbox = gtk::Box::new(gtk::Orientation::Horizontal, 8);
            let icon = gtk::Image::new();
            icon.add_css_class("raven-list-icon");
            // Rounds a thumbnail's corners once one replaces the type icon.
            icon.add_css_class("thumb");
            icon.set_overflow(gtk::Overflow::Hidden);
            let label = gtk::Label::new(None);
            label.set_halign(gtk::Align::Start);
            label.set_ellipsize(gtk::pango::EllipsizeMode::End);
            // A narrow pane scrolls sideways rather than squeezing every
            // name down to an ellipsis.
            label.set_width_chars(14);
            label.set_xalign(0.0);
            hbox.append(&icon);
            hbox.append(&label);

            attach_item_drag(&hbox, &drag_selection);

            item.set_child(Some(&hbox));
        });
        name_factory.connect_bind(|_, item| {
            let item = item.downcast_ref::<gtk::ListItem>().unwrap();
            let entry_obj = item.item().and_downcast::<FileEntryObject>().unwrap();
            let hbox = item.child().and_downcast::<gtk::Box>().unwrap();

            let pos = item.position();
            unsafe {
                if let Some(ptr) = hbox.data::<Rc<RefCell<u32>>>("item-pos") {
                    *ptr.as_ref().borrow_mut() = pos;
                }
            }
            let icon = hbox.first_child().and_downcast::<gtk::Image>().unwrap();
            let label = icon.next_sibling().and_downcast::<gtk::Label>().unwrap();
            icon.set_icon_name(Some(&entry_obj.icon_name()));
            show_thumbnail(&hbox, &icon, &entry_obj, thumbnails::LIST_SIZE);
            label.set_text(&entry_obj.name());
            // Dimmed by colour, which a selected row can turn white again.
            if entry_obj.is_hidden() {
                label.add_css_class("hidden-file");
            } else {
                label.remove_css_class("hidden-file");
            }

            if let Some(entry) = entry_obj.entry() {
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
                thumb_slot(&hbox).clear();
                hbox.set_tooltip_text(None);
            }
        });
        let name_col = gtk::ColumnViewColumn::new(Some("Name"), Some(name_factory));
        name_col.set_expand(true);
        name_col.set_resizable(true);
        column_view.append_column(&name_col);

        // Version-control column. Status arrives after the listing, so the
        // label follows the property rather than reading it once at bind.
        let vcs_factory = gtk::SignalListItemFactory::new();
        vcs_factory.connect_setup(|_, item| {
            let item = item.downcast_ref::<gtk::ListItem>().unwrap();
            let label = gtk::Label::new(None);
            label.set_halign(gtk::Align::Center);
            label.add_css_class("monospace");
            item.set_child(Some(&label));
        });
        vcs_factory.connect_bind(|_, item| {
            let item = item.downcast_ref::<gtk::ListItem>().unwrap();
            let entry_obj = item.item().and_downcast::<FileEntryObject>().unwrap();
            let label = item.child().and_downcast::<gtk::Label>().unwrap();
            render_vcs_mark(&label, &entry_obj.vcs_status());
            let label_for_notify = label.clone();
            let handler = entry_obj.connect_vcs_status_notify(move |obj| {
                render_vcs_mark(&label_for_notify, &obj.vcs_status());
            });
            unsafe {
                label.set_data("vcs-notify-handler", handler);
            }
        });
        vcs_factory.connect_unbind(|_, item| {
            let item = item.downcast_ref::<gtk::ListItem>().unwrap();
            let Some(entry_obj) = item.item().and_downcast::<FileEntryObject>() else {
                return;
            };
            let Some(label) = item.child().and_downcast::<gtk::Label>() else {
                return;
            };
            let handler: Option<glib::SignalHandlerId> =
                unsafe { label.steal_data("vcs-notify-handler") };
            if let Some(handler) = handler {
                entry_obj.disconnect(handler);
            }
        });
        let vcs_col = gtk::ColumnViewColumn::new(Some("VCS"), Some(vcs_factory));
        vcs_col.set_fixed_width(56);
        vcs_col.set_resizable(true);
        column_view.append_column(&vcs_col);

        // Size column
        let size_factory = gtk::SignalListItemFactory::new();
        size_factory.connect_setup(|_, item| {
            let item = item.downcast_ref::<gtk::ListItem>().unwrap();
            let label = gtk::Label::new(None);
            label.set_halign(gtk::Align::End);
            label.add_css_class("secondary");
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
        align_header_end(&column_view, "Size");

        // Modified column
        let mod_factory = gtk::SignalListItemFactory::new();
        mod_factory.connect_setup(|_, item| {
            let item = item.downcast_ref::<gtk::ListItem>().unwrap();
            let label = gtk::Label::new(None);
            label.set_halign(gtk::Align::Start);
            label.add_css_class("secondary");
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
            label.add_css_class("secondary");
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
        let pane_for_activate = pane.clone();
        column_view.connect_activate(move |view, pos| {
            activate_entry(view.upcast_ref(), &sel_model, pos, &state_for_activate, &cmd_tx, pane_for_activate());
        });

        // Drop target on column view
        {
            let drop_target = {
                let state = state.clone();
                let pane = pane.clone();
                crate::dnd::file_drop_target(None, move || Some(drop_destination(&state, pane())))
            };
            let cmd_tx = command_tx.clone();
            let state_for_drop = state.clone();
            let pane_for_drop = pane.clone();
            drop_target.connect_drop(move |target, value, _x, _y| {
                handle_file_drop(target, value, &state_for_drop, &cmd_tx, pane_for_drop())
            });
            column_view.add_controller(drop_target);
        }

        column_view
    }

    fn build_icon_grid_view(
        selection: &gtk::MultiSelection,
        state: &AppState,
        command_tx: &tokio::sync::mpsc::UnboundedSender<AppCommand>,
        pane: &PaneResolver,
    ) -> gtk::GridView {
        let factory = gtk::SignalListItemFactory::new();

        let drag_selection = selection.clone();
        factory.connect_setup(move |_, item| {
            let item = item.downcast_ref::<gtk::ListItem>().unwrap();
            let vbox = gtk::Box::new(gtk::Orientation::Vertical, 4);
            vbox.set_halign(gtk::Align::Center);
            vbox.set_valign(gtk::Align::Start);
            vbox.set_margin_top(8);
            vbox.set_margin_bottom(8);
            vbox.set_margin_start(4);
            vbox.set_margin_end(4);

            let icon = gtk::Image::new();
            icon.add_css_class("raven-grid-icon");
            icon.add_css_class("thumb");
            icon.set_overflow(gtk::Overflow::Hidden);
            vbox.append(&icon);

            // Whole words only: two lines of a name broken mid-word
            // ("Devel-opment") read worse than one ellipsized word.
            let label = gtk::Label::new(None);
            label.set_max_width_chars(14);
            label.set_ellipsize(gtk::pango::EllipsizeMode::End);
            label.set_wrap(true);
            label.set_wrap_mode(gtk::pango::WrapMode::Word);
            label.set_lines(2);
            label.set_halign(gtk::Align::Center);
            vbox.append(&label);

            attach_item_drag(&vbox, &drag_selection);

            item.set_child(Some(&vbox));
        });

        factory.connect_bind(|_, item| {
            let item = item.downcast_ref::<gtk::ListItem>().unwrap();
            let entry_obj = item.item().and_downcast::<FileEntryObject>().unwrap();
            let vbox = item.child().and_downcast::<gtk::Box>().unwrap();

            let pos = item.position();
            unsafe {
                if let Some(ptr) = vbox.data::<Rc<RefCell<u32>>>("item-pos") {
                    *ptr.as_ref().borrow_mut() = pos;
                }
            }

            let icon = vbox.first_child().and_downcast::<gtk::Image>().unwrap();
            let label = icon.next_sibling().and_downcast::<gtk::Label>().unwrap();

            icon.set_from_gicon(&full_colour_icon(&entry_obj.icon_name()));
            show_thumbnail(&vbox, &icon, &entry_obj, thumbnails::ICON_SIZE);
            label.set_text(&entry_obj.name());
            mark_hidden(&vbox, &label, entry_obj.is_hidden());
        });

        factory.connect_unbind(|_, item| {
            let item = item.downcast_ref::<gtk::ListItem>().unwrap();
            if let Some(vbox) = item.child().and_downcast::<gtk::Box>() {
                thumb_slot(&vbox).clear();
            }
        });

        let grid_view = gtk::GridView::new(Some(selection.clone()), Some(factory));
        grid_view.set_max_columns(10);
        grid_view.add_css_class("file-grid");
        grid_view.set_min_columns(2);
        grid_view.set_enable_rubberband(true);

        // Activation (double-click)
        let cmd_tx = command_tx.clone();
        let sel = selection.clone();
        let state_for_activate = state.clone();
        let pane_for_activate = pane.clone();
        grid_view.connect_activate(move |view, pos| {
            activate_entry(view.upcast_ref(), &sel, pos, &state_for_activate, &cmd_tx, pane_for_activate());
        });

        // Drop target
        {
            let drop_target = {
                let state = state.clone();
                let pane = pane.clone();
                crate::dnd::file_drop_target(None, move || Some(drop_destination(&state, pane())))
            };
            let cmd_tx = command_tx.clone();
            let state_for_drop = state.clone();
            let pane_for_drop = pane.clone();
            drop_target.connect_drop(move |target, value, _x, _y| {
                handle_file_drop(target, value, &state_for_drop, &cmd_tx, pane_for_drop())
            });
            grid_view.add_controller(drop_target);
        }

        grid_view
    }

    fn build_preview_grid_view(
        selection: &gtk::MultiSelection,
        state: &AppState,
        command_tx: &tokio::sync::mpsc::UnboundedSender<AppCommand>,
        pane: &PaneResolver,
    ) -> gtk::GridView {
        let factory = gtk::SignalListItemFactory::new();

        let drag_selection = selection.clone();
        factory.connect_setup(move |_, item| {
            let item = item.downcast_ref::<gtk::ListItem>().unwrap();
            let vbox = gtk::Box::new(gtk::Orientation::Vertical, 4);
            vbox.set_halign(gtk::Align::Center);
            vbox.set_valign(gtk::Align::Start);
            vbox.set_margin_top(8);
            vbox.set_margin_bottom(8);
            vbox.set_margin_start(4);
            vbox.set_margin_end(4);
            vbox.set_width_request(140);

            // Thumbnail picture (for images). The frame fits the picture to
            // the thumbnail's own shape inside a 128px square, so the rounded
            // corners land on the image rather than on letterboxing around it.
            let picture = gtk::Picture::new();
            picture.set_can_shrink(true);
            picture.add_css_class("thumb");
            picture.set_overflow(gtk::Overflow::Hidden);
            let frame = gtk::AspectFrame::new(0.5, 0.5, 1.0, true);
            frame.set_size_request(128, 128);
            frame.set_child(Some(&picture));
            vbox.append(&frame);

            // Fallback icon (for non-images), centred in the same square a
            // thumbnail gets so every row of tiles keeps one height.
            let icon_fallback = gtk::Image::new();
            icon_fallback.add_css_class("raven-preview-icon");
            icon_fallback.set_size_request(128, 128);
            icon_fallback.set_visible(false);
            vbox.append(&icon_fallback);

            let label = gtk::Label::new(None);
            label.set_max_width_chars(14);
            label.set_ellipsize(gtk::pango::EllipsizeMode::End);
            label.set_wrap(true);
            label.set_wrap_mode(gtk::pango::WrapMode::Word);
            label.set_lines(2);
            label.set_halign(gtk::Align::Center);
            vbox.append(&label);

            attach_item_drag(&vbox, &drag_selection);

            item.set_child(Some(&vbox));
        });

        factory.connect_bind(|_, item| {
            let item = item.downcast_ref::<gtk::ListItem>().unwrap();
            let entry_obj = item.item().and_downcast::<FileEntryObject>().unwrap();
            let vbox = item.child().and_downcast::<gtk::Box>().unwrap();

            let pos = item.position();
            unsafe {
                if let Some(ptr) = vbox.data::<Rc<RefCell<u32>>>("item-pos") {
                    *ptr.as_ref().borrow_mut() = pos;
                }
            }

            let frame = vbox.first_child().and_downcast::<gtk::AspectFrame>().unwrap();
            let picture = frame.child().and_downcast::<gtk::Picture>().unwrap();
            let icon_fallback = frame.next_sibling().and_downcast::<gtk::Image>().unwrap();
            let label = icon_fallback
                .next_sibling()
                .and_downcast::<gtk::Label>()
                .unwrap();

            label.set_text(&entry_obj.name());
            mark_hidden(&vbox, &label, entry_obj.is_hidden());

            // The type icon until a thumbnail is ready, which for anything
            // already thumbnailed is before this returns.
            frame.set_visible(false);
            picture.set_filename(None::<&std::path::Path>);
            icon_fallback.set_from_gicon(&full_colour_icon(&entry_obj.icon_name()));
            icon_fallback.set_visible(true);

            let local = entry_obj
                .entry()
                .and_then(|e| e.path.as_local_path().map(|p| p.to_path_buf()));
            let Some(local) = local else {
                thumb_slot(&vbox).clear();
                return;
            };
            let is_svg = local
                .extension()
                .is_some_and(|ext| ext.eq_ignore_ascii_case("svg"));
            if is_svg {
                // Vector images need no thumbnail; GTK renders them at size.
                thumb_slot(&vbox).clear();
                picture.set_filename(Some(&local));
                frame.set_visible(true);
                icon_fallback.set_visible(false);
            } else {
                let weak_frame = frame.downgrade();
                let weak_picture = picture.downgrade();
                let weak_icon = icon_fallback.downgrade();
                thumb_slot(&vbox).request(&local, thumbnails::PREVIEW_SIZE, move |thumb| {
                    if let (Some(frame), Some(picture), Some(icon)) =
                        (weak_frame.upgrade(), weak_picture.upgrade(), weak_icon.upgrade())
                    {
                        picture.set_filename(Some(thumb));
                        frame.set_visible(true);
                        icon.set_visible(false);
                    }
                });
            }
        });

        factory.connect_unbind(|_, item| {
            let item = item.downcast_ref::<gtk::ListItem>().unwrap();
            if let Some(vbox) = item.child().and_downcast::<gtk::Box>() {
                // Clear the picture to free memory
                if let Some(picture) = vbox
                    .first_child()
                    .and_downcast::<gtk::AspectFrame>()
                    .and_then(|frame| frame.child())
                    .and_downcast::<gtk::Picture>()
                {
                    picture.set_filename(None::<&std::path::Path>);
                }
            }
        });

        let grid_view = gtk::GridView::new(Some(selection.clone()), Some(factory));
        grid_view.set_max_columns(8);
        grid_view.add_css_class("file-grid");
        grid_view.set_min_columns(2);
        grid_view.set_enable_rubberband(true);

        // Activation (double-click)
        let cmd_tx = command_tx.clone();
        let sel = selection.clone();
        let state_for_activate = state.clone();
        let pane_for_activate = pane.clone();
        grid_view.connect_activate(move |view, pos| {
            activate_entry(view.upcast_ref(), &sel, pos, &state_for_activate, &cmd_tx, pane_for_activate());
        });

        // Drop target
        {
            let drop_target = {
                let state = state.clone();
                let pane = pane.clone();
                crate::dnd::file_drop_target(None, move || Some(drop_destination(&state, pane())))
            };
            let cmd_tx = command_tx.clone();
            let state_for_drop = state.clone();
            let pane_for_drop = pane.clone();
            drop_target.connect_drop(move |target, value, _x, _y| {
                handle_file_drop(target, value, &state_for_drop, &cmd_tx, pane_for_drop())
            });
            grid_view.add_controller(drop_target);
        }

        grid_view
    }

    pub fn set_entries(
        &self,
        entries: &[FileEntry],
        show_hidden: bool,
        vcs_statuses: &HashMap<String, GitFileStatus>,
    ) {
        self.model.remove_all();
        for entry in entries {
            if !show_hidden && entry.is_hidden() {
                continue;
            }
            let obj = FileEntryObject::new(entry);
            obj.set_vcs(vcs_statuses.get(&entry.name).copied());
            self.model.append(&obj);
        }
    }

    /// Repaint the version-control column from `vcs_statuses`, by file name.
    /// Rows the map does not name are cleared.
    pub fn apply_vcs_statuses(&self, vcs_statuses: &HashMap<String, GitFileStatus>) {
        for i in 0..self.model.n_items() {
            let Some(obj) = self
                .model
                .item(i)
                .and_then(|o| o.downcast::<FileEntryObject>().ok())
            else {
                continue;
            };
            let status = vcs_statuses.get(obj.name().as_str()).copied();
            obj.set_vcs(status);
        }
    }

    /// Update a directory entry's displayed size after async calculation.
    pub fn update_dir_size(&self, path: &RavenPath, size: u64) {
        let n = self.model.n_items();
        for i in 0..n {
            if let Some(obj) = self
                .model
                .item(i)
                .and_then(|o| o.downcast::<FileEntryObject>().ok())
            {
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

    /// Select the rows matching `paths` and bring the first of them into view.
    ///
    /// Returns the position of the first match, or `None` when the listing
    /// holds none of them. `None` is a normal outcome rather than a fault: a
    /// revealed file that happens to be hidden is not in the model at all while
    /// hidden files are being filtered out.
    ///
    /// Positions are the selection's, so a row the quick filter hides is not
    /// matched either.
    pub fn select_paths(&self, paths: &[RavenPath]) -> Option<u32> {
        let mut first: Option<u32> = None;

        for i in 0..self.selection.n_items() {
            let Some(obj) = self
                .selection
                .item(i)
                .and_then(|o| o.downcast::<FileEntryObject>().ok())
            else {
                continue;
            };
            let Some(entry) = obj.entry() else {
                continue;
            };
            if !paths.iter().any(|p| p == &entry.path) {
                continue;
            }

            // The first match clears whatever the pane had selected before; the
            // rest add to it, so a multi-file reveal ends up holding exactly
            // the items that were asked for.
            self.selection.select_item(i, first.is_none());
            if first.is_none() {
                first = Some(i);
            }
        }

        if let Some(pos) = first {
            self.scroll_to_position(pos);
        }
        first
    }

    /// Scroll whichever of the three views is on screen so `pos` is visible.
    fn scroll_to_position(&self, pos: u32) {
        // FOCUS rather than SELECT: the selection is set by the caller, and
        // SELECT here would collapse a multi-file reveal down to one row.
        let flags = gtk::ListScrollFlags::FOCUS;
        match self.widget.visible_child_name().as_deref() {
            Some("icons") => self.icon_grid_view.scroll_to(pos, flags, None),
            Some("previews") => self.preview_grid_view.scroll_to(pos, flags, None),
            _ => self.column_view.scroll_to(pos, None, flags, None),
        }
    }
}

/// Shared activation handler for all view modes (double-click to navigate/open).
fn activate_entry(
    view: &gtk::Widget,
    selection: &gtk::MultiSelection,
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
                    // When the activated row is the whole selection (the usual
                    // double click), go through the window's `file.open` so a
                    // failure is shown in the status bar instead of only logged.
                    let only_this = selection.is_selected(pos)
                        && selection.selection().size() == 1;
                    if !(only_this && view.activate_action("file.open", None).is_ok()) {
                        let config = state.borrow().config.clone();
                        crate::file_opener::open_file(&entry.path, &config);
                    }
                }
            }
        }
    }
}

/// Shared file drop handler for all view modes.
/// The item's thumbnail slot, created on first use.
fn thumb_slot(item: &gtk::Box) -> Slot {
    unsafe {
        if let Some(slot) = item.data::<Slot>("thumb-slot") {
            return slot.as_ref().clone();
        }
        let slot = Slot::default();
        item.set_data("thumb-slot", slot.clone());
        slot
    }
}

/// Swap `icon`'s type icon for a thumbnail when the entry is an image or
/// video. The caller has already set the type icon, which stays otherwise.
/// Dim a hidden file's grid tile by colour, as the list does, never with
/// widget opacity: opacity on the tile would fade the selection drawn on it
/// too. `.hidden-item` fades only the icon or thumbnail.
fn mark_hidden(tile: &gtk::Box, label: &gtk::Label, hidden: bool) {
    if hidden {
        tile.add_css_class("hidden-item");
        label.add_css_class("hidden-file");
    } else {
        tile.remove_css_class("hidden-item");
        label.remove_css_class("hidden-file");
    }
}

fn show_thumbnail(item: &gtk::Box, icon: &gtk::Image, entry_obj: &FileEntryObject, size: u32) {
    let slot = thumb_slot(item);
    let local = entry_obj
        .entry()
        .and_then(|e| e.path.as_local_path().map(|p| p.to_path_buf()));
    let Some(local) = local else {
        slot.clear();
        return;
    };
    let weak = icon.downgrade();
    slot.request(&local, size, move |thumb| {
        if let Some(icon) = weak.upgrade() {
            icon.set_from_file(Some(thumb));
        }
    });
}

/// Make a list item draggable, and give it the "item-pos" cell that bind
/// keeps current and both the drag and right-click read.
fn attach_item_drag(widget: &gtk::Box, selection: &gtk::MultiSelection) {
    let pos_cell: Rc<RefCell<u32>> = Rc::new(RefCell::new(u32::MAX));

    let drag_source = gtk::DragSource::new();
    drag_source.set_actions(gtk::gdk::DragAction::COPY | gtk::gdk::DragAction::MOVE);
    let sel = selection.clone();
    let pos = pos_cell.clone();
    drag_source.connect_prepare(move |_source, _x, _y| drag_content(&sel, *pos.borrow()));
    // Weak: the controller is owned by the widget it would otherwise keep alive.
    let weak = widget.downgrade();
    drag_source.connect_drag_begin(move |source, _drag| {
        if let Some(widget) = weak.upgrade() {
            source.set_icon(Some(&gtk::WidgetPaintable::new(Some(&widget))), 0, 0);
        }
    });
    widget.add_controller(drag_source);

    unsafe {
        widget.set_data("item-pos", pos_cell);
    }
}

/// The data a drag out of the list carries: the whole selection when the
/// dragged item is part of it, otherwise just that item.
///
/// Other applications (browsers, editors, chat clients) only accept files as
/// a GdkFileList, which GTK offers as text/uri-list and through the portal's
/// file transfer for sandboxed apps. Raven's own drop targets read a plain
/// string of `file://` URI lines as a fallback, so that is offered alongside.
fn drag_content(selection: &gtk::MultiSelection, pos: u32) -> Option<gtk::gdk::ContentProvider> {
    if pos == u32::MAX {
        return None;
    }
    let positions: Vec<u32> = if selection.is_selected(pos) {
        (0..selection.n_items())
            .filter(|&i| selection.is_selected(i))
            .collect()
    } else {
        vec![pos]
    };
    let paths: Vec<std::path::PathBuf> = positions
        .into_iter()
        .filter_map(|i| selection.item(i).and_downcast::<FileEntryObject>())
        .filter_map(|obj| obj.entry())
        .filter_map(|entry| entry.path.as_local_path().map(|p| p.to_path_buf()))
        .collect();
    if paths.is_empty() {
        return None;
    }

    let files: Vec<gtk::gio::File> = paths.iter().map(gtk::gio::File::for_path).collect();
    // Escaped URIs (spaces as %20 and so on), which dnd::parse_uri_list and
    // other apps' uri-list readers both decode.
    let internal: String = files
        .iter()
        .map(|f| format!("{}\r\n", f.uri()))
        .collect();
    Some(gtk::gdk::ContentProvider::new_union(&[
        gtk::gdk::ContentProvider::for_value(&gtk::gdk::FileList::from_array(&files).to_value()),
        gtk::gdk::ContentProvider::for_value(&internal.to_value()),
    ]))
}

/// The directory a drop onto a listing lands in: the pane it was dropped on,
/// which in dual-pane mode need not be the active one.
fn drop_destination(state: &AppState, pane_id: u32) -> RavenPath {
    let s = state.borrow();
    s.pane_by_id(pane_id)
        .map(|p| p.current_path.clone())
        .unwrap_or_else(|| s.active_tab().active_pane().current_path.clone())
}

fn handle_file_drop(
    target: &gtk::DropTarget,
    value: &glib::Value,
    state: &AppState,
    cmd_tx: &tokio::sync::mpsc::UnboundedSender<AppCommand>,
    pane_id: u32,
) -> bool {
    let destination = drop_destination(state, pane_id);
    crate::dnd::perform_drop(target, value, &destination, cmd_tx)
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

#[cfg(test)]
mod tests {
    use super::quick_filter_matches;

    #[test]
    fn quick_filter_is_a_case_insensitive_substring_match() {
        assert!(quick_filter_matches("Report.PDF", ""));
        assert!(quick_filter_matches("Report.PDF", "rep"));
        assert!(quick_filter_matches("Report.PDF", "T.p"));
        assert!(quick_filter_matches("résumé.odt", "RÉS"));
        assert!(!quick_filter_matches("Report.PDF", "reports"));
        assert!(!quick_filter_matches("notes.txt", "q"));
    }
}
