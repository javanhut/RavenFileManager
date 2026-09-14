//! Raven's open/save dialog, shown for xdg-desktop-portal FileChooser requests.
//!
//! The bus side lives in `raven_dbus::portal`; this module is only the window.
//! It is deliberately separate from the main file list: a picker is a
//! short-lived, single-purpose window in another application's flow, and it
//! reads the directory straight through `gtk::DirectoryList` rather than
//! standing up the backend runtime a file manager window needs.
//!
//! The folder a dialog starts in is remembered per requesting application and
//! globally, separately for opening and saving, in `portal_state.toml` under
//! the Raven config directory.

use std::cell::RefCell;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use gtk4 as gtk;
use gtk::prelude::*;
use libadwaita as adw;
use adw::prelude::*;
use serde::{Deserialize, Serialize};

use raven_core::config::AppConfig;
use raven_dbus::portal::{ChooserMode, ChooserOptions, ChooserSelection, FileFilter, FilterRule};

const ATTRIBUTES: &str = "standard::name,standard::display-name,standard::icon,\
standard::type,standard::size,standard::is-hidden,standard::is-backup,\
standard::fast-content-type,time::modified";

// ---------------------------------------------------------------------------
// Remembered folders
// ---------------------------------------------------------------------------

/// Last folders for one requester (or the global fallback).
#[derive(Debug, Default, Clone, PartialEq, Serialize, Deserialize)]
pub struct FolderMemory {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub open: Option<PathBuf>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub save: Option<PathBuf>,
}

impl FolderMemory {
    fn get(&self, save: bool) -> Option<&Path> {
        if save {
            self.save.as_deref()
        } else {
            self.open.as_deref()
        }
    }

    fn set(&mut self, save: bool, folder: &Path) {
        let slot = if save { &mut self.save } else { &mut self.open };
        *slot = Some(folder.to_path_buf());
    }
}

/// Contents of `portal_state.toml`.
#[derive(Debug, Default, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChooserState {
    #[serde(default)]
    pub last: FolderMemory,
    #[serde(default)]
    pub apps: HashMap<String, FolderMemory>,
}

impl ChooserState {
    pub fn path() -> PathBuf {
        AppConfig::config_dir().join("portal_state.toml")
    }

    pub fn load() -> Self {
        std::fs::read_to_string(Self::path())
            .ok()
            .and_then(|s| toml::from_str(&s).ok())
            .unwrap_or_default()
    }

    pub fn store(&self) {
        let result = std::fs::create_dir_all(AppConfig::config_dir())
            .map_err(|e| e.to_string())
            .and_then(|_| toml::to_string_pretty(self).map_err(|e| e.to_string()))
            .and_then(|s| std::fs::write(Self::path(), s).map_err(|e| e.to_string()));
        if let Err(e) = result {
            tracing::warn!("Could not save file chooser state: {}", e);
        }
    }

    /// Record `folder` as the last one used, globally and for `app_id`.
    ///
    /// Unsandboxed callers often reach the portal with an empty app id; those
    /// only update the global entry, since an empty key would merge every such
    /// application into one.
    pub fn remember(&mut self, app_id: &str, save: bool, folder: &Path) {
        self.last.set(save, folder);
        if !app_id.is_empty() {
            self.apps.entry(app_id.to_string()).or_default().set(save, folder);
        }
    }

    /// The folder a dialog should open in.
    ///
    /// What the requester asked for wins, then where this application was last,
    /// then where any application was last, then `fallback`. Remembered folders
    /// that no longer exist are skipped.
    pub fn start_folder(
        &self,
        app_id: &str,
        save: bool,
        options: &ChooserOptions,
        fallback: &Path,
    ) -> PathBuf {
        let existing = |p: Option<&Path>| p.filter(|p| p.is_dir()).map(Path::to_path_buf);
        existing(options.current_folder.as_deref())
            .or_else(|| existing(options.current_file.as_deref().and_then(Path::parent)))
            .or_else(|| {
                if app_id.is_empty() {
                    None
                } else {
                    existing(self.apps.get(app_id).and_then(|m| m.get(save)))
                }
            })
            .or_else(|| existing(self.last.get(save)))
            .unwrap_or_else(|| fallback.to_path_buf())
    }
}

/// Downloads if it exists, else home.
pub fn default_folder() -> PathBuf {
    glib::user_special_dir(glib::UserDirectory::Downloads)
        .filter(|p| p.is_dir())
        .unwrap_or_else(glib::home_dir)
}

// ---------------------------------------------------------------------------
// Matching and path helpers
// ---------------------------------------------------------------------------

/// Case-insensitive shell glob: `*`, `?` and `[...]` classes (with `!`/`^`
/// negation and ranges). GTK sends patterns like `*.[pP][nN][gG]`, so classes
/// matter.
pub fn glob_matches(pattern: &str, name: &str) -> bool {
    let p: Vec<char> = pattern.to_lowercase().chars().collect();
    let n: Vec<char> = name.to_lowercase().chars().collect();
    glob_at(&p, &n)
}

fn glob_at(p: &[char], n: &[char]) -> bool {
    match p.first() {
        None => n.is_empty(),
        Some('*') => (0..=n.len()).any(|i| glob_at(&p[1..], &n[i..])),
        Some('?') => !n.is_empty() && glob_at(&p[1..], &n[1..]),
        Some('[') => {
            let Some(close) = p.iter().skip(2).position(|c| *c == ']').map(|i| i + 2) else {
                return n.first() == Some(&'[') && glob_at(&p[1..], &n[1..]);
            };
            let Some(c) = n.first() else {
                return false;
            };
            let class = &p[1..close];
            let (negate, class) = match class.first() {
                Some('!') | Some('^') => (true, &class[1..]),
                _ => (false, class),
            };
            let mut hit = false;
            let mut i = 0;
            while i < class.len() {
                if i + 2 < class.len() && class[i + 1] == '-' {
                    hit |= class[i] <= *c && *c <= class[i + 2];
                    i += 3;
                } else {
                    hit |= class[i] == *c;
                    i += 1;
                }
            }
            hit != negate && glob_at(&p[close + 1..], &n[1..])
        }
        Some(c) => n.first() == Some(c) && glob_at(&p[1..], &n[1..]),
    }
}

fn mime_matches(pattern: &str, content_type: &str) -> bool {
    if pattern == "*" || pattern == "*/*" || pattern.eq_ignore_ascii_case(content_type) {
        return true;
    }
    if let Some(major) = pattern.strip_suffix("/*") {
        return content_type
            .split('/')
            .next()
            .is_some_and(|m| m.eq_ignore_ascii_case(major));
    }
    gio::content_type_is_mime_type(content_type, pattern)
}

/// Whether a file named `name` passes `filter`. `content_type` is used for
/// MIME rules when known; otherwise it is guessed from the name.
pub fn filter_accepts(filter: &FileFilter, name: &str, content_type: Option<&str>) -> bool {
    if filter.rules.is_empty() {
        return true;
    }
    filter.rules.iter().any(|rule| match rule {
        FilterRule::Glob(g) => glob_matches(g, name),
        FilterRule::Mime(m) => match content_type {
            Some(ct) => mime_matches(m, ct),
            None => mime_matches(m, &gio::content_type_guess(Some(name), None).0),
        },
    })
}

/// Turn what the user typed into a path: `~`, `file://` URIs, absolute paths,
/// and names relative to the folder being shown.
pub fn resolve_typed(folder: &Path, text: &str) -> PathBuf {
    let text = text.trim();
    if text == "~" {
        return glib::home_dir();
    }
    if let Some(rest) = text.strip_prefix("~/") {
        return glib::home_dir().join(rest);
    }
    if text.starts_with("file://") {
        if let Some(p) = raven_dbus::uri::file_uri_to_path(text) {
            return p;
        }
    }
    if text.starts_with('/') {
        PathBuf::from(text)
    } else {
        folder.join(text)
    }
}

// ---------------------------------------------------------------------------
// Dialog
// ---------------------------------------------------------------------------

/// What to show.
pub struct DialogRequest {
    pub mode: ChooserMode,
    pub app_id: String,
    pub title: String,
    pub options: ChooserOptions,
}

type DoneFn = Box<dyn FnOnce(Option<ChooserSelection>)>;

struct FilterState {
    show_hidden: bool,
    folders_only: bool,
    current: Option<FileFilter>,
}

fn is_dir(info: &gio::FileInfo) -> bool {
    info.file_type() == gio::FileType::Directory
}

fn row_visible(state: &FilterState, info: &gio::FileInfo) -> bool {
    if !state.show_hidden && (info.is_hidden() || info.is_backup()) {
        return false;
    }
    if is_dir(info) {
        return true;
    }
    if state.folders_only {
        return false;
    }
    match &state.current {
        None => true,
        Some(filter) => filter_accepts(
            filter,
            &info.name().to_string_lossy(),
            info.attribute_string("standard::fast-content-type").as_deref(),
        ),
    }
}

struct Inner {
    mode: ChooserMode,
    app_id: String,
    options: ChooserOptions,
    window: adw::Window,
    folder: RefCell<PathBuf>,
    dir_list: gtk::DirectoryList,
    filter_state: Rc<RefCell<FilterState>>,
    model: gtk::SortListModel,
    selection: gtk::SelectionModel,
    location: gtk::Entry,
    up_button: gtk::Button,
    name_entry: gtk::Entry,
    status: gtk::Label,
    places_box: gtk::ListBox,
    /// The folder of each row of `places_box`, by row index.
    place_paths: Vec<PathBuf>,
    done: RefCell<Option<DoneFn>>,
}

/// A picker window. Cloning shares the same window.
#[derive(Clone)]
pub struct FileChooserDialog {
    inner: Rc<Inner>,
}

impl FileChooserDialog {
    /// Build and show a dialog. `on_done` runs exactly once, with `None` when
    /// the user cancels or the window is closed.
    pub fn present(
        app: &adw::Application,
        request: DialogRequest,
        on_done: impl FnOnce(Option<ChooserSelection>) + 'static,
    ) -> Self {
        let DialogRequest {
            mode,
            app_id,
            title,
            options,
        } = request;
        let picks_folder = options.directory || mode == ChooserMode::SaveFiles;
        let start = ChooserState::load().start_folder(
            &app_id,
            mode != ChooserMode::Open,
            &options,
            &default_folder(),
        );

        // Filter list: the requester's current filter goes first if it is not
        // among the offered ones.
        let mut filters = options.filters.clone();
        if let Some(cur) = &options.current_filter {
            if !filters.contains(cur) {
                filters.insert(0, cur.clone());
            }
        }
        let initial_filter = options
            .current_filter
            .clone()
            .or_else(|| filters.first().cloned());

        let filter_state = Rc::new(RefCell::new(FilterState {
            show_hidden: false,
            folders_only: picks_folder,
            current: if picks_folder { None } else { initial_filter.clone() },
        }));

        // --- Model ---
        let dir_list = gtk::DirectoryList::new(Some(ATTRIBUTES), None::<&gio::File>);
        let list_filter = {
            let fs = filter_state.clone();
            gtk::CustomFilter::new(move |obj| {
                obj.downcast_ref::<gio::FileInfo>()
                    .is_some_and(|info| row_visible(&fs.borrow(), info))
            })
        };
        let filtered = gtk::FilterListModel::new(Some(dir_list.clone()), Some(list_filter.clone()));
        let sorter = gtk::CustomSorter::new(|a, b| {
            let (Some(a), Some(b)) = (
                a.downcast_ref::<gio::FileInfo>(),
                b.downcast_ref::<gio::FileInfo>(),
            ) else {
                return gtk::Ordering::Equal;
            };
            is_dir(b)
                .cmp(&is_dir(a))
                .then_with(|| {
                    a.display_name()
                        .to_lowercase()
                        .cmp(&b.display_name().to_lowercase())
                })
                .into()
        });
        let model = gtk::SortListModel::new(Some(filtered), Some(sorter));
        let selection: gtk::SelectionModel = if options.multiple && mode == ChooserMode::Open {
            gtk::MultiSelection::new(Some(model.clone())).upcast()
        } else {
            let single = gtk::SingleSelection::new(Some(model.clone()));
            single.set_autoselect(false);
            single.set_can_unselect(true);
            single.upcast()
        };

        // --- Window and header ---
        let title_text = if title.is_empty() {
            match mode {
                ChooserMode::Open if picks_folder => "Select Folder",
                ChooserMode::Open => "Open File",
                ChooserMode::Save => "Save File",
                ChooserMode::SaveFiles => "Select Folder to Save In",
            }
            .to_string()
        } else {
            title
        };
        let window = adw::Window::builder()
            .application(app)
            .title(&title_text)
            .default_width(920)
            .default_height(620)
            .build();

        let header = adw::HeaderBar::new();
        header.set_show_start_title_buttons(false);
        header.set_show_end_title_buttons(false);
        header.set_title_widget(Some(&adw::WindowTitle::new(&title_text, &app_id)));

        let cancel_button = gtk::Button::with_mnemonic("_Cancel");
        header.pack_start(&cancel_button);

        let accept_label = options.accept_label.clone().unwrap_or_else(|| {
            match mode {
                ChooserMode::Open if picks_folder => "_Select",
                ChooserMode::Open => "_Open",
                _ => "_Save",
            }
            .to_string()
        });
        let accept_button = gtk::Button::with_mnemonic(&accept_label);
        accept_button.add_css_class("suggested-action");
        header.pack_end(&accept_button);

        let hidden_toggle = gtk::ToggleButton::new();
        hidden_toggle.set_icon_name("view-reveal-symbolic");
        hidden_toggle.set_tooltip_text(Some("Show Hidden Files (Ctrl+H)"));
        header.pack_end(&hidden_toggle);

        let new_folder_entry = gtk::Entry::new();
        new_folder_entry.set_placeholder_text(Some("Folder name"));
        let new_folder_popover = gtk::Popover::new();
        new_folder_popover.set_child(Some(&new_folder_entry));
        let new_folder_button = gtk::MenuButton::new();
        new_folder_button.set_icon_name("folder-new-symbolic");
        new_folder_button.set_tooltip_text(Some("New Folder"));
        new_folder_button.set_popover(Some(&new_folder_popover));
        new_folder_button.set_visible(mode != ChooserMode::Open || picks_folder);
        header.pack_end(&new_folder_button);

        // --- Places ---
        // Built like the file manager's own sidebar (eyebrow heading, tinted
        // tiles, the current folder's row selected), so the picker reads as
        // the same app.
        let place_list = places();
        let place_paths: Vec<PathBuf> = place_list.iter().map(|(_, _, p)| p.clone()).collect();
        let places_box = gtk::ListBox::new();
        places_box.add_css_class("navigation-sidebar");
        for (label, icon, _) in &place_list {
            let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
            row.set_margin_top(1);
            row.set_margin_bottom(1);
            row.append(&crate::widgets::sidebar::nav_tile(icon));
            let l = gtk::Label::new(Some(label));
            l.set_halign(gtk::Align::Start);
            l.set_ellipsize(gtk::pango::EllipsizeMode::End);
            row.append(&l);
            places_box.append(&row);
        }
        let places_column = gtk::Box::new(gtk::Orientation::Vertical, 0);
        places_column.add_css_class("places");
        places_column.append(&crate::widgets::sidebar::section_heading("Places", true));
        places_column.append(&places_box);
        let places_scroll = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .child(&places_column)
            .build();
        // Raven Glass's sidebar, like the file manager's own places.
        places_scroll.add_css_class("sidebar");

        // --- Location row ---
        let up_button = gtk::Button::from_icon_name("go-up-symbolic");
        up_button.set_tooltip_text(Some("Parent Folder (Alt+Up)"));
        let location = gtk::Entry::new();
        location.set_hexpand(true);
        location.set_tooltip_text(Some("Type a path and press Enter (Ctrl+L)"));
        let location_row = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        location_row.add_css_class("location-row");
        up_button.add_css_class("flat");
        location_row.append(&up_button);
        location_row.append(&location);

        // --- File list ---
        let column_view = gtk::ColumnView::new(Some(selection.clone()));
        column_view.add_css_class("data-table");
        build_columns(&column_view);
        let list_scroll = gtk::ScrolledWindow::builder()
            .child(&column_view)
            .vexpand(true)
            .hexpand(true)
            .build();

        let status = gtk::Label::new(None);
        status.add_css_class("dim-label");
        status.set_halign(gtk::Align::Start);
        status.set_margin_start(8);
        status.set_margin_top(4);
        status.set_visible(false);

        // --- Bottom row: name and filter ---
        let bottom = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        bottom.add_css_class("chooser-footer");
        let name_entry = gtk::Entry::new();
        if mode == ChooserMode::Save {
            let name_label = gtk::Label::with_mnemonic("_Name");
            name_label.set_mnemonic_widget(Some(&name_entry));
            bottom.append(&name_label);
            name_entry.set_hexpand(true);
            name_entry.set_activates_default(true);
            let suggested = options.current_name.clone().or_else(|| {
                options
                    .current_file
                    .as_ref()
                    .and_then(|f| f.file_name())
                    .map(|n| n.to_string_lossy().into_owned())
            });
            if let Some(name) = suggested {
                name_entry.set_text(&name);
            }
            bottom.append(&name_entry);
        } else {
            let spacer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
            spacer.set_hexpand(true);
            bottom.append(&spacer);
        }

        let filter_dropdown = if !filters.is_empty() && !picks_folder {
            let names: Vec<&str> = filters.iter().map(|f| f.name.as_str()).collect();
            let dropdown = gtk::DropDown::from_strings(&names);
            let idx = initial_filter
                .as_ref()
                .and_then(|f| filters.iter().position(|g| g == f))
                .unwrap_or(0);
            dropdown.set_selected(idx as u32);
            bottom.append(&dropdown);
            Some(dropdown)
        } else {
            None
        };

        // Opening with no filters to choose from leaves the footer with
        // nothing in it; an empty strip under a hairline reads as a mistake.
        bottom.set_visible(mode == ChooserMode::Save || filter_dropdown.is_some());

        let content = gtk::Box::new(gtk::Orientation::Vertical, 0);
        content.append(&location_row);
        content.append(&list_scroll);
        content.append(&status);
        content.append(&bottom);

        let paned = gtk::Paned::new(gtk::Orientation::Horizontal);
        paned.set_start_child(Some(&places_scroll));
        paned.set_end_child(Some(&content));
        paned.set_position(190);
        paned.set_resize_start_child(false);
        paned.set_shrink_start_child(false);

        let toolbar = adw::ToolbarView::new();
        toolbar.add_top_bar(&header);
        toolbar.set_content(Some(&paned));
        window.set_content(Some(&toolbar));
        window.set_default_widget(Some(&accept_button));

        let inner = Rc::new(Inner {
            mode,
            app_id,
            options,
            window: window.clone(),
            folder: RefCell::new(start.clone()),
            dir_list: dir_list.clone(),
            filter_state: filter_state.clone(),
            model,
            selection: selection.clone(),
            location: location.clone(),
            up_button: up_button.clone(),
            name_entry: name_entry.clone(),
            status,
            places_box: places_box.clone(),
            place_paths,
            done: RefCell::new(Some(Box::new(on_done))),
        });
        inner.navigate(&start);

        // --- Signals ---
        let weak = Rc::downgrade(&inner);
        cancel_button.connect_clicked(move |_| {
            if let Some(inner) = weak.upgrade() {
                inner.finish_and_close(None);
            }
        });

        let weak = Rc::downgrade(&inner);
        accept_button.connect_clicked(move |_| {
            if let Some(inner) = weak.upgrade() {
                inner.accept();
            }
        });

        // Holds the only guaranteed strong reference: the dialog lives as long
        // as its window, and GTK drops the handler when the window is disposed.
        let strong = inner.clone();
        window.connect_close_request(move |_| {
            strong.finish(None);
            glib::Propagation::Proceed
        });

        let weak = Rc::downgrade(&inner);
        up_button.connect_clicked(move |_| {
            if let Some(inner) = weak.upgrade() {
                inner.go_up();
            }
        });

        let weak = Rc::downgrade(&inner);
        location.connect_activate(move |_| {
            if let Some(inner) = weak.upgrade() {
                inner.location_activated();
            }
        });

        let weak = Rc::downgrade(&inner);
        places_box.connect_row_activated(move |_, row| {
            if let (Some(inner), Some((_, _, path))) =
                (weak.upgrade(), place_list.get(row.index() as usize))
            {
                inner.navigate(path);
            }
        });

        let weak = Rc::downgrade(&inner);
        column_view.connect_activate(move |_, pos| {
            if let Some(inner) = weak.upgrade() {
                inner.activate_row(pos);
            }
        });

        let weak = Rc::downgrade(&inner);
        selection.connect_selection_changed(move |_, _, _| {
            if let Some(inner) = weak.upgrade() {
                inner.selection_changed();
            }
        });

        {
            let fs = filter_state.clone();
            let list_filter = list_filter.clone();
            hidden_toggle.connect_toggled(move |t| {
                fs.borrow_mut().show_hidden = t.is_active();
                list_filter.changed(gtk::FilterChange::Different);
            });
        }

        if let Some(dropdown) = &filter_dropdown {
            let fs = filter_state.clone();
            let list_filter = list_filter.clone();
            dropdown.connect_selected_notify(move |dd| {
                fs.borrow_mut().current = filters.get(dd.selected() as usize).cloned();
                list_filter.changed(gtk::FilterChange::Different);
            });
        }

        {
            let weak = Rc::downgrade(&inner);
            let popover = new_folder_popover.clone();
            new_folder_entry.connect_activate(move |entry| {
                let Some(inner) = weak.upgrade() else { return };
                let name = entry.text().trim().to_string();
                if name.is_empty() || name.contains('/') {
                    return;
                }
                let path = inner.folder.borrow().join(&name);
                match std::fs::create_dir(&path) {
                    Ok(()) => {
                        popover.popdown();
                        entry.set_text("");
                        inner.navigate(&path);
                    }
                    Err(e) => inner.set_status(&format!("Could not create folder: {}", e)),
                }
            });
        }

        {
            let weak = Rc::downgrade(&inner);
            dir_list.connect_loading_notify(move |list| {
                let Some(inner) = weak.upgrade() else { return };
                if !list.is_loading() {
                    if let Some(err) = list.error() {
                        inner.set_status(&format!("Cannot read this folder: {}", err.message()));
                    }
                }
            });
        }

        // --- Shortcuts ---
        let shortcuts = gtk::ShortcutController::new();
        {
            let weak = Rc::downgrade(&inner);
            add_shortcut(&shortcuts, "Escape", move || {
                if let Some(inner) = weak.upgrade() {
                    inner.finish_and_close(None);
                }
            });
            let toggle = hidden_toggle.clone();
            add_shortcut(&shortcuts, "<Control>h", move || toggle.set_active(!toggle.is_active()));
            let loc = location.clone();
            add_shortcut(&shortcuts, "<Control>l", move || {
                loc.grab_focus();
                loc.select_region(0, -1);
            });
            let weak = Rc::downgrade(&inner);
            add_shortcut(&shortcuts, "<Alt>Up", move || {
                if let Some(inner) = weak.upgrade() {
                    inner.go_up();
                }
            });
        }
        window.add_controller(shortcuts);

        window.present();
        if mode == ChooserMode::Save {
            name_entry.grab_focus();
            let text = name_entry.text();
            let stem = Path::new(text.as_str())
                .file_stem()
                .map(|s| s.to_string_lossy().chars().count())
                .unwrap_or(0);
            name_entry.select_region(0, stem as i32);
        } else {
            column_view.grab_focus();
        }

        Self { inner }
    }

    /// Close the dialog as cancelled, e.g. when the requester withdraws the
    /// request. Does nothing once the dialog has answered.
    pub fn cancel(&self) {
        self.inner.finish_and_close(None);
    }
}

impl Inner {
    fn picks_folder(&self) -> bool {
        self.options.directory || self.mode == ChooserMode::SaveFiles
    }

    fn set_status(&self, msg: &str) {
        self.status.set_text(msg);
        self.status.set_visible(!msg.is_empty());
    }

    fn navigate(&self, path: &Path) {
        if !path.is_dir() {
            self.set_status(&format!("“{}” is not a folder", path.display()));
            return;
        }
        self.set_status("");
        *self.folder.borrow_mut() = path.to_path_buf();
        self.dir_list.set_file(Some(&gio::File::for_path(path)));
        self.location.set_text(&path.to_string_lossy());
        self.up_button.set_sensitive(path.parent().is_some());
        // The place being shown is the selected row, as in the main sidebar.
        match self.place_paths.iter().position(|p| p == path) {
            Some(index) => {
                let row = self.places_box.row_at_index(index as i32);
                self.places_box.select_row(row.as_ref());
            }
            None => self.places_box.unselect_all(),
        }
    }

    fn go_up(&self) {
        let parent = self.folder.borrow().parent().map(Path::to_path_buf);
        if let Some(parent) = parent {
            self.navigate(&parent);
        }
    }

    fn info_at(&self, pos: u32) -> Option<gio::FileInfo> {
        self.model.item(pos).and_downcast()
    }

    fn path_of(&self, info: &gio::FileInfo) -> PathBuf {
        self.folder.borrow().join(info.name())
    }

    fn selected(&self) -> Vec<gio::FileInfo> {
        let set = self.selection.selection();
        (0..set.size())
            .filter_map(|i| self.info_at(set.nth(i as u32)))
            .collect()
    }

    fn activate_row(self: &Rc<Self>, pos: u32) {
        let Some(info) = self.info_at(pos) else { return };
        if is_dir(&info) {
            let path = self.path_of(&info);
            self.navigate(&path);
            return;
        }
        match self.mode {
            ChooserMode::Open if !self.picks_folder() => {
                if !self.selection.is_selected(pos) {
                    self.selection.select_item(pos, true);
                }
                self.accept();
            }
            ChooserMode::Save => {
                self.name_entry.set_text(&info.name().to_string_lossy());
                self.accept();
            }
            _ => {}
        }
    }

    fn selection_changed(&self) {
        if self.mode != ChooserMode::Save {
            return;
        }
        if let Some(info) = self.selected().into_iter().find(|i| !is_dir(i)) {
            self.name_entry.set_text(&info.name().to_string_lossy());
        }
    }

    fn location_activated(self: &Rc<Self>) {
        let folder = self.folder.borrow().clone();
        let target = resolve_typed(&folder, &self.location.text());
        if target.is_dir() {
            self.navigate(&target);
            return;
        }
        match self.mode {
            ChooserMode::Open if !self.picks_folder() && target.is_file() => {
                self.finish_and_close(Some(vec![target]));
            }
            ChooserMode::Save => match (target.parent(), target.file_name()) {
                (Some(parent), Some(name)) if parent.is_dir() => {
                    self.navigate(parent);
                    self.name_entry.set_text(&name.to_string_lossy());
                    self.accept();
                }
                _ => self.set_status(&format!("“{}” does not exist", target.display())),
            },
            _ => self.set_status(&format!("“{}” does not exist", target.display())),
        }
    }

    fn accept(self: &Rc<Self>) {
        let folder = self.folder.borrow().clone();
        match self.mode {
            ChooserMode::Open => {
                let selected = self.selected();
                if self.picks_folder() {
                    let dirs: Vec<PathBuf> = selected
                        .iter()
                        .filter(|i| is_dir(i))
                        .map(|i| self.path_of(i))
                        .collect();
                    let paths = if dirs.is_empty() { vec![folder] } else { dirs };
                    self.finish_and_close(Some(paths));
                    return;
                }
                let files: Vec<PathBuf> = selected
                    .iter()
                    .filter(|i| !is_dir(i))
                    .map(|i| self.path_of(i))
                    .collect();
                if !files.is_empty() {
                    self.finish_and_close(Some(files));
                } else if let [only] = selected.as_slice() {
                    let path = self.path_of(only);
                    self.navigate(&path);
                } else {
                    self.set_status("Select a file to open");
                }
            }
            ChooserMode::Save => {
                let name = self.name_entry.text().trim().to_string();
                if name.is_empty() {
                    self.set_status("Enter a file name");
                    self.name_entry.grab_focus();
                    return;
                }
                let target = resolve_typed(&folder, &name);
                if target.is_dir() {
                    self.navigate(&target);
                    self.name_entry.set_text("");
                    return;
                }
                if !target.parent().is_some_and(Path::is_dir) {
                    self.set_status(&format!("The folder for “{}” does not exist", target.display()));
                    return;
                }
                if target.exists() {
                    self.confirm_replace(vec![target.clone()], vec![target]);
                } else {
                    self.finish_and_close(Some(vec![target]));
                }
            }
            ChooserMode::SaveFiles => {
                let dest = self
                    .selected()
                    .iter()
                    .find(|i| is_dir(i))
                    .map(|i| self.path_of(i))
                    .unwrap_or(folder);
                let existing: Vec<PathBuf> = self
                    .options
                    .files
                    .iter()
                    .map(|n| dest.join(n))
                    .filter(|p| p.exists())
                    .collect();
                if existing.is_empty() {
                    self.finish_and_close(Some(vec![dest]));
                } else {
                    self.confirm_replace(existing, vec![dest]);
                }
            }
        }
    }

    fn confirm_replace(self: &Rc<Self>, existing: Vec<PathBuf>, result: Vec<PathBuf>) {
        let heading = match existing.as_slice() {
            [one] => format!(
                "Replace “{}”?",
                one.file_name().map(|n| n.to_string_lossy()).unwrap_or_default()
            ),
            many => format!("Replace {} files?", many.len()),
        };
        let dialog = adw::MessageDialog::new(
            Some(&self.window),
            Some(&heading),
            Some("A file with this name already exists. Replacing it will overwrite its contents."),
        );
        dialog.add_responses(&[("cancel", "_Cancel"), ("replace", "_Replace")]);
        dialog.set_response_appearance("replace", adw::ResponseAppearance::Destructive);
        dialog.set_default_response(Some("cancel"));
        dialog.set_close_response("cancel");
        let weak = Rc::downgrade(self);
        dialog.connect_response(None, move |_, response| {
            if response == "replace" {
                if let Some(inner) = weak.upgrade() {
                    inner.finish_and_close(Some(result.clone()));
                }
            }
        });
        dialog.present();
    }

    /// Answer the requester once; later calls do nothing.
    fn finish(&self, paths: Option<Vec<PathBuf>>) {
        let Some(done) = self.done.borrow_mut().take() else {
            return;
        };
        let selection = paths.map(|paths| {
            // Files picked here were used by the requesting application, so
            // they belong in the recent list like files Raven opens itself.
            // A save target may not exist yet; RecentManager adds it anyway.
            for path in paths.iter().filter(|p| !p.is_dir()) {
                crate::file_opener::record_recent(path);
            }
            let folder = match self.mode {
                ChooserMode::Save => paths.first().and_then(|p| p.parent()).map(Path::to_path_buf),
                ChooserMode::SaveFiles => paths.first().cloned(),
                ChooserMode::Open => Some(self.folder.borrow().clone()),
            };
            if let Some(folder) = folder {
                let mut state = ChooserState::load();
                state.remember(&self.app_id, self.mode != ChooserMode::Open, &folder);
                state.store();
            }
            ChooserSelection {
                paths,
                current_filter: if self.options.filters.is_empty() {
                    None
                } else {
                    self.filter_state.borrow().current.clone()
                },
            }
        });
        done(selection);
    }

    fn finish_and_close(&self, paths: Option<Vec<PathBuf>>) {
        self.finish(paths);
        self.window.close();
    }
}

fn add_shortcut(controller: &gtk::ShortcutController, accel: &str, f: impl Fn() + 'static) {
    let action = gtk::CallbackAction::new(move |_, _| {
        f();
        glib::Propagation::Stop
    });
    controller.add_shortcut(gtk::Shortcut::new(
        gtk::ShortcutTrigger::parse_string(accel),
        Some(action),
    ));
}

/// Sidebar places: home, the XDG user folders, the filesystem root, then Raven
/// bookmarks not already listed.
fn places() -> Vec<(String, String, PathBuf)> {
    let home = glib::home_dir();
    let mut out = vec![("Home".to_string(), "user-home-symbolic".to_string(), home.clone())];
    let special = [
        (glib::UserDirectory::Desktop, "Desktop", "user-desktop-symbolic"),
        (glib::UserDirectory::Documents, "Documents", "folder-documents-symbolic"),
        (glib::UserDirectory::Downloads, "Downloads", "folder-download-symbolic"),
        (glib::UserDirectory::Pictures, "Pictures", "folder-pictures-symbolic"),
        (glib::UserDirectory::Videos, "Videos", "folder-videos-symbolic"),
        (glib::UserDirectory::Music, "Music", "folder-music-symbolic"),
    ];
    for (dir, name, icon) in special {
        if let Some(path) = glib::user_special_dir(dir) {
            if path.is_dir() && !out.iter().any(|(_, _, p)| *p == path) {
                out.push((name.to_string(), icon.to_string(), path));
            }
        }
    }
    for bookmark in AppConfig::load().bookmarks {
        let path = resolve_typed(&home, &bookmark.path);
        if path.is_dir() && !out.iter().any(|(_, _, p)| *p == path) {
            let icon = bookmark.icon.unwrap_or_else(|| "folder-symbolic".to_string());
            out.push((bookmark.name, icon, path));
        }
    }
    out.push((
        "Computer".to_string(),
        "drive-harddisk-symbolic".to_string(),
        PathBuf::from("/"),
    ));
    out
}

fn build_columns(column_view: &gtk::ColumnView) {
    let name_factory = gtk::SignalListItemFactory::new();
    name_factory.connect_setup(|_, item| {
        let item = item.downcast_ref::<gtk::ListItem>().unwrap();
        let row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        let icon = gtk::Image::new();
        let label = gtk::Label::new(None);
        label.set_halign(gtk::Align::Start);
        label.set_ellipsize(gtk::pango::EllipsizeMode::End);
        row.append(&icon);
        row.append(&label);
        item.set_child(Some(&row));
    });
    name_factory.connect_bind(|_, item| {
        let item = item.downcast_ref::<gtk::ListItem>().unwrap();
        let Some(info) = item.item().and_downcast::<gio::FileInfo>() else { return };
        let Some(row) = item.child().and_downcast::<gtk::Box>() else { return };
        let Some(icon) = row.first_child().and_downcast::<gtk::Image>() else { return };
        let Some(label) = icon.next_sibling().and_downcast::<gtk::Label>() else { return };
        match info.icon() {
            Some(gicon) => icon.set_from_gicon(&gicon),
            None => icon.set_icon_name(Some("text-x-generic")),
        }
        label.set_text(&info.display_name());
    });
    let name_col = gtk::ColumnViewColumn::new(Some("Name"), Some(name_factory));
    name_col.set_expand(true);
    name_col.set_resizable(true);
    column_view.append_column(&name_col);

    label_column(column_view, "Size", |info| {
        if is_dir(info) {
            String::new()
        } else {
            glib::format_size(info.size() as u64).to_string()
        }
    });
    label_column(column_view, "Modified", |info| {
        info.modification_date_time()
            .and_then(|d| d.to_local().ok())
            .and_then(|d| d.format("%Y-%m-%d %H:%M").ok())
            .map(|s| s.to_string())
            .unwrap_or_default()
    });
}

fn label_column(
    column_view: &gtk::ColumnView,
    title: &str,
    text: impl Fn(&gio::FileInfo) -> String + 'static,
) {
    let factory = gtk::SignalListItemFactory::new();
    factory.connect_setup(|_, item| {
        let item = item.downcast_ref::<gtk::ListItem>().unwrap();
        let label = gtk::Label::new(None);
        label.set_xalign(0.0);
        label.add_css_class("dim-label");
        item.set_child(Some(&label));
    });
    factory.connect_bind(move |_, item| {
        let item = item.downcast_ref::<gtk::ListItem>().unwrap();
        let (Some(info), Some(label)) = (
            item.item().and_downcast::<gio::FileInfo>(),
            item.child().and_downcast::<gtk::Label>(),
        ) else {
            return;
        };
        label.set_text(&text(&info));
    });
    let col = gtk::ColumnViewColumn::new(Some(title), Some(factory));
    col.set_resizable(true);
    column_view.append_column(&col);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn globs_match_case_insensitively_with_classes() {
        assert!(glob_matches("*.png", "Photo.PNG"));
        assert!(glob_matches("*.[pP][nN][gG]", "a.png"));
        assert!(glob_matches("report-??.pdf", "report-01.pdf"));
        assert!(!glob_matches("report-??.pdf", "report-1.pdf"));
        assert!(glob_matches("*", ""));
        assert!(glob_matches("[a-c]*", "beta"));
        assert!(!glob_matches("[!a-c]*", "beta"));
        assert!(!glob_matches("*.jpg", "a.jpeg"));
    }

    #[test]
    fn filters_accept_by_glob_or_mime_family() {
        let images = FileFilter {
            name: "Images".into(),
            rules: vec![FilterRule::Mime("image/*".into()), FilterRule::Glob("*.xcf".into())],
        };
        assert!(filter_accepts(&images, "a.bin", Some("image/png")));
        assert!(filter_accepts(&images, "art.xcf", Some("application/octet-stream")));
        assert!(!filter_accepts(&images, "notes.txt", Some("text/plain")));
        let empty = FileFilter { name: "All".into(), rules: vec![] };
        assert!(filter_accepts(&empty, "x", None));
    }

    #[test]
    fn typed_paths_resolve_against_the_folder() {
        let folder = Path::new("/srv/data");
        assert_eq!(resolve_typed(folder, "a.txt"), PathBuf::from("/srv/data/a.txt"));
        assert_eq!(resolve_typed(folder, " /etc/hosts "), PathBuf::from("/etc/hosts"));
        assert_eq!(resolve_typed(folder, "file:///tmp/a%20b"), PathBuf::from("/tmp/a b"));
        assert_eq!(resolve_typed(folder, "~/x"), glib::home_dir().join("x"));
    }

    #[test]
    fn start_folder_prefers_request_then_app_then_global() {
        let mut state = ChooserState::default();
        let fallback = Path::new("/");
        let none = ChooserOptions::default();
        assert_eq!(state.start_folder("brave", true, &none, fallback), PathBuf::from("/"));

        state.remember("", true, Path::new("/tmp"));
        assert_eq!(state.start_folder("brave", true, &none, fallback), PathBuf::from("/tmp"));
        // Open and save are remembered separately.
        assert_eq!(state.start_folder("brave", false, &none, fallback), PathBuf::from("/"));

        state.remember("brave", true, Path::new("/usr"));
        state.remember("other", true, Path::new("/etc"));
        assert_eq!(state.start_folder("brave", true, &none, fallback), PathBuf::from("/usr"));
        assert_eq!(state.last.save, Some(PathBuf::from("/etc")));

        let requested = ChooserOptions {
            current_folder: Some(PathBuf::from("/var")),
            ..Default::default()
        };
        assert_eq!(state.start_folder("brave", true, &requested, fallback), PathBuf::from("/var"));

        // A remembered folder that is gone is skipped.
        state.remember("gone", false, Path::new("/definitely/not/here"));
        assert_eq!(state.start_folder("gone", false, &none, fallback), PathBuf::from("/"));
    }

    #[test]
    fn state_round_trips_through_toml() {
        let mut state = ChooserState::default();
        state.remember("org.example.App", false, Path::new("/tmp"));
        let text = toml::to_string_pretty(&state).unwrap();
        let back: ChooserState = toml::from_str(&text).unwrap();
        assert_eq!(back, state);
    }
}
