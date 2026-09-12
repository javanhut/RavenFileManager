use std::rc::Rc;

use gtk4 as gtk;
use gtk::prelude::*;

use raven_core::commands::AppCommand;
use raven_core::config::ViewMode;

use crate::state::{AppState, PaneResolver, PaneSide};
use crate::widgets::context_menu::FileContextMenu;
use crate::widgets::file_list::FileListView;
use crate::widgets::path_bar::PathBar;

/// One pane: a path bar over a file listing, plus its context menu.
pub struct PaneView {
    pub side: PaneSide,
    pub container: gtk::Box,
    pub file_list: Rc<FileListView>,
    pub path_bar: Rc<PathBar>,
    pub context_menu: Rc<FileContextMenu>,
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
        path_bar.container.set_margin_start(8);
        path_bar.container.set_margin_end(8);
        path_bar.container.set_margin_top(4);
        path_bar.container.set_margin_bottom(4);

        let file_list = Rc::new(FileListView::new(state, command_tx, resolver));

        let container = gtk::Box::new(gtk::Orientation::Vertical, 0);
        container.add_css_class("pane-box");
        container.set_hexpand(true);
        container.set_vexpand(true);
        container.append(&path_bar.container);
        container.append(&file_list.widget);

        let context_menu = Rc::new(FileContextMenu::new());
        context_menu.popover.set_parent(&file_list.widget);

        Self {
            side,
            container,
            file_list,
            path_bar,
            context_menu,
        }
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

/// The two panes side by side, and the bookkeeping of which one is active.
pub struct PaneViews {
    pub left: PaneView,
    pub right: PaneView,
    pub paned: gtk::Paned,
    state: AppState,
    command_tx: tokio::sync::mpsc::UnboundedSender<AppCommand>,
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
        let (visible, show_hidden, vcs, path) = {
            let s = self.state.borrow();
            let Some(pane) = s.pane_by_id(pane_id) else {
                return;
            };
            (
                pane.visible_entries(),
                s.show_hidden,
                pane.vcs_statuses.clone(),
                pane.current_path.clone(),
            )
        };
        view.file_list.set_entries(&visible, show_hidden, &vcs);
        view.path_bar.set_path(&path, &self.command_tx, pane_id);
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
    pub fn reload_all(&self) {
        let targets: Vec<(u32, _)> = {
            let s = self.state.borrow();
            self.visible_pane_ids()
                .into_iter()
                .filter_map(|id| s.pane_by_id(id).map(|p| (id, p.current_path.clone())))
                .collect()
        };
        for (pane_id, path) in targets {
            let _ = self.command_tx.send(AppCommand::Navigate { path, pane_id });
        }
    }

    /// Bring the panes in line with the active tab: layout, listings, and a
    /// fresh load of each shown directory.
    pub fn show_active_tab(&self) {
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
