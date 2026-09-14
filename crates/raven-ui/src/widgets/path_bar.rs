use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

use gtk4 as gtk;
use gtk::prelude::*;

use raven_core::commands::AppCommand;
use raven_core::path::RavenPath;

use crate::state::PaneResolver;

/// A path bar that shows breadcrumb buttons or an editable text entry (toggled with Ctrl+L).
pub struct PathBar {
    pub container: gtk::Box,
    breadcrumb_box: gtk::Box,
    entry: gtk::Entry,
    edit_mode: Rc<RefCell<bool>>,
}

impl PathBar {
    pub fn new(
        command_tx: tokio::sync::mpsc::UnboundedSender<AppCommand>,
        pane: PaneResolver,
    ) -> Self {
        let container = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        container.add_css_class("path-bar");
        container.set_hexpand(true);

        let breadcrumb_box = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        breadcrumb_box.set_hexpand(true);

        let entry = gtk::Entry::new();
        entry.set_hexpand(true);
        entry.set_visible(false);
        entry.set_placeholder_text(Some("Enter path..."));

        container.append(&breadcrumb_box);
        container.append(&entry);

        let edit_mode = Rc::new(RefCell::new(false));

        // When Enter is pressed in the entry, navigate to the typed path
        let cmd_tx = command_tx.clone();
        let entry_clone = entry.clone();
        let breadcrumb_clone = breadcrumb_box.clone();
        let edit_mode_clone = edit_mode.clone();
        entry.connect_activate(move |entry| {
            let text = entry.text().to_string();
            if !text.is_empty() {
                let path = RavenPath::local(PathBuf::from(&text));
                let _ = cmd_tx.send(AppCommand::Navigate {
                    path,
                    pane_id: pane(),
                });
            }
            // Switch back to breadcrumb mode
            entry_clone.set_visible(false);
            breadcrumb_clone.set_visible(true);
            *edit_mode_clone.borrow_mut() = false;
        });

        // Escape cancels edit mode
        let entry_clone2 = entry.clone();
        let breadcrumb_clone2 = breadcrumb_box.clone();
        let edit_mode_clone2 = edit_mode.clone();
        let key_controller = gtk::EventControllerKey::new();
        key_controller.connect_key_pressed(move |_, key, _, _| {
            if key == gtk::gdk::Key::Escape {
                entry_clone2.set_visible(false);
                breadcrumb_clone2.set_visible(true);
                *edit_mode_clone2.borrow_mut() = false;
                return glib::Propagation::Stop;
            }
            glib::Propagation::Proceed
        });
        entry.add_controller(key_controller);

        Self {
            container,
            breadcrumb_box,
            entry,
            edit_mode,
        }
    }

    /// Toggle between breadcrumb and edit mode.
    pub fn toggle_edit_mode(&self) {
        let mut editing = self.edit_mode.borrow_mut();
        *editing = !*editing;
        if *editing {
            self.breadcrumb_box.set_visible(false);
            self.entry.set_visible(true);
            self.entry.grab_focus();
            self.entry.select_region(0, -1);
        } else {
            self.entry.set_visible(false);
            self.breadcrumb_box.set_visible(true);
        }
    }

    /// Show a view that is not a folder, such as Recent: a single label in
    /// place of the breadcrumbs, and nothing to edit.
    pub fn set_label(&self, icon_name: &str, text: &str) {
        while let Some(child) = self.breadcrumb_box.first_child() {
            self.breadcrumb_box.remove(&child);
        }
        self.entry.set_text("");

        let content = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        content.set_margin_start(8);
        let icon = gtk::Image::from_icon_name(icon_name);
        icon.add_css_class("crumb-sep");
        content.append(&icon);
        let label = gtk::Label::new(Some(text));
        label.add_css_class("crumb-label");
        content.append(&label);
        self.breadcrumb_box.append(&content);
    }

    /// Update the breadcrumb buttons for the given path.
    pub fn set_path(
        &self,
        path: &RavenPath,
        command_tx: &tokio::sync::mpsc::UnboundedSender<AppCommand>,
        pane_id: u32,
    ) {
        // Clear existing breadcrumbs
        while let Some(child) = self.breadcrumb_box.first_child() {
            self.breadcrumb_box.remove(&child);
        }

        // Update entry text
        self.entry.set_text(&path.to_string());

        if let RavenPath::Local(local_path) = path {
            let navigate = |target: PathBuf| {
                let cmd_tx = command_tx.clone();
                move || {
                    let _ = cmd_tx.send(AppCommand::Navigate {
                        path: RavenPath::local(target.clone()),
                        pane_id,
                    });
                }
            };

            // Root button
            let root_btn = gtk::Button::from_icon_name("drive-harddisk-symbolic");
            root_btn.add_css_class("crumb");
            root_btn.set_tooltip_text(Some("/"));
            let go = navigate(PathBuf::from("/"));
            root_btn.connect_clicked(move |_| go());
            self.breadcrumb_box.append(&root_btn);

            let mut crumbs: Vec<(String, PathBuf)> = Vec::new();
            let mut accumulated = PathBuf::from("/");
            for component in local_path.components() {
                if let std::path::Component::Normal(name) = component {
                    accumulated.push(name);
                    crumbs.push((name.to_string_lossy().into_owned(), accumulated.clone()));
                }
            }

            // Deep paths fold their upper folders into one "…" crumb rather
            // than squeezing every name to a stub; the folders nearest the
            // one shown are the ones worth reading.
            let hidden = collapsed_crumbs(crumbs.len());
            if hidden > 0 {
                self.breadcrumb_box.append(&crumb_separator());
                self.breadcrumb_box.append(&overflow_crumb(&crumbs[..hidden], &navigate));
            }

            let last_index = crumbs.len().saturating_sub(1);
            for (index, (label, target)) in crumbs.iter().enumerate().skip(hidden) {
                self.breadcrumb_box.append(&crumb_separator());

                let btn = gtk::Button::with_label(label);
                btn.add_css_class("crumb");
                btn.set_tooltip_text(Some(label));
                if let Some(text) = btn.child().and_downcast::<gtk::Label>() {
                    if index == last_index {
                        // The folder being shown always keeps its whole name.
                        btn.add_css_class("current");
                    } else {
                        // A long ancestor may still give up width in a narrow
                        // pane, but never below a readable stretch of its name
                        // (and a short name never at all).
                        let keep = label.chars().count().min(MIN_CRUMB_CHARS) as i32;
                        text.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
                        text.set_width_chars(keep);
                    }
                }
                let go = navigate(target.clone());
                btn.connect_clicked(move |_| go());
                self.breadcrumb_box.append(&btn);
            }
        }
    }
}

/// Folders past this many are folded into the "…" crumb.
const VISIBLE_CRUMBS: usize = 3;

/// The fewest characters an ancestor crumb keeps when space runs out.
const MIN_CRUMB_CHARS: usize = 10;

/// How many of `count` folders (root excluded) go behind the "…" crumb:
/// none while all fit the budget, and never just one, which would take as
/// much room as the folder it hides.
fn collapsed_crumbs(count: usize) -> usize {
    if count > VISIBLE_CRUMBS + 1 {
        count - VISIBLE_CRUMBS
    } else {
        0
    }
}

fn crumb_separator() -> gtk::Image {
    let sep = gtk::Image::from_icon_name("go-next-symbolic");
    sep.add_css_class("crumb-sep");
    sep
}

/// The "…" crumb: a menu of the folded folders, outermost first.
///
/// A plain button that builds its popover on click, not a `gtk::MenuButton`:
/// a menu button presents its popover from inside its own size allocation,
/// and with one in the path bar a status-bar text change made in the same
/// frame was left laid out at its old width ("11 ite…").
fn overflow_crumb<F, G>(hidden: &[(String, PathBuf)], navigate: &F) -> gtk::Button
where
    F: Fn(PathBuf) -> G,
    G: Fn() + 'static,
{
    let btn = gtk::Button::with_label("…");
    btn.add_css_class("crumb");
    btn.set_tooltip_text(Some("Folders above"));

    let items: Vec<(String, PathBuf, Rc<dyn Fn()>)> = hidden
        .iter()
        .map(|(label, target)| {
            let go: Rc<dyn Fn()> = Rc::new(navigate(target.clone()));
            (label.clone(), target.clone(), go)
        })
        .collect();

    btn.connect_clicked(move |btn| {
        let list = gtk::Box::new(gtk::Orientation::Vertical, 0);
        list.add_css_class("crumb-menu");
        let popover = gtk::Popover::new();
        popover.set_child(Some(&list));
        for (label, target, go) in &items {
            let content = gtk::Box::new(gtk::Orientation::Horizontal, 8);
            content.append(&gtk::Image::from_icon_name("folder-symbolic"));
            let text = gtk::Label::new(Some(label));
            text.set_halign(gtk::Align::Start);
            text.set_ellipsize(gtk::pango::EllipsizeMode::Middle);
            text.set_max_width_chars(40);
            content.append(&text);

            let item = gtk::Button::new();
            item.add_css_class("flat");
            item.set_child(Some(&content));
            item.set_tooltip_text(Some(&target.to_string_lossy()));
            let go = go.clone();
            let weak = popover.downgrade();
            item.connect_clicked(move |_| {
                if let Some(popover) = weak.upgrade() {
                    popover.popdown();
                }
                go();
            });
            list.append(&item);
        }
        popover.set_parent(btn);
        // Unparented once closed, from an idle: not while it is unmapping.
        popover.connect_closed(|popover| {
            let popover = popover.clone();
            glib::idle_add_local_once(move || {
                if popover.parent().is_some() {
                    popover.unparent();
                }
            });
        });
        popover.popup();
    });
    btn
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shallow_paths_show_every_folder() {
        assert_eq!(collapsed_crumbs(0), 0);
        assert_eq!(collapsed_crumbs(3), 0);
        // Folding one folder would save nothing.
        assert_eq!(collapsed_crumbs(4), 0);
    }

    #[test]
    fn deep_paths_keep_the_nearest_folders() {
        assert_eq!(collapsed_crumbs(5), 2);
        assert_eq!(collapsed_crumbs(7), 4);
    }
}
