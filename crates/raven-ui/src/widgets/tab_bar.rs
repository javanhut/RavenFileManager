use std::cell::RefCell;
use std::rc::Rc;

use gtk4 as gtk;
use gtk::prelude::*;

use raven_core::commands::AppCommand;

use crate::state::AppState;

/// A tab bar showing open tabs with close buttons and new-tab button.
pub struct TabBar {
    pub widget: gtk::Box,
    tabs_box: gtk::Box,
    state: AppState,
    command_tx: tokio::sync::mpsc::UnboundedSender<AppCommand>,
    on_tab_changed: Rc<RefCell<Option<Box<dyn Fn(u32)>>>>,
}

impl TabBar {
    pub fn new(
        state: AppState,
        command_tx: tokio::sync::mpsc::UnboundedSender<AppCommand>,
    ) -> Self {
        let widget = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        widget.add_css_class("tab-bar");

        // Separate pills rather than a linked strip: Raven draws a choice of
        // equals as items with air between them.
        let tabs_box = gtk::Box::new(gtk::Orientation::Horizontal, 4);
        tabs_box.set_hexpand(true);
        widget.append(&tabs_box);

        // New tab button
        let new_tab_btn = gtk::Button::from_icon_name("tab-new-symbolic");
        new_tab_btn.set_tooltip_text(Some("New Tab (Ctrl+T)"));
        new_tab_btn.add_css_class("flat");
        new_tab_btn.add_css_class("tab-new");
        {
            let state = state.clone();
            let cmd_tx = command_tx.clone();
            new_tab_btn.connect_clicked(move |_| open_new_tab(&state, &cmd_tx));
        }
        widget.append(&new_tab_btn);
        // Hidden until a second tab exists (see `refresh`).
        widget.set_visible(false);

        Self {
            widget,
            tabs_box,
            state,
            command_tx,
            on_tab_changed: Rc::new(RefCell::new(None)),
        }
    }

    /// A header-bar button that opens a tab, for while the strip (and its own
    /// "+") is hidden.
    pub fn new_tab_button(&self) -> gtk::Button {
        let btn = gtk::Button::from_icon_name("tab-new-symbolic");
        btn.add_css_class("flat");
        btn.set_tooltip_text(Some("New Tab (Ctrl+T)"));
        let state = self.state.clone();
        let cmd_tx = self.command_tx.clone();
        btn.connect_clicked(move |_| open_new_tab(&state, &cmd_tx));
        btn
    }

    pub fn set_on_tab_changed(&self, callback: impl Fn(u32) + 'static) {
        *self.on_tab_changed.borrow_mut() = Some(Box::new(callback));
    }

    /// Rebuild the tab buttons from current state.
    pub fn refresh(&self) {
        while let Some(child) = self.tabs_box.first_child() {
            self.tabs_box.remove(&child);
        }

        let s = self.state.borrow();
        let active_tab = s.active_tab;
        let tab_count = s.tabs.len();
        // A strip holding one tab is a whole row spent on nothing: the other
        // Raven apps start their content right under the header bar, so the
        // strip shows only once there is a second tab to switch to.
        self.widget.set_visible(tab_count > 1);

        for (idx, tab) in s.tabs.iter().enumerate() {
            let tab_box = gtk::Box::new(gtk::Orientation::Horizontal, 4);

            let label = gtk::Label::new(Some(&tab.title));
            label.set_ellipsize(gtk::pango::EllipsizeMode::End);
            label.set_max_width_chars(20);
            tab_box.append(&label);

            // Close button (only if more than one tab)
            if s.tabs.len() > 1 {
                let close_btn = gtk::Button::from_icon_name("window-close-symbolic");
                close_btn.add_css_class("tab-close");
                close_btn.set_valign(gtk::Align::Center);

                let state = self.state.clone();
                let tab_idx = idx;
                close_btn.connect_clicked(move |_| {
                    let mut s = state.borrow_mut();
                    s.close_tab(tab_idx);
                });
                tab_box.append(&close_btn);
            }

            let btn = gtk::ToggleButton::new();
            btn.add_css_class("tab");
            if tab_count == 1 {
                // No close button, so no room kept for one.
                btn.add_css_class("single");
            }
            btn.set_child(Some(&tab_box));
            btn.set_active(idx == active_tab);

            let state = self.state.clone();
            let cmd_tx = self.command_tx.clone();
            let on_changed = self.on_tab_changed.clone();
            let tab_idx = idx;
            btn.connect_toggled(move |btn| {
                if btn.is_active() {
                    let mut s = state.borrow_mut();
                    s.active_tab = tab_idx;
                    let pane_id = s.active_tab().active_pane().id;
                    drop(s);
                    // The window reloads the tab's panes; sending a navigate here
                    // as well would load the active one twice.
                    let _ = &cmd_tx;
                    if let Some(ref cb) = *on_changed.borrow() {
                        cb(pane_id);
                    }
                }
            });

            self.attach_drag_and_drop(&btn, idx, tab_count);
            self.tabs_box.append(&btn);
        }
    }

    /// Tabs can be dragged onto each other to reorder, and files dragged from
    /// a listing can be dropped on a tab to move them into its directory.
    fn attach_drag_and_drop(&self, btn: &gtk::ToggleButton, idx: usize, tab_count: usize) {
        if tab_count > 1 {
            let source = gtk::DragSource::new();
            source.set_actions(gtk::gdk::DragAction::MOVE);
            source.connect_prepare(move |_, _, _| {
                Some(gtk::gdk::ContentProvider::for_value(
                    &format!("{}{}", TAB_DRAG_PREFIX, idx).to_value(),
                ))
            });
            {
                let btn = btn.clone();
                source.connect_drag_begin(move |source, _| {
                    let paintable = gtk::WidgetPaintable::new(Some(&btn));
                    source.set_icon(Some(&paintable), 0, 0);
                });
            }
            btn.add_controller(source);
        }

        // Tab drags only offer MOVE, so the shared copy/move feedback shows a
        // move for them; files get copy or move by modifier and filesystem.
        let target = {
            let state = self.state.clone();
            crate::dnd::file_drop_target(Some("drop-target"), move || {
                state.borrow().tabs.get(idx).map(|t| t.active_pane().current_path.clone())
            })
        };
        {
            let state = self.state.clone();
            let cmd_tx = self.command_tx.clone();
            let on_changed = self.on_tab_changed.clone();
            let tabs_box = self.tabs_box.clone();
            let btn = btn.clone();
            target.connect_drop(move |target, value, _, _| {
                btn.remove_css_class("drop-target");
                let drop = match value.get::<String>() {
                    Ok(text) => parse_tab_drop(&text),
                    Err(_) => TabDrop::Files,
                };
                match drop {
                    TabDrop::Tab(from) => {
                        let moved = state.borrow_mut().move_tab(from, idx);
                        if moved {
                            // Re-rendering replaces this very button, which is
                            // fine once the drop handler has returned.
                            let state = state.clone();
                            let cmd_tx = cmd_tx.clone();
                            let on_changed = on_changed.clone();
                            let tabs_box = tabs_box.clone();
                            glib::idle_add_local_once(move || {
                                rebuild_tabs(&tabs_box, &state, &cmd_tx, &on_changed);
                            });
                        }
                        moved
                    }
                    TabDrop::Files => {
                        let destination = {
                            let s = state.borrow();
                            s.tabs.get(idx).map(|t| t.active_pane().current_path.clone())
                        };
                        let Some(destination) = destination else {
                            return false;
                        };
                        crate::dnd::perform_drop(target, value, &destination, &cmd_tx)
                    }
                    TabDrop::Nothing => false,
                }
            });
        }
        btn.add_controller(target);
    }
}

/// Open a tab on the active pane's folder.
fn open_new_tab(state: &AppState, cmd_tx: &tokio::sync::mpsc::UnboundedSender<AppCommand>) {
    let path = state.borrow().active_tab().active_pane().current_path.clone();
    let mut s = state.borrow_mut();
    let _tab_id = s.add_tab(path.clone());
    let pane_id = s.active_tab().active_pane().id;
    drop(s);
    let _ = cmd_tx.send(AppCommand::Navigate { path, pane_id });
}

const TAB_DRAG_PREFIX: &str = "raven-tab:";

/// What landed on a tab.
#[derive(Debug, PartialEq, Eq)]
enum TabDrop {
    /// Another tab, by its index before the move.
    Tab(usize),
    /// Files, as a file list or URI text; `dnd::perform_drop` reads them.
    Files,
    Nothing,
}

/// Classify a string dropped on a tab. Non-string values (file lists) are
/// always files.
fn parse_tab_drop(text: &str) -> TabDrop {
    if let Some(rest) = text.strip_prefix(TAB_DRAG_PREFIX) {
        return match rest.trim().parse::<usize>() {
            Ok(idx) => TabDrop::Tab(idx),
            Err(_) => TabDrop::Nothing,
        };
    }
    if crate::dnd::parse_uri_list(text).is_empty() {
        TabDrop::Nothing
    } else {
        TabDrop::Files
    }
}

/// `TabBar::refresh` without a `TabBar`: the drop handler outlives the
/// button it is attached to, so it re-renders through the pieces it holds.
fn rebuild_tabs(
    tabs_box: &gtk::Box,
    state: &AppState,
    command_tx: &tokio::sync::mpsc::UnboundedSender<AppCommand>,
    on_tab_changed: &Rc<RefCell<Option<Box<dyn Fn(u32)>>>>,
) {
    let bar = TabBar {
        widget: gtk::Box::new(gtk::Orientation::Horizontal, 0),
        tabs_box: tabs_box.clone(),
        state: state.clone(),
        command_tx: command_tx.clone(),
        on_tab_changed: on_tab_changed.clone(),
    };
    bar.refresh();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tab_drops_are_told_apart_from_file_drops() {
        assert_eq!(parse_tab_drop("raven-tab:2"), TabDrop::Tab(2));
        assert_eq!(parse_tab_drop("raven-tab:x"), TabDrop::Nothing);
        assert_eq!(
            parse_tab_drop("file:///a/b.txt\r\nfile:///a/c%20d.txt\r\n"),
            TabDrop::Files
        );
        assert_eq!(parse_tab_drop("hello"), TabDrop::Nothing);
    }
}
