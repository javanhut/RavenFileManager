use std::cell::RefCell;
use std::collections::HashSet;
use std::path::PathBuf;
use std::rc::Rc;

use raven_core::config::{AppConfig, ViewMode};
use raven_core::entry::FileEntry;
use raven_core::filter::FilterSpec;
use raven_core::path::RavenPath;
use raven_core::sort::SortSpec;

/// A tag restriction resolved by the backend, which owns the TagEngine.
#[derive(Debug, Clone)]
pub struct TagFilter {
    pub tag: String,
    pub paths: HashSet<RavenPath>,
}

/// Per-pane state tracking navigation history and current directory contents.
#[derive(Debug)]
pub struct PaneState {
    pub id: u32,
    pub current_path: RavenPath,
    pub entries: Vec<FileEntry>,
    pub selection: Vec<usize>,
    pub sort: SortSpec,
    /// Search bar / attribute filter. Independent of `tag_filter`; both apply.
    pub filter: FilterSpec,
    /// Sidebar tag restriction, if one is active.
    pub tag_filter: Option<TagFilter>,
    pub history_back: Vec<RavenPath>,
    pub history_forward: Vec<RavenPath>,
    /// Paths to select once this pane's listing contains them, from a reveal
    /// request that arrived before the directory finished loading.
    pub pending_selection: Vec<RavenPath>,
    /// Raise the properties dialog for the first item of `pending_selection`.
    pub pending_properties: bool,
}

impl PaneState {
    pub fn new(id: u32, path: RavenPath) -> Self {
        Self {
            id,
            current_path: path,
            entries: Vec::new(),
            selection: Vec::new(),
            sort: SortSpec::default(),
            filter: FilterSpec::default(),
            tag_filter: None,
            history_back: Vec::new(),
            history_forward: Vec::new(),
            pending_selection: Vec::new(),
            pending_properties: false,
        }
    }

    /// Drop a reveal that has not been applied yet. Called whenever the user
    /// navigates by hand: they have asked for a different directory than the
    /// one the reveal was waiting on, and applying it later would select an
    /// entry in a listing they never asked to see.
    fn clear_pending_selection(&mut self) {
        self.pending_selection.clear();
        self.pending_properties = false;
    }

    /// Entries surviving both filters. The two are independent restrictions, so an
    /// entry must satisfy each one.
    pub fn visible_entries(&self) -> Vec<FileEntry> {
        self.entries
            .iter()
            .filter(|e| self.filter.matches(e))
            .filter(|e| {
                self.tag_filter
                    .as_ref()
                    .is_none_or(|t| t.paths.contains(&e.path))
            })
            .cloned()
            .collect()
    }

    /// Status-bar labels for whatever is narrowing the listing, e.g.
    /// `["filtered", "tag: Images"]`. Empty when nothing is active.
    pub fn filter_labels(&self) -> Vec<String> {
        let mut labels = Vec::new();
        if !self.filter.is_empty() {
            labels.push("filtered".to_string());
        }
        if let Some(ref t) = self.tag_filter {
            labels.push(format!("tag: {}", t.tag));
        }
        labels
    }

    /// Drop both filters. Used when the pane loads a new directory.
    pub fn clear_filters(&mut self) {
        self.filter = FilterSpec::default();
        self.tag_filter = None;
    }

    pub fn navigate_to(&mut self, path: RavenPath) {
        self.history_back.push(self.current_path.clone());
        self.history_forward.clear();
        self.current_path = path;
        self.selection.clear();
        self.clear_pending_selection();
    }

    pub fn go_back(&mut self) -> Option<RavenPath> {
        if let Some(prev) = self.history_back.pop() {
            self.history_forward.push(self.current_path.clone());
            self.current_path = prev.clone();
            self.selection.clear();
            self.clear_pending_selection();
            Some(prev)
        } else {
            None
        }
    }

    pub fn go_forward(&mut self) -> Option<RavenPath> {
        if let Some(next) = self.history_forward.pop() {
            self.history_back.push(self.current_path.clone());
            self.current_path = next.clone();
            self.selection.clear();
            self.clear_pending_selection();
            Some(next)
        } else {
            None
        }
    }

    pub fn can_go_back(&self) -> bool {
        !self.history_back.is_empty()
    }

    pub fn can_go_forward(&self) -> bool {
        !self.history_forward.is_empty()
    }
}

/// Tab state wrapping one or two panes.
#[derive(Debug)]
pub struct TabState {
    pub id: u32,
    pub title: String,
    pub pane: PaneState,
    pub secondary_pane: Option<PaneState>,
    pub dual_pane_active: bool,
}

impl TabState {
    pub fn new(id: u32, path: RavenPath) -> Self {
        let title = path
            .file_name()
            .unwrap_or("/")
            .to_string();
        Self {
            id,
            title,
            pane: PaneState::new(id * 2, path),
            secondary_pane: None,
            dual_pane_active: false,
        }
    }

    pub fn active_pane(&self) -> &PaneState {
        &self.pane
    }

    pub fn active_pane_mut(&mut self) -> &mut PaneState {
        &mut self.pane
    }
}

/// Clipboard operation for cut/copy/paste.
#[derive(Debug, Clone)]
pub enum ClipboardOp {
    Copy(Vec<RavenPath>),
    Cut(Vec<RavenPath>),
}

/// Inner state holding all application data — accessed via Rc<RefCell<>>.
#[derive(Debug)]
pub struct AppStateInner {
    pub config: AppConfig,
    pub tabs: Vec<TabState>,
    pub active_tab: usize,
    pub show_hidden: bool,
    pub show_preview: bool,
    pub preview_path: Option<RavenPath>,
    pub clipboard: Option<ClipboardOp>,
    pub view_mode: ViewMode,
    next_tab_id: u32,
}

impl AppStateInner {
    pub fn new(config: AppConfig) -> Self {
        let home = std::env::var("HOME").unwrap_or_else(|_| "/".to_string());
        let initial_path = RavenPath::local(PathBuf::from(
            config
                .navigation
                .default_path
                .clone()
                .unwrap_or(home),
        ));

        let initial_tab = TabState::new(0, initial_path);

        let view_mode = config.appearance.view_mode;
        Self {
            config,
            tabs: vec![initial_tab],
            active_tab: 0,
            show_hidden: false,
            show_preview: false,
            preview_path: None,
            clipboard: None,
            view_mode,
            next_tab_id: 1,
        }
    }

    pub fn active_tab(&self) -> &TabState {
        &self.tabs[self.active_tab]
    }

    pub fn active_tab_mut(&mut self) -> &mut TabState {
        &mut self.tabs[self.active_tab]
    }

    pub fn add_tab(&mut self, path: RavenPath) -> u32 {
        let id = self.next_tab_id;
        self.next_tab_id += 1;
        self.tabs.push(TabState::new(id, path));
        self.active_tab = self.tabs.len() - 1;
        id
    }

    pub fn close_tab(&mut self, index: usize) -> bool {
        if self.tabs.len() <= 1 {
            return false;
        }
        self.tabs.remove(index);
        if self.active_tab >= self.tabs.len() {
            self.active_tab = self.tabs.len() - 1;
        }
        true
    }

    pub fn pane_by_id(&self, pane_id: u32) -> Option<&PaneState> {
        for tab in &self.tabs {
            if tab.pane.id == pane_id {
                return Some(&tab.pane);
            }
            if let Some(ref secondary) = tab.secondary_pane {
                if secondary.id == pane_id {
                    return Some(secondary);
                }
            }
        }
        None
    }

    pub fn pane_by_id_mut(&mut self, pane_id: u32) -> Option<&mut PaneState> {
        for tab in &mut self.tabs {
            if tab.pane.id == pane_id {
                return Some(&mut tab.pane);
            }
            if let Some(ref mut secondary) = tab.secondary_pane {
                if secondary.id == pane_id {
                    return Some(secondary);
                }
            }
        }
        None
    }
}

/// Thread-safe (within GTK main thread) shared state handle.
#[derive(Debug, Clone)]
pub struct AppState {
    inner: Rc<RefCell<AppStateInner>>,
}

impl AppState {
    pub fn new(config: AppConfig) -> Self {
        Self {
            inner: Rc::new(RefCell::new(AppStateInner::new(config))),
        }
    }

    pub fn borrow(&self) -> std::cell::Ref<'_, AppStateInner> {
        self.inner.borrow()
    }

    pub fn borrow_mut(&self) -> std::cell::RefMut<'_, AppStateInner> {
        self.inner.borrow_mut()
    }
}
