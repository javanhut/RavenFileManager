//! "Other Application..." chooser for the Open With menu.
//!
//! Lists every installed application that wants to be shown, applications
//! registered for the file's content type first, with a search entry. The
//! dialog only picks; launching and changing the default are left to the
//! caller, which has the selection and somewhere to report errors.

use std::rc::Rc;

use gio::prelude::*;
use gtk4 as gtk;
use libadwaita as adw;
use adw::prelude::*;

/// Present the chooser for files of `content_type`. `on_chosen` runs once with
/// the picked application and whether "Always use for this file type" was
/// ticked; it is not called if the dialog is cancelled.
pub fn show_app_chooser(
    parent: &impl IsA<gtk::Window>,
    content_type: &str,
    on_chosen: impl Fn(gio::AppInfo, bool) + 'static,
) {
    let description = gio::content_type_get_description(content_type);
    let apps = sorted_apps(content_type);

    let window = adw::Window::builder()
        .modal(true)
        .transient_for(parent)
        .default_width(420)
        .default_height(520)
        .title("Open With")
        .build();

    let header = adw::HeaderBar::new();
    header.set_show_end_title_buttons(false);
    header.set_show_start_title_buttons(false);
    header.set_title_widget(Some(&adw::WindowTitle::new("Open With", &description)));
    let cancel = gtk::Button::with_label("Cancel");
    let open = gtk::Button::with_label("Open");
    open.add_css_class("suggested-action");
    open.set_sensitive(false);
    header.pack_start(&cancel);
    header.pack_end(&open);

    let search = gtk::SearchEntry::new();
    search.set_placeholder_text(Some("Search applications"));
    search.add_css_class("chooser-search");

    let list = gtk::ListBox::new();
    list.set_selection_mode(gtk::SelectionMode::Single);
    list.add_css_class("app-list");
    for app in &apps {
        list.append(&app_row(app));
    }
    let placeholder = gtk::Label::new(Some("No applications found"));
    placeholder.add_css_class("dim-label");
    placeholder.set_margin_top(24);
    list.set_placeholder(Some(&placeholder));

    let scrolled = gtk::ScrolledWindow::new();
    scrolled.set_vexpand(true);
    scrolled.set_hscrollbar_policy(gtk::PolicyType::Never);
    scrolled.set_child(Some(&list));

    let always = gtk::CheckButton::with_label(&format!("Always use for {}", description));
    // In a footer box rather than styled itself: padding on the check button
    // would move its box and label apart instead of away from the edges.
    let footer = gtk::Box::new(gtk::Orientation::Horizontal, 0);
    footer.add_css_class("chooser-footer");
    footer.append(&always);

    let content = gtk::Box::new(gtk::Orientation::Vertical, 0);
    content.append(&search);
    content.append(&scrolled);
    content.append(&footer);

    let toolbar = adw::ToolbarView::new();
    toolbar.add_top_bar(&header);
    toolbar.set_content(Some(&content));
    window.set_content(Some(&toolbar));

    let apps = Rc::new(apps);

    // Filter rows by the search text, matched against name and description.
    {
        let apps = apps.clone();
        let search_ref = search.clone();
        list.set_filter_func(move |row| {
            let query = search_ref.text();
            apps.get(row.index() as usize)
                .map(|app| app_matches(app, &query))
                .unwrap_or(false)
        });
    }
    {
        // Re-filter, then keep Open usable only while the selection is visible.
        let list = list.clone();
        let open = open.clone();
        search.connect_search_changed(move |_| {
            list.invalidate_filter();
            open.set_sensitive(list.selected_row().is_some_and(|r| r.is_child_visible()));
        });
    }
    {
        // The search entry has focus and binds Escape to stop-search itself, so
        // the window's key controller below never sees it; close from here.
        let window = window.clone();
        search.connect_stop_search(move |_| window.close());
    }

    {
        let open = open.clone();
        list.connect_row_selected(move |_, row| {
            open.set_sensitive(row.is_some_and(|r| r.is_child_visible()))
        });
    }

    let on_chosen = Rc::new(on_chosen);
    let choose = {
        let window = window.clone();
        let list = list.clone();
        let always = always.clone();
        let apps = apps.clone();
        move || {
            // A selected row the search has filtered out is not a choice.
            let Some(row) = list.selected_row().filter(|r| r.is_child_visible()) else {
                return;
            };
            let Some(app) = apps.get(row.index() as usize).cloned() else {
                return;
            };
            let always = always.is_active();
            window.close();
            on_chosen(app, always);
        }
    };

    {
        let choose = choose.clone();
        open.connect_clicked(move |_| choose());
    }
    {
        let choose = choose.clone();
        list.connect_row_activated(move |_, _| choose());
    }
    {
        // Enter in the search entry opens the first visible match.
        let list = list.clone();
        let choose = choose.clone();
        search.connect_activate(move |_| {
            if list.selected_row().is_none_or(|r| !r.is_child_visible()) {
                let mut i = 0;
                while let Some(row) = list.row_at_index(i) {
                    if row.is_child_visible() {
                        list.select_row(Some(&row));
                        break;
                    }
                    i += 1;
                }
            }
            choose();
        });
    }
    {
        let window = window.clone();
        cancel.connect_clicked(move |_| window.close());
    }
    {
        let window_ref = window.clone();
        let keys = gtk::EventControllerKey::new();
        keys.connect_key_pressed(move |_, key, _, _| {
            if key == gtk::gdk::Key::Escape {
                window_ref.close();
                return glib::Propagation::Stop;
            }
            glib::Propagation::Proceed
        });
        window.add_controller(keys);
    }

    window.present();
    search.grab_focus();
}

/// Applications worth offering: those registered for `content_type` first,
/// then everything else, each group alphabetical.
fn sorted_apps(content_type: &str) -> Vec<gio::AppInfo> {
    let registered: std::collections::HashSet<String> = gio::AppInfo::all_for_type(content_type)
        .iter()
        .filter_map(|a| a.id().map(|s| s.to_string()))
        .collect();
    let mut apps: Vec<(bool, String, gio::AppInfo)> = gio::AppInfo::all()
        .into_iter()
        .filter(|a| a.should_show())
        .map(|a| {
            let is_registered = a.id().is_some_and(|id| registered.contains(id.as_str()));
            (!is_registered, a.display_name().to_lowercase(), a)
        })
        .collect();
    apps.sort_by(|a, b| (a.0, &a.1).cmp(&(b.0, &b.1)));
    apps.into_iter().map(|(_, _, a)| a).collect()
}

fn app_matches(app: &gio::AppInfo, query: &str) -> bool {
    let description = app.description().map(|d| d.to_string()).unwrap_or_default();
    text_matches(&[&app.display_name(), &app.name(), &description], query)
}

/// Case-insensitive: every whitespace-separated word of `query` occurs in at
/// least one of `fields`. An empty query matches everything.
fn text_matches(fields: &[&str], query: &str) -> bool {
    let fields: Vec<String> = fields.iter().map(|f| f.to_lowercase()).collect();
    query
        .to_lowercase()
        .split_whitespace()
        .all(|word| fields.iter().any(|f| f.contains(word)))
}

fn app_row(app: &gio::AppInfo) -> gtk::ListBoxRow {
    let hbox = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    hbox.set_margin_top(6);
    hbox.set_margin_bottom(6);

    let image = match app.icon() {
        Some(icon) => gtk::Image::from_gicon(&icon),
        None => gtk::Image::from_icon_name("application-x-executable"),
    };
    image.set_pixel_size(32);
    hbox.append(&image);

    let label = gtk::Label::new(Some(&app.display_name()));
    label.set_halign(gtk::Align::Start);
    label.set_hexpand(true);
    label.set_ellipsize(gtk::pango::EllipsizeMode::End);
    if let Some(desc) = app.description() {
        label.set_tooltip_text(Some(&desc));
    }
    hbox.append(&label);

    let row = gtk::ListBoxRow::new();
    row.set_child(Some(&hbox));
    row
}

#[cfg(test)]
mod tests {
    use super::text_matches;

    #[test]
    fn empty_query_matches_all() {
        assert!(text_matches(&["Firefox"], ""));
        assert!(text_matches(&["Firefox"], "   "));
    }

    #[test]
    fn words_match_across_fields_case_insensitively() {
        let fields = ["Image Viewer", "eog", "Browse images"];
        assert!(text_matches(&fields, "image"));
        assert!(text_matches(&fields, "EOG browse"));
        assert!(!text_matches(&fields, "viewer video"));
    }
}
