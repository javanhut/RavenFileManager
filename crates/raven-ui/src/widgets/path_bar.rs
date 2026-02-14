use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

use gtk4 as gtk;
use gtk::prelude::*;

use raven_core::commands::AppCommand;
use raven_core::path::RavenPath;

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
        pane_id: u32,
    ) -> Self {
        let container = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        container.add_css_class("path-bar");
        container.set_hexpand(true);

        let breadcrumb_box = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        breadcrumb_box.add_css_class("linked");
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
                    pane_id,
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
            // Build breadcrumb buttons for each path component
            let mut accumulated = PathBuf::from("/");

            // Root button
            let root_btn = gtk::Button::with_label("/");
            root_btn.add_css_class("flat");
            let cmd_tx = command_tx.clone();
            root_btn.connect_clicked(move |_| {
                let _ = cmd_tx.send(AppCommand::Navigate {
                    path: RavenPath::local(PathBuf::from("/")),
                    pane_id,
                });
            });
            self.breadcrumb_box.append(&root_btn);

            for component in local_path.components() {
                use std::path::Component;
                match component {
                    Component::RootDir => continue,
                    Component::Normal(name) => {
                        accumulated.push(name);
                        let btn_path = accumulated.clone();
                        let label = name.to_string_lossy().to_string();

                        let sep = gtk::Label::new(Some("/"));
                        sep.add_css_class("dim-label");
                        self.breadcrumb_box.append(&sep);

                        let btn = gtk::Button::with_label(&label);
                        btn.add_css_class("flat");

                        let cmd_tx = command_tx.clone();
                        btn.connect_clicked(move |_| {
                            let _ = cmd_tx.send(AppCommand::Navigate {
                                path: RavenPath::local(btn_path.clone()),
                                pane_id,
                            });
                        });
                        self.breadcrumb_box.append(&btn);
                    }
                    _ => {}
                }
            }
        }
    }
}
