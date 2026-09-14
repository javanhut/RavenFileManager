use std::cell::RefCell;
use std::rc::Rc;

use gtk4 as gtk;
use gtk::glib;
use gtk::prelude::*;

use raven_core::commands::AppCommand;
use raven_core::config::ViewMode;
use raven_core::path::RavenPath;

use crate::state::{AppState, PaneResolver, PaneSide};
use crate::widgets::context_menu::FileContextMenu;
use crate::widgets::file_list::FileListView;
use crate::widgets::path_bar::PathBar;

/// What a quick filter was typed against: a pane, its folder, and whether it
/// was showing Recent. Showing anything else ends the filter.
type FilterScope = (u32, RavenPath, bool);

/// One pane: a path bar over a file listing, plus its context menu.
pub struct PaneView {
    pub side: PaneSide,
    pub container: gtk::Box,
    pub file_list: Rc<FileListView>,
    pub path_bar: Rc<PathBar>,
    pub context_menu: Rc<FileContextMenu>,
    /// Quick filter: opened by typing into the listing, narrows it by name.
    filter_bar: gtk::Box,
    filter_entry: gtk::SearchEntry,
    filter_scope: Rc<RefCell<Option<FilterScope>>>,
}

impl PaneView {
    fn new(
        side: PaneSide,
        state: AppState,
        command_tx: tokio::sync::mpsc::UnboundedSender<AppCommand>,
    ) -> Self {
        // The pane this view shows changes with the active tab.
        let resolver: PaneResolver = {
            let state = state.clone();
            Rc::new(move || {
                let s = state.borrow();
                let tab = s.active_tab();
                tab.pane_on(side).unwrap_or(&tab.pane).id
            })
        };

        let path_bar = Rc::new(PathBar::new(command_tx.clone(), resolver.clone()));
        path_bar.container.set_margin_start(10);
        path_bar.container.set_margin_end(10);
        path_bar.container.set_margin_top(8);
        path_bar.container.set_margin_bottom(6);

        let file_list = Rc::new(FileListView::new(
            state.clone(),
            command_tx,
            resolver.clone(),
        ));

        // --- Quick filter bar, hidden until typed into ---
        let filter_entry = gtk::SearchEntry::new();
        filter_entry.set_hexpand(true);
        filter_entry.set_placeholder_text(Some("Filter this folder"));
        let filter_bar = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        filter_bar.add_css_class("quick-filter");
        filter_bar.set_margin_start(10);
        filter_bar.set_margin_end(10);
        filter_bar.set_margin_bottom(6);
        filter_bar.append(&filter_entry);
        filter_bar.set_visible(false);
        let filter_scope: Rc<RefCell<Option<FilterScope>>> = Rc::new(RefCell::new(None));

        let no_matches = gtk::Label::new(Some("No matches"));
        no_matches.add_css_class("no-matches");
        no_matches.set_halign(gtk::Align::Center);
        no_matches.set_valign(gtk::Align::Center);
        no_matches.set_can_target(false);
        no_matches.set_visible(false);

        let overlay = gtk::Overlay::new();
        overlay.set_hexpand(true);
        overlay.set_vexpand(true);
        overlay.set_child(Some(&file_list.widget));
        overlay.add_overlay(&no_matches);

        let container = gtk::Box::new(gtk::Orientation::Vertical, 0);
        container.add_css_class("pane-box");
        container.set_hexpand(true);
        container.set_vexpand(true);
        container.append(&path_bar.container);
        container.append(&filter_bar);
        container.append(&overlay);

        let context_menu = Rc::new(FileContextMenu::new());
        context_menu.popover.set_parent(&file_list.widget);

        // "No matches" follows the filtered list, which also changes when the
        // folder reloads under an active filter.
        {
            let list = file_list.clone();
            let label = no_matches.clone();
            file_list.filtered.connect_items_changed(move |model, _, _, _| {
                label.set_visible(list.is_quick_filtered() && model.n_items() == 0);
            });
        }

        {
            let list = file_list.clone();
            let bar = filter_bar.clone();
            let label = no_matches.clone();
            filter_entry.connect_changed(move |entry| {
                let text = entry.text();
                list.set_quick_filter(&text);
                label.set_visible(!text.is_empty() && list.filtered.n_items() == 0);
                // Deleting the last character closes the bar, so Space and the
                // other plain-key shortcuts reach the listing again.
                if text.is_empty() && bar.is_visible() {
                    bar.set_visible(false);
                    list.focus_view();
                }
            });
        }
        {
            let list = file_list.clone();
            let bar = filter_bar.clone();
            filter_entry.connect_stop_search(move |entry| {
                close_filter(&bar, entry);
                list.focus_view();
            });
        }
        // Enter, or Down, hands the keyboard to the matches with the first one
        // selected, so the next Enter opens it.
        {
            let list = file_list.clone();
            filter_entry.connect_activate(move |_| focus_first_match(&list));
        }
        {
            let list = file_list.clone();
            let keys = gtk::EventControllerKey::new();
            // Capture, so the entry's inner text widget cannot take Down first.
            keys.set_propagation_phase(gtk::PropagationPhase::Capture);
            keys.connect_key_pressed(move |_, key, _, _| {
                if key == gtk::gdk::Key::Down {
                    focus_first_match(&list);
                    return glib::Propagation::Stop;
                }
                glib::Propagation::Proceed
            });
            filter_entry.add_controller(keys);
        }

        // Typing a character into the listing starts (or extends) the filter.
        // Capture phase, so the list widgets never see the keys it takes; keys
        // bound to a shortcut, like Space for the preview, are left alone.
        {
            let bar = filter_bar.clone();
            let entry = filter_entry.clone();
            let scope = filter_scope.clone();
            let state = state.clone();
            let keys = gtk::EventControllerKey::new();
            keys.set_propagation_phase(gtk::PropagationPhase::Capture);
            keys.connect_key_pressed(move |_, key, _, modifiers| {
                use gtk::gdk::ModifierType;
                // Escape ends the filter from the listing too, after Enter or
                // Down has handed it the keyboard.
                if key == gtk::gdk::Key::Escape && bar.is_visible() {
                    close_filter(&bar, &entry);
                    scope.borrow_mut().take();
                    return glib::Propagation::Stop;
                }
                if modifiers
                    .intersects(ModifierType::CONTROL_MASK | ModifierType::ALT_MASK | ModifierType::SUPER_MASK)
                {
                    return glib::Propagation::Proceed;
                }
                let Some(ch) = key.to_unicode().filter(|c| starts_filter(*c)) else {
                    return glib::Propagation::Proceed;
                };
                let held: &[&str] = if modifiers.contains(ModifierType::SHIFT_MASK) {
                    &["Shift"]
                } else {
                    &[]
                };
                let bound = key.name().is_some_and(|name| {
                    state
                        .borrow()
                        .config
                        .keybindings
                        .action_for(&name, held)
                        .is_some()
                });
                if bound {
                    return glib::Propagation::Proceed;
                }

                if !bar.is_visible() {
                    let pane_id = resolver();
                    *scope.borrow_mut() = state
                        .borrow()
                        .pane_by_id(pane_id)
                        .map(|p| (pane_id, p.current_path.clone(), p.recent));
                    bar.set_visible(true);
                }
                let mut text = entry.text().to_string();
                text.push(ch);
                entry.set_text(&text);
                entry.grab_focus();
                entry.set_position(-1);
                glib::Propagation::Stop
            });
            file_list.widget.add_controller(keys);
        }

        Self {
            side,
            container,
            file_list,
            path_bar,
            context_menu,
            filter_bar,
            filter_entry,
            filter_scope,
        }
    }

    /// Drop the quick filter without moving the keyboard.
    pub fn clear_quick_filter(&self) {
        close_filter(&self.filter_bar, &self.filter_entry);
        self.filter_scope.borrow_mut().take();
    }

    /// The three list widgets, for attaching gestures and action groups.
    pub fn list_widgets(&self) -> [gtk::Widget; 3] {
        [
            self.file_list.column_view.clone().upcast(),
            self.file_list.icon_grid_view.clone().upcast(),
            self.file_list.preview_grid_view.clone().upcast(),
        ]
    }
}

/// Whether typing `ch` into a listing starts a quick filter: printable
/// characters, but not whitespace, which no file name search starts with.
pub fn starts_filter(ch: char) -> bool {
    !ch.is_control() && !ch.is_whitespace()
}

/// Hide the filter bar and empty it. The bar goes first, so the entry's
/// change handler sees a closed bar and leaves the keyboard where it is.
fn close_filter(bar: &gtk::Box, entry: &gtk::SearchEntry) {
    bar.set_visible(false);
    if !entry.text().is_empty() {
        entry.set_text("");
    }
}

/// Select the first row the filter left and give the listing the keyboard.
fn focus_first_match(list: &FileListView) {
    if list.selection.n_items() > 0 {
        list.selection.select_item(0, true);
    }
    list.focus_view();
}

/// The two panes side by side, and the bookkeeping of which one is active.
pub struct PaneViews {
    pub left: PaneView,
    pub right: PaneView,
    pub paned: gtk::Paned,
    state: AppState,
    command_tx: tokio::sync::mpsc::UnboundedSender<AppCommand>,
    /// Bumped by every Recent load, so a slower earlier one cannot land last.
    recent_generation: std::cell::Cell<u64>,
}

impl PaneViews {
    pub fn new(
        state: AppState,
        command_tx: tokio::sync::mpsc::UnboundedSender<AppCommand>,
    ) -> Rc<Self> {
        let left = PaneView::new(PaneSide::Left, state.clone(), command_tx.clone());
        let right = PaneView::new(PaneSide::Right, state.clone(), command_tx.clone());

        let paned = gtk::Paned::new(gtk::Orientation::Horizontal);
        paned.set_hexpand(true);
        paned.set_vexpand(true);
        paned.set_start_child(Some(&left.container));
        paned.set_end_child(Some(&right.container));
        paned.set_resize_start_child(true);
        paned.set_resize_end_child(true);
        paned.set_shrink_start_child(false);
        paned.set_shrink_end_child(false);
        right.container.set_visible(false);

        let views = Rc::new(Self {
            left,
            right,
            paned,
            state,
            command_tx,
            recent_generation: std::cell::Cell::new(0),
        });

        // Focus landing anywhere in a pane makes it the active one.
        for side in [PaneSide::Left, PaneSide::Right] {
            let focus = gtk::EventControllerFocus::new();
            let views_ref = views.clone();
            focus.connect_enter(move |_| views_ref.set_active_side(side));
            views.view(side).container.add_controller(focus);

            // A click counts too, since not every widget takes focus.
            let click = gtk::GestureClick::new();
            click.set_button(0);
            click.set_propagation_phase(gtk::PropagationPhase::Capture);
            let views_ref = views.clone();
            click.connect_pressed(move |_, _, _, _| views_ref.set_active_side(side));
            views.view(side).container.add_controller(click);
        }

        views.sync_layout();
        views
    }

    pub fn view(&self, side: PaneSide) -> &PaneView {
        match side {
            PaneSide::Left => &self.left,
            PaneSide::Right => &self.right,
        }
    }

    pub fn active_side(&self) -> PaneSide {
        self.state.borrow().active_tab().active_side
    }

    pub fn active(&self) -> &PaneView {
        self.view(self.active_side())
    }

    pub fn active_pane_id(&self) -> u32 {
        self.state.borrow().active_tab().active_pane().id
    }

    /// The view showing `pane_id`, if the active tab shows it.
    pub fn view_for_pane(&self, pane_id: u32) -> Option<&PaneView> {
        let side = self.state.borrow().active_tab().side_of(pane_id)?;
        Some(self.view(side))
    }

    pub fn is_dual(&self) -> bool {
        self.state.borrow().active_tab().dual_pane_active
    }

    /// Make `side` the active pane and show it as such.
    pub fn set_active_side(&self, side: PaneSide) {
        let changed = {
            let mut s = self.state.borrow_mut();
            let tab = s.active_tab_mut();
            let before = tab.active_side;
            tab.set_active_side(side);
            before != tab.active_side
        };
        self.paint_active();
        if changed {
            let (pane_id, path) = {
                let s = self.state.borrow();
                let pane = s.active_tab().active_pane();
                (pane.id, pane.current_path.clone())
            };
            let _ = self
                .command_tx
                .send(AppCommand::RefreshTagCounts { pane_id, path });
        }
    }

    /// Give the keyboard to the active pane's listing.
    pub fn focus_active(&self) {
        self.active()
            .file_list
            .widget
            .child_focus(gtk::DirectionType::TabForward);
    }

    fn paint_active(&self) {
        let dual = self.is_dual();
        let active = self.active_side();
        for side in [PaneSide::Left, PaneSide::Right] {
            let container = &self.view(side).container;
            if dual && side == active {
                container.add_css_class("active-pane");
            } else {
                container.remove_css_class("active-pane");
            }
        }
        if dual {
            self.paned.add_css_class("dual-pane");
        } else {
            self.paned.remove_css_class("dual-pane");
        }
    }

    /// Show or hide the right pane to match the active tab.
    pub fn sync_layout(&self) {
        let dual = self.is_dual();
        self.right.container.set_visible(dual);
        if dual && self.paned.position() <= 0 {
            let width = self.paned.width();
            if width > 0 {
                self.paned.set_position(width / 2);
            }
        }
        self.paint_active();
    }

    /// Repaint a pane's listing and path bar from state. Nothing happens for
    /// a pane the active tab does not show.
    pub fn render(&self, pane_id: u32) {
        let Some(view) = self.view_for_pane(pane_id) else {
            return;
        };
        let (visible, show_hidden, vcs, path, recent) = {
            let s = self.state.borrow();
            let Some(pane) = s.pane_by_id(pane_id) else {
                return;
            };
            (
                pane.visible_entries(),
                s.show_hidden,
                pane.vcs_statuses.clone(),
                pane.current_path.clone(),
                pane.recent,
            )
        };
        self.sync_quick_filter(pane_id);
        view.file_list.set_entries(&visible, show_hidden, &vcs);
        if recent {
            view.path_bar.set_label(RECENT_ICON, "Recent");
        } else {
            view.path_bar.set_path(&path, &self.command_tx, pane_id);
        }
    }

    /// End the quick filter of the view showing `pane_id` if that view now
    /// shows something other than what the filter was typed against: another
    /// folder, another tab's pane, or Recent. A reload of the same folder
    /// keeps it.
    pub fn sync_quick_filter(&self, pane_id: u32) {
        let Some(view) = self.view_for_pane(pane_id) else {
            return;
        };
        let current: Option<FilterScope> = self
            .state
            .borrow()
            .pane_by_id(pane_id)
            .map(|p| (pane_id, p.current_path.clone(), p.recent));
        let stale = view
            .filter_scope
            .borrow()
            .as_ref()
            .is_some_and(|scope| Some(scope) != current.as_ref());
        if stale {
            view.clear_quick_filter();
        }
    }

    /// List recently used files in `pane_id`, in place of its folder.
    ///
    /// The pane switches to Recent at once; the files are checked for
    /// existence on a worker thread (a stalled mount would otherwise freeze
    /// the window) and the listing fills in when that is done, after which
    /// `on_loaded` gets the count. A reload keeps the old list meanwhile.
    pub fn show_recent(self: &Rc<Self>, pane_id: u32, on_loaded: impl FnOnce(usize) + 'static) {
        let switching = {
            let mut s = self.state.borrow_mut();
            for tab in s.tabs.iter_mut() {
                let owns = tab.pane.id == pane_id
                    || tab.secondary_pane.as_ref().is_some_and(|p| p.id == pane_id);
                if owns {
                    tab.title = "Recent".to_string();
                }
            }
            match s.pane_by_id_mut(pane_id) {
                Some(pane) if !pane.recent => {
                    pane.show_recent(Vec::new());
                    true
                }
                Some(_) => false,
                None => return,
            }
        };
        if switching {
            self.render(pane_id);
        }

        let generation = self.recent_generation.get() + 1;
        self.recent_generation.set(generation);
        let candidates = crate::recent::recent_candidates();
        let views = Rc::downgrade(self);
        glib::spawn_future_local(async move {
            let Ok(entries) =
                gio::spawn_blocking(move || crate::recent::recent_entries(candidates)).await
            else {
                return;
            };
            let Some(views) = views.upgrade() else {
                return;
            };
            if views.recent_generation.get() != generation {
                return;
            }
            let count = entries.len();
            {
                let mut s = views.state.borrow_mut();
                // Left Recent while the files were being checked.
                match s.pane_by_id_mut(pane_id) {
                    Some(pane) if pane.recent => pane.entries = entries,
                    _ => return,
                }
            }
            views.render(pane_id);
            on_loaded(count);
        });
    }

    /// Every pane the active tab shows.
    pub fn visible_pane_ids(&self) -> Vec<u32> {
        let s = self.state.borrow();
        let tab = s.active_tab();
        [PaneSide::Left, PaneSide::Right]
            .into_iter()
            .filter_map(|side| tab.pane_on(side).map(|p| p.id))
            .collect()
    }

    /// Repaint both panes, after a change that affects every listing.
    pub fn render_all(&self) {
        for pane_id in self.visible_pane_ids() {
            self.render(pane_id);
        }
    }

    /// Reload every shown pane's directory from the backend.
    pub fn reload_all(self: &Rc<Self>) {
        for pane_id in self.visible_pane_ids() {
            self.reload(pane_id);
        }
    }

    /// Reload one pane: its directory from the backend, or, while it shows
    /// Recent, the recent list -- a reload is not a request to leave it.
    pub fn reload(self: &Rc<Self>, pane_id: u32) {
        let target = self
            .state
            .borrow()
            .pane_by_id(pane_id)
            .map(|p| (p.recent, p.current_path.clone()));
        match target {
            Some((true, _)) => self.show_recent(pane_id, |_| {}),
            Some((false, path)) => {
                let _ = self.command_tx.send(AppCommand::Navigate { path, pane_id });
            }
            None => {}
        }
    }

    /// Bring the panes in line with the active tab: layout, listings, and a
    /// fresh load of each shown directory.
    pub fn show_active_tab(self: &Rc<Self>) {
        self.sync_layout();
        self.render_all();
        self.reload_all();
    }

    /// Open or close the second pane of the active tab.
    pub fn toggle_dual(&self) {
        let (now_dual, right_id) = {
            let mut s = self.state.borrow_mut();
            let tab = s.active_tab_mut();
            let now = tab.toggle_dual_pane();
            (now, tab.pane_on(PaneSide::Right).map(|p| p.id))
        };
        self.sync_layout();
        if now_dual {
            if let Some(id) = right_id {
                self.render(id);
                let path = self
                    .state
                    .borrow()
                    .pane_by_id(id)
                    .map(|p| p.current_path.clone());
                if let Some(path) = path {
                    let _ = self
                        .command_tx
                        .send(AppCommand::Navigate { path, pane_id: id });
                }
            }
        }
        self.focus_active();
    }

    /// Move keyboard focus to the other pane.
    pub fn switch_pane(&self) {
        if !self.is_dual() {
            return;
        }
        let other = self.active_side().other();
        self.set_active_side(other);
        self.focus_active();
    }

    pub fn set_view_mode(&self, mode: ViewMode) {
        self.left.file_list.set_view_mode(mode);
        self.right.file_list.set_view_mode(mode);
    }
}

/// The icon Recent is shown with, in the sidebar and the path bar.
pub const RECENT_ICON: &str = "document-open-recent-symbolic";

#[cfg(test)]
mod tests {
    use super::starts_filter;

    #[test]
    fn printable_characters_start_a_filter() {
        assert!(starts_filter('a'));
        assert!(starts_filter('Ä'));
        assert!(starts_filter('.'));
        assert!(starts_filter('7'));
        assert!(!starts_filter(' '));
        assert!(!starts_filter('\t'));
        assert!(!starts_filter('\u{8}'));
        assert!(!starts_filter('\u{7f}'));
    }
}
