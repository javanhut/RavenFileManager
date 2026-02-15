use std::cell::RefCell;
use std::rc::Rc;

use gtk4 as gtk;
use gtk::prelude::*;
use libadwaita as adw;
use libadwaita::prelude::*;

use raven_core::commands::AppCommand;
use raven_core::events::AppEvent;
use raven_core::path::RavenPath;

use crate::state::{AppState, ClipboardOp};
use crate::widgets::container_banner::ContainerBanner;
use crate::widgets::context_menu::FileContextMenu;
use crate::widgets::file_list::FileListView;
use crate::widgets::operation_panel::OperationPanel;
use crate::widgets::path_bar::PathBar;
use crate::widgets::preview_panel::PreviewPanel;
use crate::widgets::properties_dialog::PropertiesDialog;
use crate::widgets::search_bar::SearchBar;
use crate::widgets::settings_dialog::SettingsDialog;
use crate::widgets::sidebar::Sidebar;
use crate::widgets::tab_bar::TabBar;

pub struct RavenWindow {
    pub window: adw::ApplicationWindow,
    pub state: AppState,
    pub command_tx: tokio::sync::mpsc::UnboundedSender<AppCommand>,
    file_list: Rc<FileListView>,
    path_bar: Rc<PathBar>,
    tab_bar: Rc<TabBar>,
    search_bar: Rc<SearchBar>,
    preview_panel: Rc<PreviewPanel>,
    preview_separator: gtk::Separator,
    operation_panel: Rc<OperationPanel>,
    status_label: gtk::Label,
    container_banner: Rc<ContainerBanner>,
    // Disk usage status indicator
    disk_usage_status: gtk::Box,
    disk_usage_bar: gtk::ProgressBar,
    // Properties dialog (when open)
    properties_dialog: Rc<RefCell<Option<Rc<RefCell<PropertiesDialog>>>>>,
    // Context menu
    context_menu: Rc<FileContextMenu>,
    // Sidebar (for dynamic bookmark pinning)
    sidebar: Rc<Sidebar>,
}

impl RavenWindow {
    pub fn new(
        app: &adw::Application,
        state: AppState,
        command_tx: tokio::sync::mpsc::UnboundedSender<AppCommand>,
    ) -> Self {
        let window = adw::ApplicationWindow::builder()
            .application(app)
            .title("Raven File Manager")
            .default_width(1200)
            .default_height(800)
            .build();

        let pane_id = {
            let s = state.borrow();
            s.active_tab().active_pane().id
        };

        // Main layout
        let toolbar_view = adw::ToolbarView::new();

        // --- Header bar ---
        let header = adw::HeaderBar::new();

        // Navigation buttons
        let nav_box = gtk::Box::new(gtk::Orientation::Horizontal, 0);
        nav_box.add_css_class("linked");

        let back_btn = gtk::Button::from_icon_name("go-previous-symbolic");
        back_btn.set_tooltip_text(Some("Back (Alt+Left)"));
        {
            let state = state.clone();
            let cmd_tx = command_tx.clone();
            back_btn.connect_clicked(move |_| {
                navigate_back(&state, &cmd_tx, pane_id);
            });
        }
        nav_box.append(&back_btn);

        let forward_btn = gtk::Button::from_icon_name("go-next-symbolic");
        forward_btn.set_tooltip_text(Some("Forward (Alt+Right)"));
        {
            let state = state.clone();
            let cmd_tx = command_tx.clone();
            forward_btn.connect_clicked(move |_| {
                navigate_forward(&state, &cmd_tx, pane_id);
            });
        }
        nav_box.append(&forward_btn);

        let up_btn = gtk::Button::from_icon_name("go-up-symbolic");
        up_btn.set_tooltip_text(Some("Up (Alt+Up)"));
        {
            let state = state.clone();
            let cmd_tx = command_tx.clone();
            up_btn.connect_clicked(move |_| {
                navigate_up(&state, &cmd_tx, pane_id);
            });
        }
        nav_box.append(&up_btn);

        header.pack_start(&nav_box);

        // Settings button (hamburger menu)
        let settings_btn = gtk::Button::from_icon_name("open-menu-symbolic");
        settings_btn.set_tooltip_text(Some("Settings (Ctrl+,)"));
        {
            let state = state.clone();
            let window_ref = window.clone();
            settings_btn.connect_clicked(move |_| {
                let dialog = SettingsDialog::new(&window_ref, state.clone());
                dialog.present();
            });
        }
        header.pack_end(&settings_btn);

        // Search toggle button
        let search_btn = gtk::ToggleButton::new();
        search_btn.set_icon_name("system-search-symbolic");
        search_btn.set_tooltip_text(Some("Search (Ctrl+F)"));
        header.pack_end(&search_btn);

        // Hidden files toggle
        let hidden_btn = gtk::ToggleButton::new();
        hidden_btn.set_icon_name("view-reveal-symbolic");
        hidden_btn.set_tooltip_text(Some("Show hidden files (Ctrl+H)"));
        header.pack_end(&hidden_btn);

        toolbar_view.add_top_bar(&header);

        // --- Container banner ---
        let container_banner = Rc::new(ContainerBanner::new());
        toolbar_view.add_top_bar(&container_banner.revealer);

        // --- Tab bar ---
        let tab_bar = Rc::new(TabBar::new(state.clone(), command_tx.clone()));
        toolbar_view.add_top_bar(&tab_bar.widget);

        // --- Path bar ---
        let path_bar = Rc::new(PathBar::new(command_tx.clone(), pane_id));
        path_bar.container.set_margin_start(8);
        path_bar.container.set_margin_end(8);
        path_bar.container.set_margin_top(4);
        path_bar.container.set_margin_bottom(4);
        toolbar_view.add_top_bar(&path_bar.container);

        // --- Search bar ---
        let get_path = {
            let state = state.clone();
            move || -> Option<RavenPath> {
                let s = state.borrow();
                Some(s.active_tab().active_pane().current_path.clone())
            }
        };
        let search_bar = Rc::new(SearchBar::new(command_tx.clone(), get_path, pane_id));
        toolbar_view.add_top_bar(&search_bar.revealer);

        // Wire search toggle button to search bar
        {
            let sb = search_bar.clone();
            search_btn.connect_toggled(move |btn| {
                if btn.is_active() {
                    sb.show();
                } else {
                    sb.hide();
                }
            });
        }

        // --- Content area: sidebar + file list + preview panel ---
        let content_box = gtk::Box::new(gtk::Orientation::Horizontal, 0);

        // Sidebar
        let sidebar = Rc::new(Sidebar::new(state.clone(), command_tx.clone(), pane_id));
        let sidebar_scroll = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vscrollbar_policy(gtk::PolicyType::Automatic)
            .child(&sidebar.widget)
            .build();
        sidebar_scroll.set_width_request(200);

        let sidebar_separator = gtk::Separator::new(gtk::Orientation::Vertical);
        let file_list = Rc::new(FileListView::new(state.clone(), command_tx.clone(), pane_id));

        content_box.append(&sidebar_scroll);
        content_box.append(&sidebar_separator);
        content_box.append(&file_list.scrolled_window);

        // --- Context menu ---
        let context_menu = Rc::new(FileContextMenu::new());
        context_menu.popover.set_parent(&file_list.column_view);

        // Register file actions on the column_view
        let action_group = gio::SimpleActionGroup::new();

        // file.open
        {
            let file_list = file_list.clone();
            let state = state.clone();
            let cmd_tx = command_tx.clone();
            let action = gio::SimpleAction::new("open", None);
            action.connect_activate(move |_, _| {
                if let Some(entry) = get_selected_entry(&file_list) {
                    if entry.is_dir() {
                        {
                            let mut s = state.borrow_mut();
                            if let Some(pane) = s.pane_by_id_mut(pane_id) {
                                pane.navigate_to(entry.path.clone());
                            }
                        }
                        let _ = cmd_tx.send(AppCommand::Navigate { path: entry.path, pane_id });
                    } else {
                        let config = state.borrow().config.clone();
                        crate::file_opener::open_file(&entry.path, &config);
                    }
                }
            });
            action_group.add_action(&action);
        }

        // file.copy
        {
            let file_list = file_list.clone();
            let state = state.clone();
            let action = gio::SimpleAction::new("copy", None);
            action.connect_activate(move |_, _| {
                if let Some(entry) = get_selected_entry(&file_list) {
                    let mut s = state.borrow_mut();
                    s.clipboard = Some(ClipboardOp::Copy(vec![entry.path]));
                }
            });
            action_group.add_action(&action);
        }

        // file.cut
        {
            let file_list = file_list.clone();
            let state = state.clone();
            let action = gio::SimpleAction::new("cut", None);
            action.connect_activate(move |_, _| {
                if let Some(entry) = get_selected_entry(&file_list) {
                    let mut s = state.borrow_mut();
                    s.clipboard = Some(ClipboardOp::Cut(vec![entry.path]));
                }
            });
            action_group.add_action(&action);
        }

        // file.paste
        {
            let state = state.clone();
            let cmd_tx = command_tx.clone();
            let action = gio::SimpleAction::new("paste", None);
            action.connect_activate(move |_, _| {
                let (clipboard, dest) = {
                    let mut s = state.borrow_mut();
                    let dest = s.active_tab().active_pane().current_path.clone();
                    (s.clipboard.take(), dest)
                };
                match clipboard {
                    Some(ClipboardOp::Copy(sources)) => {
                        let _ = cmd_tx.send(AppCommand::CopyFiles {
                            sources,
                            destination: dest,
                        });
                    }
                    Some(ClipboardOp::Cut(sources)) => {
                        let _ = cmd_tx.send(AppCommand::MoveFiles {
                            sources,
                            destination: dest,
                        });
                    }
                    None => {}
                }
            });
            action_group.add_action(&action);
        }

        // file.rename
        {
            let file_list = file_list.clone();
            let cmd_tx = command_tx.clone();
            let window_ref = window.clone();
            let action = gio::SimpleAction::new("rename", None);
            action.connect_activate(move |_, _| {
                if let Some(entry) = get_selected_entry(&file_list) {
                    show_rename_dialog(&window_ref, &entry, &cmd_tx);
                }
            });
            action_group.add_action(&action);
        }

        // file.trash
        {
            let file_list = file_list.clone();
            let cmd_tx = command_tx.clone();
            let action = gio::SimpleAction::new("trash", None);
            action.connect_activate(move |_, _| {
                if let Some(entry) = get_selected_entry(&file_list) {
                    let _ = cmd_tx.send(AppCommand::TrashFiles { paths: vec![entry.path] });
                }
            });
            action_group.add_action(&action);
        }

        // file.delete
        {
            let file_list = file_list.clone();
            let cmd_tx = command_tx.clone();
            let action = gio::SimpleAction::new("delete", None);
            action.connect_activate(move |_, _| {
                if let Some(entry) = get_selected_entry(&file_list) {
                    let _ = cmd_tx.send(AppCommand::DeleteFiles { paths: vec![entry.path] });
                }
            });
            action_group.add_action(&action);
        }

        // file.properties
        let properties_dialog: Rc<RefCell<Option<Rc<RefCell<PropertiesDialog>>>>> =
            Rc::new(RefCell::new(None));
        {
            let file_list = file_list.clone();
            let cmd_tx = command_tx.clone();
            let window_ref = window.clone();
            let properties_dialog = properties_dialog.clone();
            let action = gio::SimpleAction::new("properties", None);
            action.connect_activate(move |_, _| {
                if let Some(entry) = get_selected_entry(&file_list) {
                    let dialog = PropertiesDialog::new(&window_ref, &entry, &cmd_tx);
                    dialog.borrow().present();
                    *properties_dialog.borrow_mut() = Some(dialog);
                }
            });
            action_group.add_action(&action);
        }

        // file.pin_to_sidebar
        {
            let file_list = file_list.clone();
            let sidebar = sidebar.clone();
            let action = gio::SimpleAction::new("pin_to_sidebar", None);
            action.connect_activate(move |_, _| {
                if let Some(entry) = get_selected_entry(&file_list) {
                    if entry.is_dir() {
                        if let Some(local) = entry.path.as_local_path() {
                            let bookmark = raven_core::config::Bookmark {
                                name: entry.name.clone(),
                                path: local.to_string_lossy().to_string(),
                                icon: Some("folder-symbolic".to_string()),
                            };
                            sidebar.add_bookmark(&bookmark);
                        }
                    }
                }
            });
            action_group.add_action(&action);
        }

        // file.open-with-N actions (support up to 20 associations)
        for assoc_idx in 0..20 {
            let file_list = file_list.clone();
            let state = state.clone();
            let action = gio::SimpleAction::new(&format!("open-with-{}", assoc_idx), None);
            action.connect_activate(move |_, _| {
                if let Some(entry) = get_selected_entry(&file_list) {
                    let config = state.borrow().config.clone();
                    crate::file_opener::open_with_association(&entry.path, &config, assoc_idx);
                }
            });
            action_group.add_action(&action);
        }

        file_list
            .column_view
            .insert_action_group("file", Some(&action_group));

        // Right-click gesture for context menu
        {
            let context_menu = context_menu.clone();
            let file_list_for_ctx = file_list.clone();
            let state = state.clone();
            let gesture = gtk::GestureClick::new();
            gesture.set_button(3); // Right click
            gesture.connect_released(move |_, _, x, y| {
                // Update "Open With" submenu before showing
                if let Some(entry) = get_selected_entry(&file_list_for_ctx) {
                    let config = state.borrow().config.clone();
                    context_menu.update_open_with(&entry, &config);
                }
                let rect = gtk::gdk::Rectangle::new(x as i32, y as i32, 1, 1);
                context_menu.popover.set_pointing_to(Some(&rect));
                context_menu.popover.popup();
            });
            file_list.column_view.add_controller(gesture);
        }

        // Preview panel (initially hidden)
        let preview_separator = gtk::Separator::new(gtk::Orientation::Vertical);
        preview_separator.set_visible(false);
        let preview_panel = Rc::new(PreviewPanel::new());
        preview_panel.widget.set_visible(false);

        content_box.append(&preview_separator);
        content_box.append(&preview_panel.widget);

        toolbar_view.set_content(Some(&content_box));

        // --- Operation panel ---
        let operation_panel = Rc::new(OperationPanel::new());
        toolbar_view.add_bottom_bar(&operation_panel.revealer);

        // --- Status bar ---
        let status_bar = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        status_bar.set_margin_start(8);
        status_bar.set_margin_end(8);
        status_bar.set_margin_top(4);
        status_bar.set_margin_bottom(4);
        let status_label = gtk::Label::new(Some("Ready"));
        status_label.set_halign(gtk::Align::Start);
        status_label.set_hexpand(true);
        status_label.add_css_class("dim-label");
        status_bar.append(&status_label);

        // Disk usage status indicator (initially hidden)
        let disk_usage_status = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        disk_usage_status.set_visible(false);

        let du_label = gtk::Label::new(Some("Calculating..."));
        du_label.add_css_class("dim-label");
        disk_usage_status.append(&du_label);

        let disk_usage_bar = gtk::ProgressBar::new();
        disk_usage_bar.set_width_request(120);
        disk_usage_status.append(&disk_usage_bar);

        let du_cancel_btn = gtk::Button::from_icon_name("process-stop-symbolic");
        du_cancel_btn.add_css_class("flat");
        du_cancel_btn.add_css_class("circular");
        du_cancel_btn.set_tooltip_text(Some("Cancel"));
        {
            let cmd_tx = command_tx.clone();
            du_cancel_btn.connect_clicked(move |_| {
                let _ = cmd_tx.send(AppCommand::CancelDiskUsage);
            });
        }
        disk_usage_status.append(&du_cancel_btn);

        status_bar.append(&disk_usage_status);
        toolbar_view.add_bottom_bar(&status_bar);

        window.set_content(Some(&toolbar_view));

        // --- Hidden files toggle ---
        {
            let state = state.clone();
            let file_list = file_list.clone();
            hidden_btn.connect_toggled(move |btn| {
                let show_hidden = btn.is_active();
                let mut s = state.borrow_mut();
                s.show_hidden = show_hidden;
                let entries = s.active_tab().active_pane().entries.clone();
                drop(s);
                file_list.set_entries(&entries, show_hidden);
            });
        }

        // --- Tab bar callbacks ---
        {
            let file_list = file_list.clone();
            let path_bar = path_bar.clone();
            let state = state.clone();
            let cmd_tx = command_tx.clone();
            tab_bar.set_on_tab_changed(move |new_pane_id| {
                let s = state.borrow();
                if let Some(pane) = s.pane_by_id(new_pane_id) {
                    let show_hidden = s.show_hidden;
                    file_list.set_entries(&pane.entries, show_hidden);
                    path_bar.set_path(&pane.current_path, &cmd_tx, new_pane_id);
                }
            });
        }

        // --- Keyboard shortcuts ---
        {
            let key_controller = gtk::EventControllerKey::new();
            let path_bar = path_bar.clone();
            let cmd_tx = command_tx.clone();
            let state = state.clone();
            let hidden_btn = hidden_btn.clone();
            let search_btn = search_btn.clone();
            let preview_panel_for_key = preview_panel.clone();
            let preview_sep_for_key = preview_separator.clone();
            let tab_bar_for_key = tab_bar.clone();
            let file_list_for_key = file_list.clone();
            let properties_dialog_for_key = properties_dialog.clone();
            let window_for_key = window.clone();
            let state_for_settings = state.clone();
            let window_for_settings = window.clone();
            key_controller.connect_key_pressed(move |_, key, _, modifiers| {
                let ctrl = modifiers.contains(gtk::gdk::ModifierType::CONTROL_MASK);
                let alt = modifiers.contains(gtk::gdk::ModifierType::ALT_MASK);

                match key {
                    // Ctrl+L: edit path bar
                    gtk::gdk::Key::l if ctrl => {
                        path_bar.toggle_edit_mode();
                        return glib::Propagation::Stop;
                    }
                    // Ctrl+H: toggle hidden files
                    gtk::gdk::Key::h if ctrl => {
                        hidden_btn.set_active(!hidden_btn.is_active());
                        return glib::Propagation::Stop;
                    }
                    // Ctrl+F: toggle search
                    gtk::gdk::Key::f if ctrl => {
                        search_btn.set_active(!search_btn.is_active());
                        return glib::Propagation::Stop;
                    }
                    // Ctrl+T: new tab
                    gtk::gdk::Key::t if ctrl => {
                        let path = {
                            let s = state.borrow();
                            s.active_tab().active_pane().current_path.clone()
                        };
                        let pane_id = {
                            let mut s = state.borrow_mut();
                            let _tab_id = s.add_tab(path.clone());
                            s.active_tab().active_pane().id
                        };
                        tab_bar_for_key.refresh();
                        let _ = cmd_tx.send(AppCommand::Navigate { path, pane_id });
                        return glib::Propagation::Stop;
                    }
                    // Ctrl+W: close tab
                    gtk::gdk::Key::w if ctrl => {
                        let closed = {
                            let mut s = state.borrow_mut();
                            let idx = s.active_tab;
                            s.close_tab(idx)
                        };
                        if closed {
                            tab_bar_for_key.refresh();
                            let (path, new_pane_id) = {
                                let s = state.borrow();
                                (
                                    s.active_tab().active_pane().current_path.clone(),
                                    s.active_tab().active_pane().id,
                                )
                            };
                            let _ = cmd_tx.send(AppCommand::Navigate {
                                path,
                                pane_id: new_pane_id,
                            });
                        }
                        return glib::Propagation::Stop;
                    }
                    // Space: toggle preview
                    gtk::gdk::Key::space if !ctrl && !alt => {
                        let show = !preview_panel_for_key.widget.is_visible();
                        preview_panel_for_key.widget.set_visible(show);
                        preview_sep_for_key.set_visible(show);

                        if show {
                            let path = {
                                let s = state.borrow();
                                s.active_tab().active_pane().current_path.clone()
                            };
                            let _ = cmd_tx.send(AppCommand::GeneratePreview { path });
                        }
                        return glib::Propagation::Stop;
                    }
                    // Alt+Left: back
                    gtk::gdk::Key::Left if alt => {
                        navigate_back(&state, &cmd_tx, pane_id);
                        return glib::Propagation::Stop;
                    }
                    // Alt+Right: forward
                    gtk::gdk::Key::Right if alt => {
                        navigate_forward(&state, &cmd_tx, pane_id);
                        return glib::Propagation::Stop;
                    }
                    // Alt+Up: parent directory
                    gtk::gdk::Key::Up if alt => {
                        navigate_up(&state, &cmd_tx, pane_id);
                        return glib::Propagation::Stop;
                    }
                    // Ctrl+R: refresh
                    gtk::gdk::Key::r if ctrl => {
                        let path = {
                            let s = state.borrow();
                            s.pane_by_id(pane_id).map(|p| p.current_path.clone())
                        };
                        if let Some(path) = path {
                            let _ = cmd_tx.send(AppCommand::Navigate { path, pane_id });
                        }
                        return glib::Propagation::Stop;
                    }
                    // Ctrl+Z: undo
                    gtk::gdk::Key::z if ctrl => {
                        let _ = cmd_tx.send(AppCommand::Undo);
                        return glib::Propagation::Stop;
                    }
                    // Ctrl+I: properties
                    gtk::gdk::Key::i if ctrl => {
                        if let Some(entry) = get_selected_entry(&file_list_for_key) {
                            let dialog =
                                PropertiesDialog::new(&window_for_key, &entry, &cmd_tx);
                            dialog.borrow().present();
                            *properties_dialog_for_key.borrow_mut() = Some(dialog);
                        }
                        return glib::Propagation::Stop;
                    }
                    // F2: rename
                    gtk::gdk::Key::F2 if !ctrl && !alt => {
                        if let Some(entry) = get_selected_entry(&file_list_for_key) {
                            show_rename_dialog(&window_for_key, &entry, &cmd_tx);
                        }
                        return glib::Propagation::Stop;
                    }
                    // Delete: trash selected files
                    gtk::gdk::Key::Delete if !ctrl => {
                        if let Some(entry) = get_selected_entry(&file_list_for_key) {
                            let _ = cmd_tx
                                .send(AppCommand::TrashFiles { paths: vec![entry.path] });
                        }
                        return glib::Propagation::Stop;
                    }
                    // Ctrl+C: copy
                    gtk::gdk::Key::c if ctrl => {
                        if let Some(entry) = get_selected_entry(&file_list_for_key) {
                            let mut s = state.borrow_mut();
                            s.clipboard = Some(ClipboardOp::Copy(vec![entry.path]));
                        }
                        return glib::Propagation::Stop;
                    }
                    // Ctrl+X: cut
                    gtk::gdk::Key::x if ctrl => {
                        if let Some(entry) = get_selected_entry(&file_list_for_key) {
                            let mut s = state.borrow_mut();
                            s.clipboard = Some(ClipboardOp::Cut(vec![entry.path]));
                        }
                        return glib::Propagation::Stop;
                    }
                    // Ctrl+V: paste
                    gtk::gdk::Key::v if ctrl => {
                        let (clipboard, dest) = {
                            let mut s = state.borrow_mut();
                            let dest = s.active_tab().active_pane().current_path.clone();
                            (s.clipboard.take(), dest)
                        };
                        match clipboard {
                            Some(ClipboardOp::Copy(sources)) => {
                                let _ = cmd_tx.send(AppCommand::CopyFiles {
                                    sources,
                                    destination: dest,
                                });
                            }
                            Some(ClipboardOp::Cut(sources)) => {
                                let _ = cmd_tx.send(AppCommand::MoveFiles {
                                    sources,
                                    destination: dest,
                                });
                            }
                            None => {}
                        }
                        return glib::Propagation::Stop;
                    }
                    // Ctrl+,: open settings
                    gtk::gdk::Key::comma if ctrl => {
                        let dialog = SettingsDialog::new(
                            &window_for_settings,
                            state_for_settings.clone(),
                        );
                        dialog.present();
                        return glib::Propagation::Stop;
                    }
                    _ => {}
                }
                glib::Propagation::Proceed
            });
            window.add_controller(key_controller);
        }

        // Request container info on startup
        let _ = command_tx.send(AppCommand::GetContainerInfo);

        Self {
            window,
            state,
            command_tx,
            file_list,
            path_bar,
            tab_bar,
            search_bar,
            preview_panel,
            preview_separator,
            operation_panel,
            status_label,
            container_banner,
            disk_usage_status,
            disk_usage_bar,
            properties_dialog,
            context_menu,
            sidebar,
        }
    }

    pub fn handle_event(&self, event: AppEvent) {
        match event {
            AppEvent::DirectoryLoaded {
                pane_id,
                path,
                entries,
            } => {
                let show_hidden = {
                    let mut state = self.state.borrow_mut();
                    if let Some(pane) = state.pane_by_id_mut(pane_id) {
                        pane.entries = entries.clone();
                    }
                    let tab_title = path.file_name().unwrap_or("/").to_string();
                    state.active_tab_mut().title = tab_title;
                    state.show_hidden
                };

                self.file_list.set_entries(&entries, show_hidden);
                self.path_bar
                    .set_path(&path, &self.command_tx, pane_id);
                self.tab_bar.refresh();

                let count = if show_hidden {
                    entries.len()
                } else {
                    entries.iter().filter(|e| !e.is_hidden()).count()
                };
                let total = entries.len();
                let hidden = total - count;
                let status = if hidden > 0 {
                    format!("{} items ({} hidden)", count, hidden)
                } else {
                    format!("{} items", count)
                };
                self.status_label.set_text(&status);
            }

            AppEvent::DirectoryError { path, error, .. } => {
                tracing::error!("Failed to load {}: {}", path, error);
                self.status_label.set_text(&format!("Error: {}", error));
            }

            // --- Operation events ---
            AppEvent::OperationStarted { id, description } => {
                self.operation_panel.add_operation(id, &description);
            }

            AppEvent::OperationProgress { progress } => {
                self.operation_panel.update_progress(&progress);
            }

            AppEvent::OperationCompleted { id } => {
                self.operation_panel.remove_operation(id);
                let (path, pane_id) = {
                    let s = self.state.borrow();
                    (
                        s.active_tab().active_pane().current_path.clone(),
                        s.active_tab().active_pane().id,
                    )
                };
                let _ = self
                    .command_tx
                    .send(AppCommand::Navigate { path, pane_id });
            }

            AppEvent::OperationFailed { id, error } => {
                self.operation_panel.remove_operation(id);
                self.status_label.set_text(&format!("Operation failed: {}", error));
            }

            AppEvent::OperationConflict { conflict: _ } => {
                // TODO: show conflict resolution dialog
            }

            // --- Search events ---
            AppEvent::SearchResult { entry, .. } => {
                let show_hidden = self.state.borrow().show_hidden;
                if show_hidden || !entry.is_hidden() {
                    self.file_list
                        .model
                        .append(&crate::widgets::file_list::FileEntryObject::new(&entry));
                }
            }

            AppEvent::SearchCompleted { total_matches } => {
                self.status_label
                    .set_text(&format!("Search complete: {} matches", total_matches));
            }

            AppEvent::SearchError { error } => {
                self.status_label.set_text(&format!("Search error: {}", error));
            }

            // --- Preview events ---
            AppEvent::PreviewReady { path, preview } => {
                self.preview_panel.set_preview(&path, &preview);
            }

            AppEvent::PreviewError { path, error } => {
                tracing::warn!("Preview failed for {}: {}", path, error);
            }

            // --- Git events ---
            AppEvent::GitStatusUpdated { .. } => {
                // TODO: update git status column in file list
            }

            // --- Automation events ---
            AppEvent::AutomationRuleTriggered {
                rule_name,
                matched_files,
                ..
            } => {
                self.status_label.set_text(&format!(
                    "Rule '{}' triggered on {} files",
                    rule_name,
                    matched_files.len()
                ));
            }

            AppEvent::AutomationActionCompleted { rule_id, action } => {
                tracing::info!("Automation rule {} completed action: {}", rule_id, action);
            }

            AppEvent::AutomationError { rule_id, error } => {
                tracing::error!("Automation rule {} error: {}", rule_id, error);
                self.status_label
                    .set_text(&format!("Automation error: {}", error));
            }

            // --- Plugin events ---
            AppEvent::PluginLoaded { name, .. } => {
                tracing::info!("Plugin loaded: {}", name);
            }

            AppEvent::PluginUnloaded { plugin_id } => {
                tracing::info!("Plugin unloaded: {}", plugin_id);
            }

            AppEvent::PluginError { plugin_id, error } => {
                tracing::error!("Plugin {} error: {}", plugin_id, error);
                self.status_label
                    .set_text(&format!("Plugin error: {}", error));
            }

            // --- Network events ---
            AppEvent::RemoteConnected {
                protocol, host, ..
            } => {
                self.status_label
                    .set_text(&format!("Connected to {} ({})", host, protocol));
            }

            AppEvent::RemoteDisconnected { id } => {
                self.status_label
                    .set_text(&format!("Disconnected from {}", id));
            }

            AppEvent::RemoteError { id, error } => {
                tracing::error!("Remote {} error: {}", id, error);
                self.status_label
                    .set_text(&format!("Connection error: {}", error));
            }

            // --- System integration events ---
            AppEvent::PackageOwnerResult { path, package } => {
                // Route to properties dialog if open and path matches
                let routed = {
                    let dialog_opt = self.properties_dialog.borrow();
                    if let Some(ref dialog) = *dialog_opt {
                        let d = dialog.borrow();
                        if d.file_path() == Some(&path) {
                            d.update_package_info(&package);
                            true
                        } else {
                            false
                        }
                    } else {
                        false
                    }
                };

                if !routed {
                    let msg = match package {
                        Some(pkg) => format!(
                            "{}: {} {} ({})",
                            path.display(),
                            pkg.name,
                            pkg.version,
                            pkg.manager
                        ),
                        None => format!("{}: not owned by any package", path.display()),
                    };
                    self.status_label.set_text(&msg);
                }
            }

            AppEvent::ProcessLocksResult { path, locks } => {
                let routed = {
                    let dialog_opt = self.properties_dialog.borrow();
                    if let Some(ref dialog) = *dialog_opt {
                        let d = dialog.borrow();
                        if d.file_path() == Some(&path) {
                            d.update_process_locks(&locks);
                            true
                        } else {
                            false
                        }
                    } else {
                        false
                    }
                };

                if !routed {
                    if locks.is_empty() {
                        self.status_label
                            .set_text(&format!("{}: no open file handles", path.display()));
                    } else {
                        let names: Vec<String> = locks
                            .iter()
                            .map(|l| format!("{} (PID {})", l.process_name, l.pid))
                            .collect();
                        self.status_label.set_text(&format!(
                            "{}: open by {}",
                            path.display(),
                            names.join(", ")
                        ));
                    }
                }
            }

            AppEvent::DiskUsageProgress { .. } => {
                // Show disk usage indicator and pulse the progress bar
                self.disk_usage_status.set_visible(true);
                self.disk_usage_bar.pulse();
            }

            AppEvent::DiskUsageCompleted {
                path,
                total_size,
                total_items,
            } => {
                self.disk_usage_status.set_visible(false);

                // Route to properties dialog if open
                let routed = {
                    let dialog_opt = self.properties_dialog.borrow();
                    if let Some(ref dialog) = *dialog_opt {
                        let d = dialog.borrow();
                        // Compare using path display since disk usage uses RavenPath
                        d.update_disk_usage(total_size, total_items);
                        true
                    } else {
                        false
                    }
                };

                if !routed {
                    let size_str = format_size(total_size);
                    self.status_label.set_text(&format!(
                        "{}: {} in {} items",
                        path, size_str, total_items
                    ));
                }
            }

            AppEvent::DiskUsageError { path, error } => {
                self.disk_usage_status.set_visible(false);
                self.status_label
                    .set_text(&format!("Disk usage error for {}: {}", path, error));
            }

            AppEvent::ContainerInfoResult { info } => {
                if info.is_container {
                    self.container_banner.show_container_info(
                        &info.container_type.to_string(),
                        info.app_id.as_deref(),
                    );
                }
            }

            AppEvent::SystemdUnitResult { path, unit } => {
                let routed = {
                    let dialog_opt = self.properties_dialog.borrow();
                    if let Some(ref dialog) = *dialog_opt {
                        let d = dialog.borrow();
                        if d.file_path() == Some(&path) {
                            d.update_systemd_unit(&unit);
                            true
                        } else {
                            false
                        }
                    } else {
                        false
                    }
                };

                if !routed {
                    let state = unit
                        .active_state
                        .as_deref()
                        .unwrap_or("unknown");
                    let desc = unit
                        .description
                        .as_deref()
                        .unwrap_or(&unit.name);
                    self.status_label.set_text(&format!(
                        "{}: {} [{}]",
                        path.display(),
                        desc,
                        state
                    ));
                }
            }

            AppEvent::SystemError { error } => {
                tracing::error!("System error: {}", error);
                self.status_label
                    .set_text(&format!("System error: {}", error));
            }

            // --- Directory size updates ---
            AppEvent::DirSizeCalculated {
                pane_id,
                path,
                size,
            } => {
                // Only update if this is still the active pane
                let active_pane_id = self.state.borrow().active_tab().active_pane().id;
                if pane_id == active_pane_id {
                    self.file_list.update_dir_size(&path, size);
                }
            }

            // --- Notifications ---
            AppEvent::Notification {
                title,
                message,
                level,
            } => {
                tracing::info!("[{:?}] {}: {}", level, title, message);
                self.status_label.set_text(&message);
            }
        }
    }

    pub fn present(&self) {
        self.window.present();
    }
}

/// Get the currently selected FileEntry from the file list.
fn get_selected_entry(file_list: &FileListView) -> Option<raven_core::entry::FileEntry> {
    let pos = file_list.selection.selected();
    file_list
        .selection
        .item(pos)
        .and_then(|item| item.downcast::<crate::widgets::file_list::FileEntryObject>().ok())
        .and_then(|obj| obj.entry())
}

/// Show a rename dialog for the given entry.
fn show_rename_dialog(
    parent: &adw::ApplicationWindow,
    entry: &raven_core::entry::FileEntry,
    command_tx: &tokio::sync::mpsc::UnboundedSender<AppCommand>,
) {
    let dialog = adw::Window::builder()
        .title("Rename")
        .default_width(400)
        .default_height(150)
        .modal(true)
        .transient_for(parent)
        .build();

    let toolbar_view = adw::ToolbarView::new();
    let header = adw::HeaderBar::new();
    toolbar_view.add_top_bar(&header);

    let content = gtk::Box::new(gtk::Orientation::Vertical, 12);
    content.set_margin_start(16);
    content.set_margin_end(16);
    content.set_margin_top(12);
    content.set_margin_bottom(16);

    let label = gtk::Label::new(Some("Enter new name:"));
    label.set_halign(gtk::Align::Start);
    content.append(&label);

    let name_entry = gtk::Entry::new();
    name_entry.set_text(&entry.name);
    // Select name without extension
    if let Some(dot_pos) = entry.name.rfind('.') {
        if dot_pos > 0 && !entry.is_dir() {
            name_entry.select_region(0, dot_pos as i32);
        }
    }
    name_entry.set_activates_default(true);
    content.append(&name_entry);

    let btn_box = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    btn_box.set_halign(gtk::Align::End);

    let cancel_btn = gtk::Button::with_label("Cancel");
    {
        let dialog = dialog.clone();
        cancel_btn.connect_clicked(move |_| {
            dialog.close();
        });
    }
    btn_box.append(&cancel_btn);

    let rename_btn = gtk::Button::with_label("Rename");
    rename_btn.add_css_class("suggested-action");
    {
        let dialog = dialog.clone();
        let cmd_tx = command_tx.clone();
        let path = entry.path.clone();
        let name_entry = name_entry.clone();
        rename_btn.connect_clicked(move |_| {
            let new_name = name_entry.text().to_string();
            if !new_name.is_empty() {
                let _ = cmd_tx.send(AppCommand::RenameFile {
                    path: path.clone(),
                    new_name,
                });
                dialog.close();
            }
        });
    }
    btn_box.append(&rename_btn);

    content.append(&btn_box);

    // Enter key submits
    {
        let dialog = dialog.clone();
        let cmd_tx = command_tx.clone();
        let path = entry.path.clone();
        let name_entry_ref = name_entry.clone();
        let key_controller = gtk::EventControllerKey::new();
        key_controller.connect_key_pressed(move |_, key, _, _| {
            match key {
                gtk::gdk::Key::Escape => {
                    dialog.close();
                    return glib::Propagation::Stop;
                }
                gtk::gdk::Key::Return | gtk::gdk::Key::KP_Enter => {
                    let new_name = name_entry_ref.text().to_string();
                    if !new_name.is_empty() {
                        let _ = cmd_tx.send(AppCommand::RenameFile {
                            path: path.clone(),
                            new_name,
                        });
                        dialog.close();
                    }
                    return glib::Propagation::Stop;
                }
                _ => {}
            }
            glib::Propagation::Proceed
        });
        name_entry.add_controller(key_controller);
    }

    toolbar_view.set_content(Some(&content));
    dialog.set_content(Some(&toolbar_view));
    dialog.present();
}

fn navigate_back(
    state: &AppState,
    command_tx: &tokio::sync::mpsc::UnboundedSender<AppCommand>,
    pane_id: u32,
) {
    let path = {
        let mut s = state.borrow_mut();
        if let Some(pane) = s.pane_by_id_mut(pane_id) {
            pane.go_back()
        } else {
            None
        }
    };
    if let Some(path) = path {
        let _ = command_tx.send(AppCommand::Navigate { path, pane_id });
    }
}

fn navigate_forward(
    state: &AppState,
    command_tx: &tokio::sync::mpsc::UnboundedSender<AppCommand>,
    pane_id: u32,
) {
    let path = {
        let mut s = state.borrow_mut();
        if let Some(pane) = s.pane_by_id_mut(pane_id) {
            pane.go_forward()
        } else {
            None
        }
    };
    if let Some(path) = path {
        let _ = command_tx.send(AppCommand::Navigate { path, pane_id });
    }
}

fn navigate_up(
    state: &AppState,
    command_tx: &tokio::sync::mpsc::UnboundedSender<AppCommand>,
    pane_id: u32,
) {
    let parent = {
        let s = state.borrow();
        if let Some(pane) = s.pane_by_id(pane_id) {
            pane.current_path.parent()
        } else {
            None
        }
    };
    if let Some(path) = parent {
        {
            let mut s = state.borrow_mut();
            if let Some(pane) = s.pane_by_id_mut(pane_id) {
                pane.navigate_to(path.clone());
            }
        }
        let _ = command_tx.send(AppCommand::Navigate { path, pane_id });
    }
}

fn format_size(bytes: u64) -> String {
    const KIB: u64 = 1024;
    const MIB: u64 = 1024 * KIB;
    const GIB: u64 = 1024 * MIB;
    const TIB: u64 = 1024 * GIB;

    if bytes >= TIB {
        format!("{:.1} TiB", bytes as f64 / TIB as f64)
    } else if bytes >= GIB {
        format!("{:.1} GiB", bytes as f64 / GIB as f64)
    } else if bytes >= MIB {
        format!("{:.1} MiB", bytes as f64 / MIB as f64)
    } else if bytes >= KIB {
        format!("{:.1} KiB", bytes as f64 / KIB as f64)
    } else {
        format!("{} B", bytes)
    }
}
