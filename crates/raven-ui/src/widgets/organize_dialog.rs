use std::cell::RefCell;
use std::collections::HashSet;
use std::rc::Rc;

use gtk4 as gtk;
use gtk::prelude::*;
use libadwaita as adw;
use libadwaita::prelude::*;

use raven_core::ai_types::OrganizeSuggestion;
use raven_core::commands::AppCommand;

/// Dialog for displaying and applying organization suggestions.
pub struct OrganizeDialog {
    pub window: adw::Window,
    content_box: gtk::Box,
    action_bar: gtk::Box,
    selected_indices: Rc<RefCell<HashSet<usize>>>,
    suggestions: Rc<RefCell<Vec<OrganizeSuggestion>>>,
    command_tx: tokio::sync::mpsc::UnboundedSender<AppCommand>,
}

impl OrganizeDialog {
    pub fn new(
        parent: &adw::ApplicationWindow,
        command_tx: tokio::sync::mpsc::UnboundedSender<AppCommand>,
    ) -> Self {
        let window = adw::Window::builder()
            .title("Suggest Organization")
            .default_width(600)
            .default_height(500)
            .modal(true)
            .transient_for(parent)
            .build();

        let toolbar_view = adw::ToolbarView::new();
        let header = adw::HeaderBar::new();
        toolbar_view.add_top_bar(&header);

        let outer_box = gtk::Box::new(gtk::Orientation::Vertical, 0);

        let scroll = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vscrollbar_policy(gtk::PolicyType::Automatic)
            .vexpand(true)
            .build();

        let content_box = gtk::Box::new(gtk::Orientation::Vertical, 12);
        content_box.set_margin_start(16);
        content_box.set_margin_end(16);
        content_box.set_margin_top(12);
        content_box.set_margin_bottom(12);
        scroll.set_child(Some(&content_box));
        outer_box.append(&scroll);

        // Action bar
        let action_bar = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        action_bar.set_margin_start(16);
        action_bar.set_margin_end(16);
        action_bar.set_margin_top(8);
        action_bar.set_margin_bottom(12);
        action_bar.set_halign(gtk::Align::End);

        let cancel_btn = gtk::Button::with_label("Cancel");
        {
            let window = window.clone();
            cancel_btn.connect_clicked(move |_| {
                window.close();
            });
        }
        action_bar.append(&cancel_btn);

        let selected_indices: Rc<RefCell<HashSet<usize>>> =
            Rc::new(RefCell::new(HashSet::new()));
        let suggestions: Rc<RefCell<Vec<OrganizeSuggestion>>> =
            Rc::new(RefCell::new(Vec::new()));

        let apply_btn = gtk::Button::with_label("Apply Selected");
        apply_btn.add_css_class("suggested-action");
        {
            let selected = selected_indices.clone();
            let suggestions = suggestions.clone();
            let cmd_tx = command_tx.clone();
            let window = window.clone();
            apply_btn.connect_clicked(move |_| {
                let indices = selected.borrow();
                let all_suggestions = suggestions.borrow();
                let to_apply: Vec<OrganizeSuggestion> = indices
                    .iter()
                    .filter_map(|&i| all_suggestions.get(i).cloned())
                    .collect();
                if !to_apply.is_empty() {
                    let _ = cmd_tx.send(AppCommand::ApplyOrganization {
                        suggestions: to_apply,
                    });
                }
                window.close();
            });
        }
        action_bar.append(&apply_btn);

        outer_box.append(&action_bar);

        toolbar_view.set_content(Some(&outer_box));
        window.set_content(Some(&toolbar_view));

        Self {
            window,
            content_box,
            action_bar,
            selected_indices,
            suggestions,
            command_tx,
        }
    }

    pub fn present(&self) {
        self.window.present();
    }

    /// Set the suggestions to display.
    pub fn set_suggestions(&self, suggestions: Vec<OrganizeSuggestion>) {
        // Clear previous content
        while let Some(child) = self.content_box.first_child() {
            self.content_box.remove(&child);
        }
        self.selected_indices.borrow_mut().clear();

        if suggestions.is_empty() {
            let label = gtk::Label::new(Some("This directory looks well-organized already."));
            label.add_css_class("title-3");
            label.set_margin_top(48);
            label.set_margin_bottom(48);
            self.content_box.append(&label);
            self.action_bar.set_visible(false);
            *self.suggestions.borrow_mut() = suggestions;
            return;
        }

        self.action_bar.set_visible(true);

        for (idx, suggestion) in suggestions.iter().enumerate() {
            let frame = gtk::Frame::new(None);
            frame.set_margin_top(2);
            frame.set_margin_bottom(2);

            let vbox = gtk::Box::new(gtk::Orientation::Vertical, 4);
            vbox.set_margin_start(12);
            vbox.set_margin_end(12);
            vbox.set_margin_top(8);
            vbox.set_margin_bottom(8);

            // Header with checkbox
            let hbox = gtk::Box::new(gtk::Orientation::Horizontal, 8);

            let check = gtk::CheckButton::new();
            check.set_active(true);
            self.selected_indices.borrow_mut().insert(idx);

            {
                let selected = self.selected_indices.clone();
                check.connect_toggled(move |btn| {
                    if btn.is_active() {
                        selected.borrow_mut().insert(idx);
                    } else {
                        selected.borrow_mut().remove(&idx);
                    }
                });
            }
            hbox.append(&check);

            let desc_label = gtk::Label::new(Some(&suggestion.description));
            desc_label.set_halign(gtk::Align::Start);
            desc_label.set_hexpand(true);
            desc_label.set_wrap(true);
            hbox.append(&desc_label);

            vbox.append(&hbox);

            // Expandable file list
            let expander = gtk::Expander::new(Some(&format!(
                "{} files",
                suggestion.source_files.len()
            )));
            let file_list = gtk::Box::new(gtk::Orientation::Vertical, 2);
            file_list.set_margin_start(24);
            for file in &suggestion.source_files {
                let label = gtk::Label::new(Some(&file.name));
                label.set_halign(gtk::Align::Start);
                label.add_css_class("dim-label");
                label.add_css_class("monospace");
                file_list.append(&label);
            }
            expander.set_child(Some(&file_list));
            vbox.append(&expander);

            frame.set_child(Some(&vbox));
            self.content_box.append(&frame);
        }

        *self.suggestions.borrow_mut() = suggestions;
    }
}
