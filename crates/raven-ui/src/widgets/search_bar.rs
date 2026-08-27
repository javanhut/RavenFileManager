use std::rc::Rc;

use gtk4 as gtk;
use gtk::prelude::*;

use raven_core::commands::{AppCommand, SearchMode};
use raven_core::path::RavenPath;

/// Search bar with mode selector (filter/filename/content/smart search).
pub struct SearchBar {
    pub revealer: gtk::Revealer,
    entry: gtk::SearchEntry,
    mode_dropdown: gtk::DropDown,
    /// Close the bar and drop the filter it applied. Shared by the close button, the
    /// Escape key, and `hide()` so every dismissal path behaves identically.
    dismiss: Rc<dyn Fn()>,
}

impl SearchBar {
    pub fn new(
        command_tx: tokio::sync::mpsc::UnboundedSender<AppCommand>,
        get_current_path: impl Fn() -> Option<RavenPath> + 'static,
        pane_id: u32,
    ) -> Self {
        let revealer = gtk::Revealer::new();
        revealer.set_transition_type(gtk::RevealerTransitionType::SlideDown);

        let hbox = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        hbox.set_margin_start(8);
        hbox.set_margin_end(8);
        hbox.set_margin_top(4);
        hbox.set_margin_bottom(4);

        // Search mode dropdown
        let modes = gtk::StringList::new(&["Filter", "Filename", "Content", "Smart"]);
        let mode_dropdown = gtk::DropDown::new(Some(modes), gtk::Expression::NONE);
        mode_dropdown.set_selected(0);
        hbox.append(&mode_dropdown);

        // Search entry
        let entry = gtk::SearchEntry::new();
        entry.set_hexpand(true);
        entry.set_placeholder_text(Some("Search..."));
        hbox.append(&entry);

        // Dismissing the bar drops the filter it applied, so the listing returns to
        // the full directory instead of staying narrowed by a hidden query.
        let dismiss: Rc<dyn Fn()> = {
            let revealer = revealer.clone();
            let entry = entry.clone();
            let cmd_tx = command_tx.clone();
            Rc::new(move || {
                revealer.set_reveal_child(false);
                entry.set_text("");
                let _ = cmd_tx.send(AppCommand::SetFilter {
                    filter: raven_core::filter::FilterSpec::empty(),
                    pane_id,
                });
            })
        };

        // Close button
        let close_btn = gtk::Button::from_icon_name("window-close-symbolic");
        close_btn.add_css_class("flat");
        {
            let dismiss = dismiss.clone();
            close_btn.connect_clicked(move |_| dismiss());
        }
        hbox.append(&close_btn);

        revealer.set_child(Some(&hbox));

        // Handle search on Enter
        let cmd_tx = command_tx.clone();
        let mode = mode_dropdown.clone();
        entry.connect_activate(move |entry| {
            let query = entry.text().to_string();
            if query.is_empty() {
                // An emptied box clears the filter; returning early would strand it
                // with no way to undo from here.
                let _ = cmd_tx.send(AppCommand::SetFilter {
                    filter: raven_core::filter::FilterSpec::empty(),
                    pane_id,
                });
                return;
            }

            let search_mode = match mode.selected() {
                0 => {
                    // Filter mode — handled by sending SetFilter
                    let _ = cmd_tx.send(AppCommand::SetFilter {
                        filter: raven_core::filter::FilterSpec::with_query(&query),
                        pane_id,
                    });
                    return;
                }
                1 => SearchMode::Filename,
                2 => SearchMode::Content,
                3 => {
                    // Smart mode: parse NL query, decide filter vs recursive search
                    let parsed = raven_ai::nl_search::parse_nl_query(&query);
                    if parsed.needs_recursive {
                        if let Some(path) = get_current_path() {
                            let _ = cmd_tx.send(AppCommand::Search {
                                query,
                                path,
                                search_mode: SearchMode::NaturalLanguage,
                            });
                        }
                    } else {
                        let filter = raven_ai::nl_search::parsed_to_filter(&parsed);
                        let _ = cmd_tx.send(AppCommand::SetFilter {
                            filter,
                            pane_id,
                        });
                    }
                    return;
                }
                _ => SearchMode::Filename,
            };

            if let Some(path) = get_current_path() {
                let _ = cmd_tx.send(AppCommand::Search {
                    query,
                    path,
                    search_mode,
                });
            }
        });

        // Escape closes search bar
        let dismiss_for_key = dismiss.clone();
        let key_controller = gtk::EventControllerKey::new();
        key_controller.connect_key_pressed(move |_, key, _, _| {
            if key == gtk::gdk::Key::Escape {
                dismiss_for_key();
                return glib::Propagation::Stop;
            }
            glib::Propagation::Proceed
        });
        entry.add_controller(key_controller);

        Self {
            revealer,
            entry,
            mode_dropdown,
            dismiss,
        }
    }

    /// Empty the query text. Does not touch the applied filter — callers that clear
    /// pane state (navigation) reset both.
    pub fn clear(&self) {
        self.entry.set_text("");
    }

    pub fn show(&self) {
        self.revealer.set_reveal_child(true);
        self.entry.grab_focus();
    }

    /// Close the bar and drop its filter.
    pub fn hide(&self) {
        (self.dismiss)();
    }

    pub fn toggle(&self) {
        if self.revealer.reveals_child() {
            self.hide();
        } else {
            self.show();
        }
    }
}
