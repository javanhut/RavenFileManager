use std::cell::RefCell;
use std::rc::Rc;

use gtk4 as gtk;
use gtk::prelude::*;
use libadwaita as adw;
use libadwaita::prelude::*;

use raven_core::config::{AppConfig, FileAssociation, Keybinding, Theme, ViewMode};

use crate::state::AppState;
use crate::themes;

/// Settings dialog with pages for general, appearance, keybindings, and file associations.
pub struct SettingsDialog {
    pub window: adw::Window,
}

impl SettingsDialog {
    pub fn new(parent: &adw::ApplicationWindow, state: AppState) -> Self {
        let window = adw::Window::builder()
            .title("Settings")
            .default_width(700)
            .default_height(600)
            .modal(true)
            .transient_for(parent)
            .build();

        let toolbar_view = adw::ToolbarView::new();
        let header = adw::HeaderBar::new();
        toolbar_view.add_top_bar(&header);

        // Stack + sidebar for settings pages
        let stack = gtk::Stack::new();
        stack.set_transition_type(gtk::StackTransitionType::SlideLeftRight);

        let sidebar = gtk::StackSidebar::new();
        sidebar.set_stack(&stack);
        sidebar.set_width_request(180);

        // Build pages
        let config = {
            let s = state.borrow();
            s.config.clone()
        };

        stack.add_titled(
            &Self::build_general_page(&config, state.clone()),
            Some("general"),
            "General",
        );
        stack.add_titled(
            &Self::build_appearance_page(&config, state.clone()),
            Some("appearance"),
            "Appearance",
        );
        stack.add_titled(
            &Self::build_keybindings_page(&config, state.clone()),
            Some("keybindings"),
            "Keybindings",
        );
        stack.add_titled(
            &Self::build_file_associations_page(&config, state.clone()),
            Some("associations"),
            "File Associations",
        );
        stack.add_titled(
            &Self::build_tags_page(state.clone()),
            Some("tags"),
            "Tags",
        );

        let content_box = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        content_box.append(&sidebar);
        let sep = gtk::Separator::new(gtk::Orientation::Vertical);
        content_box.append(&sep);
        content_box.append(&stack);
        stack.set_hexpand(true);
        stack.set_vexpand(true);

        toolbar_view.set_content(Some(&content_box));
        window.set_content(Some(&toolbar_view));

        Self { window }
    }

    pub fn present(&self) {
        self.window.present();
    }

    fn build_general_page(config: &AppConfig, state: AppState) -> gtk::ScrolledWindow {
        let scroll = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vscrollbar_policy(gtk::PolicyType::Automatic)
            .build();

        let content = gtk::Box::new(gtk::Orientation::Vertical, 16);
        content.set_margin_start(24);
        content.set_margin_end(24);
        content.set_margin_top(16);
        content.set_margin_bottom(16);

        // Behavior section
        let behavior_group = adw::PreferencesGroup::new();
        behavior_group.set_title("Behavior");

        let hidden_row = adw::SwitchRow::new();
        hidden_row.set_title("Show hidden files by default");
        hidden_row.set_active(config.general.show_hidden_files);
        {
            let state = state.clone();
            hidden_row.connect_active_notify(move |row| {
                let mut s = state.borrow_mut();
                s.config.general.show_hidden_files = row.is_active();
                let _ = s.config.save();
            });
        }
        behavior_group.add(&hidden_row);

        let confirm_delete_row = adw::SwitchRow::new();
        confirm_delete_row.set_title("Confirm before deleting");
        confirm_delete_row.set_active(config.general.confirm_delete);
        {
            let state = state.clone();
            confirm_delete_row.connect_active_notify(move |row| {
                let mut s = state.borrow_mut();
                s.config.general.confirm_delete = row.is_active();
                let _ = s.config.save();
            });
        }
        behavior_group.add(&confirm_delete_row);

        let confirm_trash_row = adw::SwitchRow::new();
        confirm_trash_row.set_title("Confirm before trashing");
        confirm_trash_row.set_active(config.general.confirm_trash);
        {
            let state = state.clone();
            confirm_trash_row.connect_active_notify(move |row| {
                let mut s = state.borrow_mut();
                s.config.general.confirm_trash = row.is_active();
                let _ = s.config.save();
            });
        }
        behavior_group.add(&confirm_trash_row);

        let single_click_row = adw::SwitchRow::new();
        single_click_row.set_title("Single click to open");
        single_click_row.set_active(config.general.single_click_open);
        {
            let state = state.clone();
            single_click_row.connect_active_notify(move |row| {
                let mut s = state.borrow_mut();
                s.config.general.single_click_open = row.is_active();
                let _ = s.config.save();
            });
        }
        behavior_group.add(&single_click_row);

        content.append(&behavior_group);

        // Terminal section
        let terminal_group = adw::PreferencesGroup::new();
        terminal_group.set_title("Terminal");

        let terminal_row = adw::EntryRow::new();
        terminal_row.set_title("Default terminal emulator");
        terminal_row.set_text(&config.general.default_terminal);
        {
            let state = state.clone();
            terminal_row.connect_changed(move |row| {
                let text = row.text().to_string();
                if !text.is_empty() {
                    let mut s = state.borrow_mut();
                    s.config.general.default_terminal = text;
                    let _ = s.config.save();
                }
            });
        }
        terminal_group.add(&terminal_row);

        content.append(&terminal_group);

        scroll.set_child(Some(&content));
        scroll
    }

    fn build_appearance_page(config: &AppConfig, state: AppState) -> gtk::ScrolledWindow {
        let scroll = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vscrollbar_policy(gtk::PolicyType::Automatic)
            .build();

        let content = gtk::Box::new(gtk::Orientation::Vertical, 16);
        content.set_margin_start(24);
        content.set_margin_end(24);
        content.set_margin_top(16);
        content.set_margin_bottom(16);

        // Theme section
        let theme_group = adw::PreferencesGroup::new();
        theme_group.set_title("Theme");

        let theme_row = adw::ComboRow::new();
        theme_row.set_title("Color theme");
        let theme_names: Vec<&str> = Theme::ALL.iter().map(|t| t.display_name()).collect();
        let theme_model = gtk::StringList::new(&theme_names);
        theme_row.set_model(Some(&theme_model));
        let current_idx = Theme::ALL
            .iter()
            .position(|t| *t == config.appearance.theme)
            .unwrap_or(0);
        theme_row.set_selected(current_idx as u32);
        {
            let state = state.clone();
            theme_row.connect_selected_notify(move |row| {
                let idx = row.selected() as usize;
                let theme = Theme::ALL.get(idx).copied().unwrap_or_default();
                themes::apply_theme(theme);
                let mut s = state.borrow_mut();
                s.config.appearance.theme = theme;
                let _ = s.config.save();
            });
        }
        theme_group.add(&theme_row);
        content.append(&theme_group);

        let layout_group = adw::PreferencesGroup::new();
        layout_group.set_title("Layout");

        let pathbar_row = adw::SwitchRow::new();
        pathbar_row.set_title("Show path bar");
        pathbar_row.set_active(config.appearance.show_path_bar);
        {
            let state = state.clone();
            pathbar_row.connect_active_notify(move |row| {
                let mut s = state.borrow_mut();
                s.config.appearance.show_path_bar = row.is_active();
                let _ = s.config.save();
            });
        }
        layout_group.add(&pathbar_row);

        let statusbar_row = adw::SwitchRow::new();
        statusbar_row.set_title("Show status bar");
        statusbar_row.set_active(config.appearance.show_status_bar);
        {
            let state = state.clone();
            statusbar_row.connect_active_notify(move |row| {
                let mut s = state.borrow_mut();
                s.config.appearance.show_status_bar = row.is_active();
                let _ = s.config.save();
            });
        }
        layout_group.add(&statusbar_row);

        let sidebar_row = adw::SwitchRow::new();
        sidebar_row.set_title("Show sidebar");
        sidebar_row.set_active(config.appearance.show_sidebar);
        {
            let state = state.clone();
            sidebar_row.connect_active_notify(move |row| {
                let mut s = state.borrow_mut();
                s.config.appearance.show_sidebar = row.is_active();
                let _ = s.config.save();
            });
        }
        layout_group.add(&sidebar_row);

        let view_mode_row = adw::ComboRow::new();
        view_mode_row.set_title("Default view mode");
        let view_mode_model = gtk::StringList::new(&["List", "Icons", "Previews"]);
        view_mode_row.set_model(Some(&view_mode_model));
        let current_view_idx = match config.appearance.view_mode {
            ViewMode::List => 0,
            ViewMode::Icons => 1,
            ViewMode::Previews => 2,
        };
        view_mode_row.set_selected(current_view_idx);
        {
            let state = state.clone();
            view_mode_row.connect_selected_notify(move |row| {
                let mode = match row.selected() {
                    1 => ViewMode::Icons,
                    2 => ViewMode::Previews,
                    _ => ViewMode::List,
                };
                let mut s = state.borrow_mut();
                s.view_mode = mode;
                s.config.appearance.view_mode = mode;
                let _ = s.config.save();
            });
        }
        layout_group.add(&view_mode_row);

        content.append(&layout_group);

        let sizes_group = adw::PreferencesGroup::new();
        sizes_group.set_title("Sizes");

        let icon_row = adw::SpinRow::new(
            Some(&gtk::Adjustment::new(
                config.appearance.icon_size as f64,
                16.0,
                64.0,
                2.0,
                8.0,
                0.0,
            )),
            1.0,
            0,
        );
        icon_row.set_title("Icon size");
        {
            let state = state.clone();
            icon_row.connect_value_notify(move |row| {
                let mut s = state.borrow_mut();
                s.config.appearance.icon_size = row.value() as u32;
                let _ = s.config.save();
            });
        }
        sizes_group.add(&icon_row);

        let font_row = adw::SpinRow::new(
            Some(&gtk::Adjustment::new(
                config.appearance.font_size as f64,
                8.0,
                32.0,
                1.0,
                4.0,
                0.0,
            )),
            1.0,
            0,
        );
        font_row.set_title("Font size");
        {
            let state = state.clone();
            font_row.connect_value_notify(move |row| {
                let mut s = state.borrow_mut();
                s.config.appearance.font_size = row.value() as u32;
                let _ = s.config.save();
            });
        }
        sizes_group.add(&font_row);

        content.append(&sizes_group);

        scroll.set_child(Some(&content));
        scroll
    }

    fn build_keybindings_page(config: &AppConfig, state: AppState) -> gtk::ScrolledWindow {
        let scroll = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vscrollbar_policy(gtk::PolicyType::Automatic)
            .build();

        let content = gtk::Box::new(gtk::Orientation::Vertical, 16);
        content.set_margin_start(24);
        content.set_margin_end(24);
        content.set_margin_top(16);
        content.set_margin_bottom(16);

        let group = adw::PreferencesGroup::new();
        group.set_title("Keyboard Shortcuts");
        group.set_description(Some("Click on a shortcut to change it. Press the new key combination and Enter to confirm, or Escape to cancel."));

        let bindings = config.keybindings.bindings.clone();
        let list_box = Rc::new(gtk::ListBox::new());
        list_box.set_selection_mode(gtk::SelectionMode::None);
        list_box.add_css_class("boxed-list");

        for (idx, binding) in bindings.iter().enumerate() {
            let row = Self::build_keybinding_row(binding, idx, state.clone());
            list_box.append(&row);
        }

        group.add(list_box.as_ref());

        // Reset to defaults button
        let reset_btn = gtk::Button::with_label("Reset to Defaults");
        reset_btn.add_css_class("destructive-action");
        reset_btn.set_halign(gtk::Align::Start);
        reset_btn.set_margin_top(8);
        {
            let state = state.clone();
            let list_box = list_box.clone();
            reset_btn.connect_clicked(move |_| {
                {
                    let mut s = state.borrow_mut();
                    s.config.keybindings = raven_core::config::KeybindingsConfig::default();
                    let _ = s.config.save();
                }
                // Rebuild list
                while let Some(child) = list_box.first_child() {
                    list_box.remove(&child);
                }
                let s = state.borrow();
                for (idx, binding) in s.config.keybindings.bindings.iter().enumerate() {
                    let row = Self::build_keybinding_row(binding, idx, state.clone());
                    list_box.append(&row);
                }
            });
        }

        content.append(&group);
        content.append(&reset_btn);

        scroll.set_child(Some(&content));
        scroll
    }

    fn build_keybinding_row(binding: &Keybinding, idx: usize, state: AppState) -> gtk::ListBoxRow {
        let row = gtk::ListBoxRow::new();
        row.set_activatable(false);

        let hbox = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        hbox.set_margin_start(12);
        hbox.set_margin_end(12);
        hbox.set_margin_top(8);
        hbox.set_margin_bottom(8);

        // Action label
        let action_label = gtk::Label::new(Some(&format_action_name(&binding.action)));
        action_label.set_halign(gtk::Align::Start);
        action_label.set_hexpand(true);
        action_label.set_width_chars(20);
        action_label.set_xalign(0.0);
        hbox.append(&action_label);

        // Shortcut display / edit area
        let shortcut_label = gtk::Label::new(Some(&format_keybinding(binding)));
        shortcut_label.add_css_class("dim-label");
        shortcut_label.add_css_class("monospace");

        let shortcut_entry = gtk::Label::new(Some("Press a key..."));
        shortcut_entry.add_css_class("monospace");
        shortcut_entry.set_visible(false);

        let edit_btn = gtk::Button::from_icon_name("document-edit-symbolic");
        edit_btn.add_css_class("flat");
        edit_btn.set_tooltip_text(Some("Edit shortcut"));

        hbox.append(&shortcut_label);
        hbox.append(&shortcut_entry);
        hbox.append(&edit_btn);

        // Key capture controller
        let key_controller = gtk::EventControllerKey::new();
        let shortcut_label_ref = shortcut_label.clone();
        let shortcut_entry_ref = shortcut_entry.clone();
        let edit_btn_ref = edit_btn.clone();
        let state_ref = state.clone();

        let is_capturing = Rc::new(RefCell::new(false));
        let is_capturing_for_key = is_capturing.clone();
        let is_capturing_for_btn = is_capturing.clone();

        key_controller.connect_key_pressed(move |_, key, _, modifiers| {
            if !*is_capturing_for_key.borrow() {
                return glib::Propagation::Proceed;
            }

            let key_name = key.name().map(|n| n.to_string()).unwrap_or_default();

            // Escape cancels
            if key_name == "Escape" {
                *is_capturing_for_key.borrow_mut() = false;
                shortcut_entry_ref.set_visible(false);
                shortcut_label_ref.set_visible(true);
                edit_btn_ref.set_sensitive(true);
                return glib::Propagation::Stop;
            }

            // Ignore bare modifier keys
            if matches!(
                key,
                gtk::gdk::Key::Shift_L
                    | gtk::gdk::Key::Shift_R
                    | gtk::gdk::Key::Control_L
                    | gtk::gdk::Key::Control_R
                    | gtk::gdk::Key::Alt_L
                    | gtk::gdk::Key::Alt_R
                    | gtk::gdk::Key::Super_L
                    | gtk::gdk::Key::Super_R
            ) {
                return glib::Propagation::Stop;
            }

            // Build modifier list
            let mut mods = Vec::new();
            if modifiers.contains(gtk::gdk::ModifierType::CONTROL_MASK) {
                mods.push("Ctrl".to_string());
            }
            if modifiers.contains(gtk::gdk::ModifierType::ALT_MASK) {
                mods.push("Alt".to_string());
            }
            if modifiers.contains(gtk::gdk::ModifierType::SHIFT_MASK) {
                mods.push("Shift".to_string());
            }

            // Save the new keybinding
            {
                let mut s = state_ref.borrow_mut();
                if let Some(b) = s.config.keybindings.bindings.get_mut(idx) {
                    b.key = key_name;
                    b.modifiers = mods;
                    shortcut_label_ref.set_text(&format_keybinding(b));
                }
                let _ = s.config.save();
            }

            *is_capturing_for_key.borrow_mut() = false;
            shortcut_entry_ref.set_visible(false);
            shortcut_label_ref.set_visible(true);
            edit_btn_ref.set_sensitive(true);

            glib::Propagation::Stop
        });
        row.add_controller(key_controller);

        // Edit button click handler
        {
            let shortcut_label = shortcut_label.clone();
            let shortcut_entry = shortcut_entry.clone();
            edit_btn.connect_clicked(move |btn| {
                *is_capturing_for_btn.borrow_mut() = true;
                shortcut_label.set_visible(false);
                shortcut_entry.set_visible(true);
                btn.set_sensitive(false);
            });
        }

        row.set_child(Some(&hbox));
        row
    }

    fn build_tags_page(state: AppState) -> gtk::ScrolledWindow {
        let scroll = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vscrollbar_policy(gtk::PolicyType::Automatic)
            .build();

        let content = gtk::Box::new(gtk::Orientation::Vertical, 16);
        content.set_margin_start(24);
        content.set_margin_end(24);
        content.set_margin_top(16);
        content.set_margin_bottom(16);

        let group = adw::PreferencesGroup::new();
        group.set_title("Tag Rules");
        group.set_description(Some(
            "Tags are automatically applied based on rules. Smart tags use file metadata.",
        ));

        let tags_dir = dirs_config_path().join("tags.toml");
        let engine = raven_ai::tag_engine::TagEngine::new(tags_dir);
        let rules = engine.tag_rules().to_vec();

        let list_box = Rc::new(gtk::ListBox::new());
        list_box.set_selection_mode(gtk::SelectionMode::None);
        list_box.add_css_class("boxed-list");

        for rule in &rules {
            let row = gtk::ListBoxRow::new();
            row.set_activatable(false);

            let hbox = gtk::Box::new(gtk::Orientation::Horizontal, 12);
            hbox.set_margin_start(12);
            hbox.set_margin_end(12);
            hbox.set_margin_top(8);
            hbox.set_margin_bottom(8);

            let name_label = gtk::Label::new(Some(&rule.name));
            name_label.set_halign(gtk::Align::Start);
            name_label.set_hexpand(true);
            name_label.set_xalign(0.0);
            hbox.append(&name_label);

            let smart_label = if rule.is_smart {
                gtk::Label::new(Some("Smart"))
            } else {
                gtk::Label::new(Some("Custom"))
            };
            smart_label.add_css_class("dim-label");
            hbox.append(&smart_label);

            // Rule summary
            let summary = summarize_rule(rule);
            let summary_label = gtk::Label::new(Some(&summary));
            summary_label.add_css_class("dim-label");
            summary_label.add_css_class("monospace");
            summary_label.set_width_chars(20);
            summary_label.set_ellipsize(gtk::pango::EllipsizeMode::End);
            hbox.append(&summary_label);

            row.set_child(Some(&hbox));
            list_box.append(&row);
        }

        group.add(list_box.as_ref());
        content.append(&group);

        scroll.set_child(Some(&content));
        scroll
    }

    fn build_file_associations_page(config: &AppConfig, state: AppState) -> gtk::ScrolledWindow {
        let scroll = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vscrollbar_policy(gtk::PolicyType::Automatic)
            .build();

        let content = gtk::Box::new(gtk::Orientation::Vertical, 16);
        content.set_margin_start(24);
        content.set_margin_end(24);
        content.set_margin_top(16);
        content.set_margin_bottom(16);

        let group = adw::PreferencesGroup::new();
        group.set_title("File Associations");
        group.set_description(Some("Map file types to specific applications. Use MIME patterns (e.g., text/*, image/png) or file extensions."));

        let associations = config.file_associations.clone();
        let list_box = Rc::new(gtk::Box::new(gtk::Orientation::Vertical, 4));

        for (idx, assoc) in associations.iter().enumerate() {
            let row = Self::build_association_row(assoc, idx, state.clone(), list_box.clone());
            list_box.append(&row);
        }

        if associations.is_empty() {
            let empty_label = gtk::Label::new(Some("No custom file associations. Using system defaults."));
            empty_label.add_css_class("dim-label");
            empty_label.set_margin_top(12);
            empty_label.set_margin_bottom(12);
            list_box.append(&empty_label);
        }

        group.add(list_box.as_ref());

        // Add new association button
        let add_btn = gtk::Button::with_label("Add Association");
        add_btn.add_css_class("suggested-action");
        add_btn.set_halign(gtk::Align::Start);
        add_btn.set_margin_top(8);
        {
            let state = state.clone();
            let list_box = list_box.clone();
            add_btn.connect_clicked(move |_| {
                // Remove empty label if present
                while let Some(child) = list_box.first_child() {
                    if child.downcast_ref::<gtk::Label>().is_some() {
                        list_box.remove(&child);
                    } else {
                        break;
                    }
                }

                let new_assoc = FileAssociation {
                    mime_pattern: String::new(),
                    extensions: Vec::new(),
                    application: String::new(),
                    label: "New Application".to_string(),
                };
                let idx = {
                    let mut s = state.borrow_mut();
                    s.config.file_associations.push(new_assoc.clone());
                    s.config.file_associations.len() - 1
                };
                let row = Self::build_association_row(&new_assoc, idx, state.clone(), list_box.clone());
                list_box.append(&row);
            });
        }

        content.append(&group);
        content.append(&add_btn);

        scroll.set_child(Some(&content));
        scroll
    }

    fn build_association_row(
        assoc: &FileAssociation,
        idx: usize,
        state: AppState,
        list_box: Rc<gtk::Box>,
    ) -> gtk::Frame {
        let frame = gtk::Frame::new(None);
        frame.set_margin_top(4);
        frame.set_margin_bottom(4);

        let vbox = gtk::Box::new(gtk::Orientation::Vertical, 8);
        vbox.set_margin_start(12);
        vbox.set_margin_end(12);
        vbox.set_margin_top(8);
        vbox.set_margin_bottom(8);

        // Row 1: Label + delete button
        let header_row = gtk::Box::new(gtk::Orientation::Horizontal, 8);

        let label_entry = gtk::Entry::new();
        label_entry.set_placeholder_text(Some("Application name"));
        label_entry.set_text(&assoc.label);
        label_entry.set_hexpand(true);
        {
            let state = state.clone();
            label_entry.connect_changed(move |entry| {
                let text = entry.text().to_string();
                let mut s = state.borrow_mut();
                if let Some(a) = s.config.file_associations.get_mut(idx) {
                    a.label = text;
                    let _ = s.config.save();
                }
            });
        }
        header_row.append(&label_entry);

        let delete_btn = gtk::Button::from_icon_name("user-trash-symbolic");
        delete_btn.add_css_class("flat");
        delete_btn.add_css_class("destructive-action");
        delete_btn.set_tooltip_text(Some("Remove association"));
        {
            let state = state.clone();
            let frame = frame.clone();
            let list_box = list_box.clone();
            delete_btn.connect_clicked(move |_| {
                {
                    let mut s = state.borrow_mut();
                    if idx < s.config.file_associations.len() {
                        s.config.file_associations.remove(idx);
                        let _ = s.config.save();
                    }
                }
                list_box.remove(&frame);
            });
        }
        header_row.append(&delete_btn);
        vbox.append(&header_row);

        // Row 2: MIME pattern
        let mime_row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        let mime_label = gtk::Label::new(Some("MIME:"));
        mime_label.set_width_chars(10);
        mime_label.set_xalign(0.0);
        mime_label.add_css_class("dim-label");
        mime_row.append(&mime_label);

        let mime_entry = gtk::Entry::new();
        mime_entry.set_placeholder_text(Some("e.g., text/plain, image/*, application/pdf"));
        mime_entry.set_text(&assoc.mime_pattern);
        mime_entry.set_hexpand(true);
        {
            let state = state.clone();
            mime_entry.connect_changed(move |entry| {
                let text = entry.text().to_string();
                let mut s = state.borrow_mut();
                if let Some(a) = s.config.file_associations.get_mut(idx) {
                    a.mime_pattern = text;
                    let _ = s.config.save();
                }
            });
        }
        mime_row.append(&mime_entry);
        vbox.append(&mime_row);

        // Row 3: Extensions
        let ext_row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        let ext_label = gtk::Label::new(Some("Extensions:"));
        ext_label.set_width_chars(10);
        ext_label.set_xalign(0.0);
        ext_label.add_css_class("dim-label");
        ext_row.append(&ext_label);

        let ext_entry = gtk::Entry::new();
        ext_entry.set_placeholder_text(Some("e.g., rs, py, js (comma-separated)"));
        ext_entry.set_text(&assoc.extensions.join(", "));
        ext_entry.set_hexpand(true);
        {
            let state = state.clone();
            ext_entry.connect_changed(move |entry| {
                let text = entry.text().to_string();
                let extensions: Vec<String> = text
                    .split(',')
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect();
                let mut s = state.borrow_mut();
                if let Some(a) = s.config.file_associations.get_mut(idx) {
                    a.extensions = extensions;
                    let _ = s.config.save();
                }
            });
        }
        ext_row.append(&ext_entry);
        vbox.append(&ext_row);

        // Row 4: Application command
        let app_row = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        let app_label = gtk::Label::new(Some("Command:"));
        app_label.set_width_chars(10);
        app_label.set_xalign(0.0);
        app_label.add_css_class("dim-label");
        app_row.append(&app_label);

        let app_entry = gtk::Entry::new();
        app_entry.set_placeholder_text(Some("e.g., code, gimp %f, vlc %f"));
        app_entry.set_text(&assoc.application);
        app_entry.set_hexpand(true);
        {
            let state = state.clone();
            app_entry.connect_changed(move |entry| {
                let text = entry.text().to_string();
                let mut s = state.borrow_mut();
                if let Some(a) = s.config.file_associations.get_mut(idx) {
                    a.application = text;
                    let _ = s.config.save();
                }
            });
        }
        app_row.append(&app_entry);
        vbox.append(&app_row);

        frame.set_child(Some(&vbox));
        frame
    }
}

fn dirs_config_path() -> std::path::PathBuf {
    if let Some(config_dir) = dirs_config_dir() {
        config_dir.join("raven")
    } else {
        std::path::PathBuf::from(".")
    }
}

fn dirs_config_dir() -> Option<std::path::PathBuf> {
    std::env::var_os("XDG_CONFIG_HOME")
        .map(std::path::PathBuf::from)
        .or_else(|| {
            std::env::var_os("HOME").map(|h| std::path::PathBuf::from(h).join(".config"))
        })
}

fn summarize_rule(rule: &raven_ai::tags::TagRule) -> String {
    use raven_ai::tags::TagMatchRule;
    rule.rules
        .iter()
        .map(|r| match r {
            TagMatchRule::Extension { exts } => {
                let first_few: Vec<&str> = exts.iter().take(3).map(|s| s.as_str()).collect();
                format!("ext: {}", first_few.join(", "))
            }
            TagMatchRule::NamePattern { pattern } => format!("name: {}", pattern),
            TagMatchRule::MimePrefix { prefix } => format!("mime: {}", prefix),
            TagMatchRule::SizeRange { min, max } => {
                match (min, max) {
                    (Some(min), None) => format!(">{}B", min),
                    (None, Some(max)) => format!("<{}B", max),
                    (Some(min), Some(max)) => format!("{}-{}B", min, max),
                    (None, None) => "any size".to_string(),
                }
            }
            TagMatchRule::ModifiedRange { after_days, before_days } => {
                match (after_days, before_days) {
                    (Some(d), None) => format!("<{}d old", d),
                    (None, Some(d)) => format!(">{}d old", d),
                    _ => "any date".to_string(),
                }
            }
        })
        .collect::<Vec<_>>()
        .join("; ")
}

/// Format an action name for display (snake_case -> Title Case).
fn format_action_name(action: &str) -> String {
    action
        .split('_')
        .map(|word| {
            let mut chars = word.chars();
            match chars.next() {
                None => String::new(),
                Some(c) => c.to_uppercase().to_string() + chars.as_str(),
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

/// Format a keybinding for display (e.g., "Ctrl+Shift+F2").
fn format_keybinding(binding: &Keybinding) -> String {
    let mut parts = binding.modifiers.clone();
    parts.push(binding.key.clone());
    parts.join("+")
}
