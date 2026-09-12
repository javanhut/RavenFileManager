use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::rc::Rc;

use raven_core::config::{AppConfig, ViewMode};
use raven_core::custom_actions::{self, CustomAction};
use raven_core::entry::FileEntry;
use raven_core::events::GitFileStatus;
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
    /// Version-control status of the listing's entries, by file name. Empty
    /// outside a repository or until the backend reports it.
    pub vcs_statuses: HashMap<String, GitFileStatus>,
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
            vcs_statuses: HashMap::new(),
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

/// Which of a tab's two panes a widget or a key press refers to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PaneSide {
    Left,
    Right,
}

impl PaneSide {
    pub fn other(self) -> Self {
        match self {
            PaneSide::Left => PaneSide::Right,
            PaneSide::Right => PaneSide::Left,
        }
    }
}

/// How a widget finds out which pane it acts on. Panes change under a
/// widget -- switching tabs shows another pane in the same list, and the
/// sidebar always targets whichever pane is active -- so the id is looked up
/// when needed rather than fixed at construction.
pub type PaneResolver = Rc<dyn Fn() -> u32>;

/// Tab state wrapping one or two panes.
#[derive(Debug)]
pub struct TabState {
    pub id: u32,
    pub title: String,
    pub pane: PaneState,
    pub secondary_pane: Option<PaneState>,
    pub dual_pane_active: bool,
    /// The pane keyboard and sidebar actions go to.
    pub active_side: PaneSide,
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
            active_side: PaneSide::Left,
        }
    }

    pub fn active_pane(&self) -> &PaneState {
        match (self.active_side, &self.secondary_pane) {
            (PaneSide::Right, Some(secondary)) if self.dual_pane_active => secondary,
            _ => &self.pane,
        }
    }

    pub fn active_pane_mut(&mut self) -> &mut PaneState {
        match (self.active_side, self.dual_pane_active) {
            (PaneSide::Right, true) if self.secondary_pane.is_some() => {
                self.secondary_pane.as_mut().unwrap()
            }
            _ => &mut self.pane,
        }
    }

    /// The pane shown on `side`, if that side is shown at all.
    pub fn pane_on(&self, side: PaneSide) -> Option<&PaneState> {
        match side {
            PaneSide::Left => Some(&self.pane),
            PaneSide::Right if self.dual_pane_active => self.secondary_pane.as_ref(),
            PaneSide::Right => None,
        }
    }

    /// Which side a pane is on, if this tab shows it.
    pub fn side_of(&self, pane_id: u32) -> Option<PaneSide> {
        if self.pane.id == pane_id {
            Some(PaneSide::Left)
        } else if self.dual_pane_active
            && self.secondary_pane.as_ref().map(|p| p.id) == Some(pane_id)
        {
            Some(PaneSide::Right)
        } else {
            None
        }
    }

    /// Make the pane on `side` the active one. Choosing a side that is not
    /// shown falls back to the left.
    pub fn set_active_side(&mut self, side: PaneSide) {
        self.active_side = match side {
            PaneSide::Right if self.dual_pane_active && self.secondary_pane.is_some() => side,
            _ => PaneSide::Left,
        };
    }

    /// Open or close the second pane. Opening it for the first time starts
    /// it in the same directory as the first; reopening keeps where it was.
    /// Returns whether it is now open.
    pub fn toggle_dual_pane(&mut self) -> bool {
        if self.dual_pane_active {
            self.dual_pane_active = false;
            self.active_side = PaneSide::Left;
        } else {
            if self.secondary_pane.is_none() {
                self.secondary_pane =
                    Some(PaneState::new(self.id * 2 + 1, self.pane.current_path.clone()));
            }
            self.dual_pane_active = true;
        }
        self.dual_pane_active
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
    /// Context-menu commands from actions.toml.
    pub custom_actions: Vec<CustomAction>,
    /// Plugins the backend reports as loaded, id to name.
    pub loaded_plugins: HashMap<String, String>,
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
            custom_actions: custom_actions::load(),
            loaded_plugins: HashMap::new(),
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

    /// Move the tab at `from` so that it sits at `to`, keeping the same tab
    /// active. Returns `false` for an out-of-range or no-op move.
    pub fn move_tab(&mut self, from: usize, to: usize) -> bool {
        if from == to || from >= self.tabs.len() || to >= self.tabs.len() {
            return false;
        }
        let active_id = self.tabs[self.active_tab].id;
        let tab = self.tabs.remove(from);
        self.tabs.insert(to, tab);
        self.active_tab = self
            .tabs
            .iter()
            .position(|t| t.id == active_id)
            .unwrap_or(0);
        true
    }

    /// Switch to the tab `delta` steps away, wrapping around. Returns the
    /// active pane id afterwards.
    pub fn cycle_tab(&mut self, delta: i32) -> u32 {
        let len = self.tabs.len() as i32;
        if len > 0 {
            self.active_tab = ((self.active_tab as i32 + delta).rem_euclid(len)) as usize;
        }
        self.active_tab().active_pane().id
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

#[cfg(test)]
mod tests {
    use super::*;

    fn state() -> AppStateInner {
        AppStateInner::new(AppConfig::default())
    }

    #[test]
    fn dual_pane_toggles_and_routes_the_active_pane() {
        let mut s = state();
        let tab = s.active_tab_mut();
        let left_id = tab.pane.id;
        assert_eq!(tab.pane_on(PaneSide::Right).map(|p| p.id), None);

        assert!(tab.toggle_dual_pane());
        let right_id = tab.pane_on(PaneSide::Right).unwrap().id;
        assert_ne!(left_id, right_id);
        assert_eq!(tab.side_of(right_id), Some(PaneSide::Right));

        tab.set_active_side(PaneSide::Right);
        assert_eq!(tab.active_pane().id, right_id);
        tab.active_pane_mut().navigate_to(RavenPath::local("/tmp"));
        assert_eq!(tab.secondary_pane.as_ref().unwrap().current_path, RavenPath::local("/tmp"));

        // Closing the second pane makes the left one active again and hides it.
        assert!(!tab.toggle_dual_pane());
        assert_eq!(tab.active_pane().id, left_id);
        assert_eq!(tab.side_of(right_id), None);
        // Reopening keeps the right pane's directory.
        tab.toggle_dual_pane();
        assert_eq!(
            tab.pane_on(PaneSide::Right).unwrap().current_path,
            RavenPath::local("/tmp")
        );
    }

    #[test]
    fn tabs_move_and_keep_the_active_one() {
        let mut s = state();
        s.add_tab(RavenPath::local("/a"));
        s.add_tab(RavenPath::local("/b"));
        // Tabs: [home, /a, /b], active is /b (index 2).
        let active_id = s.active_tab().id;
        assert!(s.move_tab(2, 0));
        assert_eq!(s.active_tab, 0);
        assert_eq!(s.active_tab().id, active_id);
        assert!(!s.move_tab(1, 1));
        assert!(!s.move_tab(0, 9));

        let next = s.cycle_tab(1);
        assert_eq!(s.active_tab, 1);
        assert_eq!(next, s.active_tab().active_pane().id);
        s.cycle_tab(-1);
        assert_eq!(s.active_tab, 0);
        s.cycle_tab(-1);
        assert_eq!(s.active_tab, 2);
    }
}
