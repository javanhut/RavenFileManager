use gtk4 as gtk;
use gtk::prelude::*;

/// Right-click context menu for file operations.
pub struct FileContextMenu {
    pub popover: gtk::PopoverMenu,
}

impl FileContextMenu {
    pub fn new() -> Self {
        let menu = gio::Menu::new();

        // Open section
        let open_section = gio::Menu::new();
        open_section.append(Some("Open"), Some("file.open"));
        menu.append_section(None, &open_section);

        // Edit section
        let edit_section = gio::Menu::new();
        edit_section.append(Some("Cut"), Some("file.cut"));
        edit_section.append(Some("Copy"), Some("file.copy"));
        edit_section.append(Some("Paste"), Some("file.paste"));
        edit_section.append(Some("Rename"), Some("file.rename"));
        menu.append_section(None, &edit_section);

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

        Self { popover }
    }
}
