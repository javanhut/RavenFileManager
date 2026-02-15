use gtk4 as gtk;
use gtk::prelude::*;

use raven_core::config::AppConfig;
use raven_core::entry::FileEntry;

use crate::file_opener;

/// Right-click context menu for file operations.
pub struct FileContextMenu {
    pub popover: gtk::PopoverMenu,
    menu: gio::Menu,
    open_with_section: gio::Menu,
    tag_section: gio::Menu,
}

impl FileContextMenu {
    pub fn new() -> Self {
        let menu = gio::Menu::new();

        // Open section
        let open_section = gio::Menu::new();
        open_section.append(Some("Open"), Some("file.open"));
        menu.append_section(None, &open_section);

        // Open With submenu (dynamically populated)
        let open_with_section = gio::Menu::new();
        let open_with_submenu = gio::MenuItem::new_submenu(Some("Open With"), &open_with_section);
        menu.append_item(&open_with_submenu);

        // Edit section
        let edit_section = gio::Menu::new();
        edit_section.append(Some("Cut"), Some("file.cut"));
        edit_section.append(Some("Copy"), Some("file.copy"));
        edit_section.append(Some("Paste"), Some("file.paste"));
        edit_section.append(Some("Rename"), Some("file.rename"));
        menu.append_section(None, &edit_section);

        // Sidebar section (for directories)
        let sidebar_section = gio::Menu::new();
        sidebar_section.append(Some("Pin to Sidebar"), Some("file.pin_to_sidebar"));
        menu.append_section(None, &sidebar_section);

        // Tag submenu (dynamically populated)
        let tag_section = gio::Menu::new();
        let tag_submenu = gio::MenuItem::new_submenu(Some("Tag"), &tag_section);
        menu.append_item(&tag_submenu);

        // AI section
        let ai_section = gio::Menu::new();
        ai_section.append(Some("Find Duplicates..."), Some("file.find_duplicates"));
        ai_section.append(Some("Suggest Organization..."), Some("file.suggest_organization"));
        menu.append_section(None, &ai_section);

        // Destructive section
        let delete_section = gio::Menu::new();
        delete_section.append(Some("Move to Trash"), Some("file.trash"));
        delete_section.append(Some("Delete"), Some("file.delete"));
        menu.append_section(None, &delete_section);

        // Properties section
        let props_section = gio::Menu::new();
        props_section.append(Some("Properties"), Some("file.properties"));
        menu.append_section(None, &props_section);

        let popover = gtk::PopoverMenu::from_model(Some(&menu));
        popover.set_has_arrow(false);

        Self {
            popover,
            menu,
            open_with_section,
            tag_section,
        }
    }

    /// Update the "Open With" submenu based on matching file associations.
    pub fn update_open_with(&self, entry: &FileEntry, config: &AppConfig) {
        self.open_with_section.remove_all();

        let extension = entry.extension();
        let matches = file_opener::get_matching_associations(config, extension);

        for (i, label) in &matches {
            let action_name = format!("file.open-with-{}", i);
            self.open_with_section.append(Some(label), Some(&action_name));
        }

        if matches.is_empty() {
            self.open_with_section
                .append(Some("(no associations configured)"), None);
        }
    }

    /// Update the "Tag" submenu with available tags and current file's tags.
    pub fn update_tags(&self, all_tags: &[String], current_tags: &[String]) {
        self.tag_section.remove_all();

        if all_tags.is_empty() {
            self.tag_section
                .append(Some("(no tags defined)"), None);
            return;
        }

        for (i, tag) in all_tags.iter().enumerate() {
            let is_tagged = current_tags.contains(tag);
            let label = if is_tagged {
                format!("\u{2713} {}", tag)
            } else {
                tag.clone()
            };
            let action_name = format!("file.toggle-tag-{}", i);
            self.tag_section.append(Some(&label), Some(&action_name));
        }
    }
}
