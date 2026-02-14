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

        let tabs_box = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        tabs_box.add_css_class("linked");
        tabs_box.set_hexpand(true);
        widget.append(&tabs_box);

        // New tab button
        let new_tab_btn = gtk::Button::from_icon_name("tab-new-symbolic");
        new_tab_btn.set_tooltip_text(Some("New Tab (Ctrl+T)"));
        new_tab_btn.add_css_class("flat");
        {
            let state = state.clone();
            let cmd_tx = command_tx.clone();
            new_tab_btn.connect_clicked(move |_| {
                let path = {
                    let s = state.borrow();
                    s.active_tab().active_pane().current_path.clone()
                };
                let mut s = state.borrow_mut();
                let _tab_id = s.add_tab(path.clone());
                let pane_id = s.active_tab().active_pane().id;
                drop(s);
                let _ = cmd_tx.send(AppCommand::Navigate {
                    path,
                    pane_id,
                });
            });
        }
        widget.append(&new_tab_btn);

        Self {
            widget,
            tabs_box,
            state,
            command_tx,
            on_tab_changed: Rc::new(RefCell::new(None)),
        }
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

        for (idx, tab) in s.tabs.iter().enumerate() {
            let tab_box = gtk::Box::new(gtk::Orientation::Horizontal, 4);

            let label = gtk::Label::new(Some(&tab.title));
            label.set_ellipsize(gtk::pango::EllipsizeMode::End);
            label.set_max_width_chars(20);
            tab_box.append(&label);

            // Close button (only if more than one tab)
            if s.tabs.len() > 1 {
                let close_btn = gtk::Button::from_icon_name("window-close-symbolic");
                close_btn.add_css_class("flat");
                close_btn.add_css_class("circular");
                close_btn.set_margin_start(4);

                let state = self.state.clone();
                let tab_idx = idx;
                close_btn.connect_clicked(move |_| {
                    let mut s = state.borrow_mut();
                    s.close_tab(tab_idx);
                });
                tab_box.append(&close_btn);
            }

            let btn = gtk::ToggleButton::new();
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
                    let path = s.active_tab().active_pane().current_path.clone();
                    drop(s);
                    let _ = cmd_tx.send(AppCommand::Navigate {
                        path,
                        pane_id,
                    });
                    if let Some(ref cb) = *on_changed.borrow() {
                        cb(pane_id);
                    }
                }
            });

            self.tabs_box.append(&btn);
        }
    }
}
