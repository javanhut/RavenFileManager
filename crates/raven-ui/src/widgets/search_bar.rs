use gtk4 as gtk;
use gtk::prelude::*;

use raven_core::commands::{AppCommand, SearchMode};
use raven_core::path::RavenPath;

/// Search bar with mode selector (filter/filename/content/smart search).
pub struct SearchBar {
    pub revealer: gtk::Revealer,
    entry: gtk::SearchEntry,
    mode_dropdown: gtk::DropDown,
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

        // Close button
        let close_btn = gtk::Button::from_icon_name("window-close-symbolic");
        close_btn.add_css_class("flat");
        {
            let revealer = revealer.clone();
            close_btn.connect_clicked(move |_| {
                revealer.set_reveal_child(false);
            });
        }
        hbox.append(&close_btn);

        revealer.set_child(Some(&hbox));

        // Handle search on Enter
        let cmd_tx = command_tx.clone();
        let mode = mode_dropdown.clone();
        entry.connect_activate(move |entry| {
            let query = entry.text().to_string();
            if query.is_empty() {
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
        let revealer_for_key = revealer.clone();
        let key_controller = gtk::EventControllerKey::new();
        key_controller.connect_key_pressed(move |_, key, _, _| {
            if key == gtk::gdk::Key::Escape {
                revealer_for_key.set_reveal_child(false);
                return glib::Propagation::Stop;
            }
            glib::Propagation::Proceed
        });
        entry.add_controller(key_controller);

        Self {
            revealer,
            entry,
            mode_dropdown,
        }
    }

    pub fn show(&self) {
        self.revealer.set_reveal_child(true);
        self.entry.grab_focus();
    }

    pub fn hide(&self) {
        self.revealer.set_reveal_child(false);
    }

    pub fn toggle(&self) {
        if self.revealer.reveals_child() {
            self.hide();
        } else {
            self.show();
        }
    }
}
