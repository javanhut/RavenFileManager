use gtk::glib::variant::ToVariant;
use gtk::prelude::*;
use gtk4 as gtk;

use raven_core::config::AppConfig;
use raven_core::custom_actions::CustomAction;
use raven_core::entry::FileEntry;
use raven_core::events::PluginActionInfo;

use crate::file_opener;

/// The window action a plugin menu item activates, with a `(ss)` target of
/// (plugin id, action name).
pub const PLUGIN_ACTION: &str = "file.plugin-action";

/// One group of plugin items: all of one plugin's actions, labelled with the
/// plugin's name when more than one plugin contributes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginMenuGroup {
    pub title: Option<String>,
    pub items: Vec<PluginMenuItem>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginMenuItem {
    pub label: String,
    pub plugin_id: String,
    pub action: String,
}

/// Lay registered plugin actions out for the "Plugins" submenu.
///
/// Actions keep the order they arrive in (the registry groups them by
/// plugin); consecutive actions of one plugin form a group. Titles are only
/// worth their space when there is more than one group to tell apart, and use
/// the plugin's display name from `plugin_names` when it is known.
pub fn plugin_menu_groups(
    actions: &[PluginActionInfo],
    plugin_names: &std::collections::HashMap<String, String>,
) -> Vec<PluginMenuGroup> {
    let mut groups: Vec<(String, Vec<PluginMenuItem>)> = Vec::new();
    for action in actions {
        let item = PluginMenuItem {
            label: action.label.clone(),
            plugin_id: action.plugin_id.clone(),
            action: action.name.clone(),
        };
        match groups.last_mut() {
            Some((id, items)) if *id == action.plugin_id => items.push(item),
            _ => groups.push((action.plugin_id.clone(), vec![item])),
        }
    }
    let titled = groups.len() > 1;
    groups
        .into_iter()
        .map(|(id, items)| PluginMenuGroup {
            title: titled.then(|| plugin_names.get(&id).cloned().unwrap_or(id)),
            items,
        })
        .collect()
}

/// Right-click context menu for file operations.
pub struct FileContextMenu {
    pub popover: gtk::PopoverMenu,
    menu: gio::Menu,
    open_with_section: gio::Menu,
    tag_section: gio::Menu,
    /// Custom actions from actions.toml, rebuilt for each selection.
    actions_section: gio::Menu,
    /// Holds the "Plugins" submenu while any plugin has registered an action,
    /// and nothing otherwise.
    plugin_section: gio::Menu,
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
        edit_section.append(Some("Copy Path"), Some("file.copy-path"));
        edit_section.append(Some("Paste"), Some("file.paste"));
        edit_section.append(Some("Rename"), Some("file.rename"));
        menu.append_section(None, &edit_section);

        // New items are created in the directory being shown, whatever is selected.
        let new_section = gio::Menu::new();
        new_section.append(Some("New Folder..."), Some("file.new_folder"));
        new_section.append(Some("New File..."), Some("file.new_file"));
        menu.append_section(None, &new_section);

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
        ai_section.append(
            Some("Suggest Organization..."),
            Some("file.suggest_organization"),
        );
        menu.append_section(None, &ai_section);

        // Custom actions (dynamically populated; an empty section shows nothing)
        let actions_section = gio::Menu::new();
        menu.append_section(None, &actions_section);

        // Plugin actions (rebuilt when plugins load or unload)
        let plugin_section = gio::Menu::new();
        menu.append_section(None, &plugin_section);

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
            actions_section,
            plugin_section,
        }
    }

    /// Rebuild the "Plugins" submenu from the registered actions. Each item's
    /// target is (plugin id, action name) for [`PLUGIN_ACTION`].
    pub fn update_plugin_actions(
        &self,
        actions: &[PluginActionInfo],
        plugin_names: &std::collections::HashMap<String, String>,
    ) {
        self.plugin_section.remove_all();
        let groups = plugin_menu_groups(actions, plugin_names);
        if groups.is_empty() {
            return;
        }
        let submenu = gio::Menu::new();
        for group in groups {
            let section = gio::Menu::new();
            for item in group.items {
                let menu_item = gio::MenuItem::new(Some(&item.label), None);
                menu_item.set_action_and_target_value(
                    Some(PLUGIN_ACTION),
                    Some(&(item.plugin_id, item.action).to_variant()),
                );
                section.append_item(&menu_item);
            }
            submenu.append_section(group.title.as_deref(), &section);
        }
        self.plugin_section
            .append_submenu(Some("Plugins"), &submenu);
    }

    /// Offer the custom actions that apply to `selected`. Each item carries the
    /// action's index so the window can look it up when activated.
    pub fn update_custom_actions(&self, actions: &[CustomAction], selected: &[FileEntry]) {
        self.actions_section.remove_all();
        for (i, action) in actions.iter().enumerate() {
            if !action.applies_to(selected) {
                continue;
            }
            let item = gio::MenuItem::new(Some(&action.name), None);
            item.set_action_and_target_value(
                Some("file.custom-action"),
                Some(&(i as i32).to_variant()),
            );
            if let Some(icon) = &action.icon {
                item.set_attribute_value("icon", Some(&icon.to_variant()));
            }
            self.actions_section.append_item(&item);
        }
    }

    /// Update the "Open With" submenu for `entry`: installed applications for
    /// its content type (the desktop default first, marked), then Raven's own
    /// file associations that are not already listed, then the chooser.
    pub fn update_open_with(&self, entry: &FileEntry, config: &AppConfig) {
        self.open_with_section.remove_all();

        let content_type = if entry.is_dir() {
            Some("inode/directory".to_string())
        } else {
            entry
                .metadata
                .mime_type
                .clone()
                .filter(|m| !m.is_empty())
                .or_else(|| entry.path.as_local_path().map(|p| file_opener::content_type_for_path(p)))
        };

        let apps_section = gio::Menu::new();
        let mut app_names = Vec::new();
        let mut app_programs = Vec::new();
        if let Some(content_type) = &content_type {
            let (apps, default_id) = file_opener::apps_for_content_type(content_type);
            for app in &apps {
                let Some(id) = app.id() else { continue };
                let name = app.display_name().to_string();
                let label = if default_id.as_deref() == Some(id.as_str()) {
                    format!("{} (Default)", name)
                } else {
                    name.clone()
                };
                let item = gio::MenuItem::new(Some(&label), None);
                item.set_action_and_target_value(
                    Some("file.open-with"),
                    Some(&file_opener::OpenWithTarget::App(id.to_string()).to_target().to_variant()),
                );
                if let Some(icon) = app.icon() {
                    item.set_icon(&icon);
                }
                apps_section.append_item(&item);
                app_names.push(name);
                app_programs.push(file_opener::app_program(app));
            }
        }

        let assoc_section = gio::Menu::new();
        for (i, label) in file_opener::get_matching_associations(config, entry.extension()) {
            let command = &config.file_associations[i].application;
            if file_opener::association_duplicates_app(&label, command, &app_names, &app_programs) {
                continue;
            }
            let item = gio::MenuItem::new(Some(&label), None);
            item.set_action_and_target_value(
                Some("file.open-with"),
                Some(&file_opener::OpenWithTarget::Association(i).to_target().to_variant()),
            );
            assoc_section.append_item(&item);
        }

        if apps_section.n_items() == 0 && assoc_section.n_items() == 0 {
            apps_section.append(Some("(no applications found)"), None);
        }

        let other_section = gio::Menu::new();
        let other = gio::MenuItem::new(Some("Other Application..."), None);
        other.set_action_and_target_value(
            Some("file.open-with"),
            Some(&file_opener::OpenWithTarget::Other.to_target().to_variant()),
        );
        other_section.append_item(&other);

        self.open_with_section.append_section(None, &apps_section);
        self.open_with_section.append_section(None, &assoc_section);
        self.open_with_section.append_section(None, &other_section);
    }

    pub fn clear_open_with(&self) {
        self.open_with_section.remove_all();
        self.open_with_section
            .append(Some("(no file selected)"), None);
    }

    /// Update the "Tag" submenu with available tags and current file's tags.
    pub fn update_tags(&self, all_tags: &[String], current_tags: &[String]) {
        self.tag_section.remove_all();

        if all_tags.is_empty() {
            self.tag_section.append(Some("(no tags defined)"), None);
            return;
        }

        for tag in all_tags {
            let is_tagged = current_tags.contains(tag);
            let label = if is_tagged {
                format!("\u{2713} {}", tag)
            } else {
                tag.clone()
            };
            let item = gio::MenuItem::new(Some(&label), None);
            item.set_action_and_target_value(Some("file.toggle-tag"), Some(&tag.to_variant()));
            self.tag_section.append_item(&item);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn info(plugin: &str, name: &str, label: &str) -> PluginActionInfo {
        PluginActionInfo {
            plugin_id: plugin.into(),
            name: name.into(),
            label: label.into(),
        }
    }

    #[test]
    fn no_actions_means_no_submenu() {
        assert!(plugin_menu_groups(&[], &HashMap::new()).is_empty());
    }

    #[test]
    fn one_plugin_is_one_untitled_group() {
        let groups = plugin_menu_groups(
            &[info("zip", "compress", "Compress"), info("zip", "extract", "Extract")],
            &HashMap::new(),
        );
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].title, None);
        assert_eq!(
            groups[0].items,
            vec![
                PluginMenuItem {
                    label: "Compress".into(),
                    plugin_id: "zip".into(),
                    action: "compress".into(),
                },
                PluginMenuItem {
                    label: "Extract".into(),
                    plugin_id: "zip".into(),
                    action: "extract".into(),
                },
            ]
        );
    }

    #[test]
    fn several_plugins_are_titled_by_name_or_id() {
        let names = HashMap::from([("hash".to_string(), "Checksums".to_string())]);
        let groups = plugin_menu_groups(
            &[
                info("hash", "sha", "SHA-256"),
                info("zip", "compress", "Compress"),
            ],
            &names,
        );
        assert_eq!(groups.len(), 2);
        assert_eq!(groups[0].title.as_deref(), Some("Checksums"));
        // No display name known yet: the id stands in.
        assert_eq!(groups[1].title.as_deref(), Some("zip"));
        assert_eq!(groups[1].items[0].action, "compress");
    }

    #[test]
    fn item_target_round_trips_through_a_variant() {
        let target = ("zip".to_string(), "compress".to_string()).to_variant();
        assert_eq!(target.type_().as_str(), "(ss)");
        assert_eq!(
            target.get::<(String, String)>(),
            Some(("zip".to_string(), "compress".to_string()))
        );
    }
}
