use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::rc::Rc;

use gtk4 as gtk;
use libadwaita as adw;
use libadwaita::prelude::*;

use raven_core::commands::AppCommand;
use raven_core::config::{AppConfig, ViewMode};
use raven_core::custom_actions::{ActionTarget, CustomAction};
use raven_core::entry::FileEntry;
use raven_core::events::AppEvent;
use raven_core::operations::OperationId;
use raven_core::path::RavenPath;

use crate::state::{AppState, ClipboardOp, PaneResolver, TagFilter};
use crate::widgets::conflict_dialog::show_conflict_dialog;
use crate::widgets::connect_dialog::ConnectDialog;
use crate::widgets::container_banner::ContainerBanner;
use crate::widgets::context_menu::FileContextMenu;
use crate::widgets::duplicate_dialog::DuplicateDialog;
use crate::widgets::file_list::FileListView;
use crate::widgets::operation_panel::OperationPanel;
use crate::widgets::organize_dialog::OrganizeDialog;
use crate::widgets::pane_view::PaneViews;
use crate::widgets::preview_panel::PreviewPanel;
use crate::widgets::properties_dialog::PropertiesDialog;
use crate::widgets::search_bar::SearchBar;
use crate::widgets::settings_dialog::SettingsDialog;
use crate::widgets::sidebar::Sidebar;
use crate::widgets::tab_bar::TabBar;

mod snapshot;

pub struct RavenWindow {
    pub window: adw::ApplicationWindow,
    pub state: AppState,
    pub command_tx: tokio::sync::mpsc::UnboundedSender<AppCommand>,
    views: Rc<PaneViews>,
    tab_bar: Rc<TabBar>,
    search_bar: Rc<SearchBar>,
    preview_panel: Rc<PreviewPanel>,
    operation_panel: Rc<OperationPanel>,
    status_label: gtk::Label,
    container_banner: Rc<ContainerBanner>,
    // Disk usage status indicator
    disk_usage_status: gtk::Box,
    disk_usage_bar: gtk::ProgressBar,
    // Properties dialog (when open)
    properties_dialog: Rc<RefCell<Option<Rc<RefCell<PropertiesDialog>>>>>,
    // Sidebar (for dynamic bookmark pinning)
    sidebar: Rc<Sidebar>,
    // AI dialogs
    duplicate_dialog: Rc<RefCell<Option<Rc<DuplicateDialog>>>>,
    organize_dialog: Rc<RefCell<Option<Rc<OrganizeDialog>>>>,
    /// Open conflict prompts, keyed by the operation each one blocks.
    conflict_dialogs: RefCell<HashMap<OperationId, adw::Window>>,
    /// The "Connect to Server" dialog while it is open, so a failed attempt
    /// can be reported into it and a successful one closes it.
    connect_dialog: Rc<RefCell<Option<Rc<ConnectDialog>>>>,
    /// Context-menu actions registered by loaded plugins, as last reported.
    plugin_actions: RefCell<Vec<raven_core::events::PluginActionInfo>>,
    /// The header's preview toggle and list/icon/preview buttons, which the
    /// snapshot mode presses the way a person would.
    preview_btn: gtk::ToggleButton,
    view_buttons: [gtk::ToggleButton; 3],
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

        // Widgets that always act on the active pane look its id up as they go.
        let active_resolver: PaneResolver = {
            let state = state.clone();
            Rc::new(move || state.borrow().active_tab().active_pane().id)
        };

        // The two panes, built first because header buttons act on them.
        let views = PaneViews::new(state.clone(), command_tx.clone());
        let file_list = ActiveList {
            views: views.clone(),
        };

        // Main layout
        let toolbar_view = adw::ToolbarView::new();

        // --- Header bar ---
        let header = adw::HeaderBar::new();
        header.add_css_class("main-header");

        // Navigation buttons: flat, as everything in the header is but the
        // view switch, which is the one control there showing a choice.
        let nav_box = gtk::Box::new(gtk::Orientation::Horizontal, 2);
        nav_box.add_css_class("nav-buttons");

        let back_btn = gtk::Button::from_icon_name("go-previous-symbolic");
        back_btn.add_css_class("flat");
        back_btn.set_tooltip_text(Some("Back (Alt+Left)"));
        {
            let state = state.clone();
            let cmd_tx = command_tx.clone();
            back_btn.connect_clicked(move |_| {
                navigate_back(&state, &cmd_tx, active_pane_id(&state));
            });
        }
        nav_box.append(&back_btn);

        let forward_btn = gtk::Button::from_icon_name("go-next-symbolic");
        forward_btn.add_css_class("flat");
        forward_btn.set_tooltip_text(Some("Forward (Alt+Right)"));
        {
            let state = state.clone();
            let cmd_tx = command_tx.clone();
            forward_btn.connect_clicked(move |_| {
                navigate_forward(&state, &cmd_tx, active_pane_id(&state));
            });
        }
        nav_box.append(&forward_btn);

        let up_btn = gtk::Button::from_icon_name("go-up-symbolic");
        up_btn.add_css_class("flat");
        up_btn.set_tooltip_text(Some("Up (Alt+Up)"));
        {
            let state = state.clone();
            let cmd_tx = command_tx.clone();
            up_btn.connect_clicked(move |_| {
                navigate_up(&state, &cmd_tx, active_pane_id(&state));
            });
        }
        nav_box.append(&up_btn);

        header.pack_start(&nav_box);

        // View mode toggle buttons: a segmented control, the Raven way.
        let view_mode_box = gtk::Box::new(gtk::Orientation::Horizontal, 2);
        view_mode_box.add_css_class("view-switch");

        let list_btn = gtk::ToggleButton::new();
        list_btn.set_icon_name("view-list-symbolic");
        list_btn.set_tooltip_text(Some("List view"));
        view_mode_box.append(&list_btn);

        let icons_btn = gtk::ToggleButton::new();
        icons_btn.set_icon_name("view-grid-symbolic");
        icons_btn.set_tooltip_text(Some("Icon view"));
        icons_btn.set_group(Some(&list_btn));
        view_mode_box.append(&icons_btn);

        let previews_btn = gtk::ToggleButton::new();
        previews_btn.set_icon_name("view-app-grid-symbolic");
        previews_btn.set_tooltip_text(Some("Preview view"));
        previews_btn.set_group(Some(&list_btn));
        view_mode_box.append(&previews_btn);

        // Set initial active button from state
        {
            let s = state.borrow();
            match s.view_mode {
                ViewMode::List => list_btn.set_active(true),
                ViewMode::Icons => icons_btn.set_active(true),
                ViewMode::Previews => previews_btn.set_active(true),
            }
        }

        header.pack_start(&view_mode_box);

        let dual_btn = gtk::Button::from_icon_name("view-dual-symbolic");
        dual_btn.add_css_class("flat");
        dual_btn.set_tooltip_text(Some("Dual pane (F3)"));
        {
            let views = views.clone();
            dual_btn.connect_clicked(move |_| views.toggle_dual());
        }
        header.pack_start(&dual_btn);

        // Settings button (hamburger menu)
        let settings_btn = gtk::Button::from_icon_name("open-menu-symbolic");
        settings_btn.add_css_class("flat");
        settings_btn.set_tooltip_text(Some("Settings (Ctrl+,)"));
        {
            let state = state.clone();
            let window_ref = window.clone();
            let cmd_tx = command_tx.clone();
            settings_btn.connect_clicked(move |_| {
                let dialog = SettingsDialog::new(&window_ref, state.clone(), cmd_tx.clone());
                dialog.present();
            });
        }
        header.pack_end(&settings_btn);

        // Search toggle button
        let search_btn = gtk::ToggleButton::new();
        search_btn.set_icon_name("system-search-symbolic");
        search_btn.add_css_class("flat");
        search_btn.set_tooltip_text(Some("Search (Ctrl+F)"));
        header.pack_end(&search_btn);

        // Hidden files toggle
        let hidden_btn = gtk::ToggleButton::new();
        hidden_btn.set_icon_name("view-reveal-symbolic");
        hidden_btn.add_css_class("flat");
        hidden_btn.set_tooltip_text(Some("Show hidden files (Ctrl+H)"));
        header.pack_end(&hidden_btn);

        toolbar_view.add_top_bar(&header);

        // --- Container banner ---
        let container_banner = Rc::new(ContainerBanner::new());
        toolbar_view.add_top_bar(&container_banner.revealer);

        // --- Tab bar ---
        let tab_bar = Rc::new(TabBar::new(state.clone(), command_tx.clone()));
        toolbar_view.add_top_bar(&tab_bar.widget);
        // The strip hides itself while there is one tab, so opening a tab
        // needs a home that is always there.
        header.pack_start(&tab_bar.new_tab_button());

        // --- Search bar ---
        let get_path = {
            let state = state.clone();
            move || -> Option<RavenPath> {
                let s = state.borrow();
                Some(s.active_tab().active_pane().current_path.clone())
            }
        };
        let search_bar = Rc::new(SearchBar::new(
            command_tx.clone(),
            get_path,
            active_resolver.clone(),
        ));
        toolbar_view.add_top_bar(&search_bar.revealer);

        // Wire search toggle button to search bar. The bar can also close itself (its
        // X button, Escape), so the button tracks the revealer rather than being the
        // sole owner of the open/closed state.
        {
            let sb = search_bar.clone();
            search_btn.connect_toggled(move |btn| {
                // Skip when the bar is already in the requested state: this handler
                // also runs when the button is being synced *from* the revealer below,
                // and acting again would re-send the dismissal.
                if btn.is_active() {
                    if !sb.revealer.reveals_child() {
                        sb.show();
                    }
                } else if sb.revealer.reveals_child() {
                    sb.hide();
                }
            });
        }
        {
            let btn = search_btn.clone();
            search_bar
                .revealer
                .connect_reveal_child_notify(move |revealer| {
                    btn.set_active(revealer.reveals_child());
                });
        }

        // --- Content area: sidebar + file list + preview panel ---
        let content_box = gtk::Box::new(gtk::Orientation::Horizontal, 0);

        // Sidebar
        let sidebar = Rc::new(Sidebar::new(
            state.clone(),
            command_tx.clone(),
            active_resolver.clone(),
        ));
        let sidebar_scroll = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vscrollbar_policy(gtk::PolicyType::Automatic)
            .child(&sidebar.widget)
            .build();
        sidebar_scroll.set_width_request(200);
        // Raven Glass's sidebar: its own darker layer and a hairline edge,
        // so no separator is needed beside it.
        sidebar_scroll.add_css_class("sidebar");
        sidebar_scroll.add_css_class("places");

        content_box.append(&sidebar_scroll);
        content_box.append(&views.paned);

        // --- Context menu state shared by both panes' menus ---
        let context_selected_tags: Rc<RefCell<HashSet<String>>> =
            Rc::new(RefCell::new(HashSet::new()));

        // Register file actions on the column_view
        let action_group = gio::SimpleActionGroup::new();

        // The status label is built after the actions; opening failures reach
        // it through this slot, filled once the label exists.
        let open_status: Rc<RefCell<Option<gtk::Label>>> = Rc::new(RefCell::new(None));

        // file.open
        {
            let file_list = file_list.clone();
            let state = state.clone();
            let cmd_tx = command_tx.clone();
            let open_status = open_status.clone();
            let action = gio::SimpleAction::new("open", None);
            action.connect_activate(move |_, _| {
                let selected = get_selected_entries(&file_list.get());
                if selected.is_empty() {
                    return;
                }

                if selected.len() == 1 {
                    let entry = &selected[0];
                    if entry.is_dir() {
                        let pane_id = active_pane_id(&state);
                        {
                            let mut s = state.borrow_mut();
                            if let Some(pane) = s.pane_by_id_mut(pane_id) {
                                pane.navigate_to(entry.path.clone());
                            }
                        }
                        let _ = cmd_tx.send(AppCommand::Navigate {
                            path: entry.path.clone(),
                            pane_id,
                        });
                    } else {
                        let config = state.borrow().config.clone();
                        if let Err(e) = crate::file_opener::try_open_file(&entry.path, &config) {
                            report_open_error(&open_status, &e);
                        }
                    }
                    return;
                }

                let config = state.borrow().config.clone();
                for entry in selected {
                    if !entry.is_dir() {
                        if let Err(e) = crate::file_opener::try_open_file(&entry.path, &config) {
                            report_open_error(&open_status, &e);
                        }
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
                let paths = get_selected_paths(&file_list.get());
                if !paths.is_empty() {
                    let mut s = state.borrow_mut();
                    s.clipboard = Some(ClipboardOp::Copy(paths));
                }
            });
            action_group.add_action(&action);
        }

        // file.copy-path: copy file path to system clipboard
        {
            let file_list = file_list.clone();
            let window_ref = window.clone();
            let action = gio::SimpleAction::new("copy-path", None);
            action.connect_activate(move |_, _| {
                let paths = get_selected_paths(&file_list.get());
                if !paths.is_empty() {
                    let text = paths
                        .iter()
                        .filter_map(|p| p.as_local_path())
                        .map(|p| p.display().to_string())
                        .collect::<Vec<_>>()
                        .join("\n");
                    if !text.is_empty() {
                        let display = gtk::prelude::RootExt::display(&window_ref);
                        let clipboard = display.clipboard();
                        clipboard.set_text(&text);
                    }
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
                let paths = get_selected_paths(&file_list.get());
                if !paths.is_empty() {
                    let mut s = state.borrow_mut();
                    s.clipboard = Some(ClipboardOp::Cut(paths));
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
                if let Some(entry) = get_primary_selected_entry(&file_list.get()) {
                    show_rename_dialog(&window_ref, &entry, &cmd_tx);
                }
            });
            action_group.add_action(&action);
        }

        // file.new_folder / file.new_file — created in the directory being shown.
        for (name, kind) in [("new_folder", NewItem::Folder), ("new_file", NewItem::File)] {
            let state = state.clone();
            let cmd_tx = command_tx.clone();
            let window_ref = window.clone();
            let action = gio::SimpleAction::new(name, None);
            action.connect_activate(move |_, _| {
                show_new_item_dialog(&window_ref, &state, &cmd_tx, active_pane_id(&state), kind);
            });
            action_group.add_action(&action);
        }

        // file.trash — move to Trash, or permanently delete if already in Trash.
        {
            let file_list = file_list.clone();
            let cmd_tx = command_tx.clone();
            let state = state.clone();
            let action = gio::SimpleAction::new("trash", None);
            action.connect_activate(move |_, _| {
                let paths = get_selected_paths(&file_list.get());
                if !paths.is_empty() {
                    let in_trash = {
                        let s = state.borrow();
                        is_in_trash(&s.active_tab().active_pane().current_path)
                    };
                    if in_trash {
                        let _ = cmd_tx.send(AppCommand::DeleteFiles { paths });
                    } else {
                        let _ = cmd_tx.send(AppCommand::TrashFiles { paths });
                    }
                }
            });
            action_group.add_action(&action);
        }

        // file.delete — permanently delete if in Trash, otherwise move to Trash.
        {
            let file_list = file_list.clone();
            let cmd_tx = command_tx.clone();
            let state = state.clone();
            let action = gio::SimpleAction::new("delete", None);
            action.connect_activate(move |_, _| {
                let paths = get_selected_paths(&file_list.get());
                if !paths.is_empty() {
                    let in_trash = {
                        let s = state.borrow();
                        is_in_trash(&s.active_tab().active_pane().current_path)
                    };
                    if in_trash {
                        let _ = cmd_tx.send(AppCommand::DeleteFiles { paths });
                    } else {
                        let _ = cmd_tx.send(AppCommand::TrashFiles { paths });
                    }
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
                if let Some(entry) = get_primary_selected_entry(&file_list.get()) {
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
                if let Some(entry) = get_primary_selected_entry(&file_list.get()) {
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

        // file.find_duplicates
        let duplicate_dialog: Rc<RefCell<Option<Rc<DuplicateDialog>>>> =
            Rc::new(RefCell::new(None));
        {
            let cmd_tx = command_tx.clone();
            let state = state.clone();
            let window_ref = window.clone();
            let duplicate_dialog = duplicate_dialog.clone();
            let action = gio::SimpleAction::new("find_duplicates", None);
            action.connect_activate(move |_, _| {
                let path = {
                    let s = state.borrow();
                    s.active_tab().active_pane().current_path.clone()
                };
                let dialog = Rc::new(DuplicateDialog::new(&window_ref, cmd_tx.clone()));
                dialog.present();
                *duplicate_dialog.borrow_mut() = Some(dialog);
                let _ = cmd_tx.send(AppCommand::ScanDuplicates {
                    path,
                    recursive: true,
                    min_size: 1,
                });
            });
            action_group.add_action(&action);
        }

        // file.suggest_organization
        let organize_dialog: Rc<RefCell<Option<Rc<OrganizeDialog>>>> = Rc::new(RefCell::new(None));
        {
            let cmd_tx = command_tx.clone();
            let state = state.clone();
            let window_ref = window.clone();
            let organize_dialog = organize_dialog.clone();
            let action = gio::SimpleAction::new("suggest_organization", None);
            action.connect_activate(move |_, _| {
                let path = {
                    let s = state.borrow();
                    s.active_tab().active_pane().current_path.clone()
                };
                let dialog = Rc::new(OrganizeDialog::new(&window_ref, cmd_tx.clone()));
                dialog.present();
                *organize_dialog.borrow_mut() = Some(dialog);
                let _ = cmd_tx.send(AppCommand::AnalyzeOrganization { path });
            });
            action_group.add_action(&action);
        }

        // file.toggle-tag(tag-name)
        {
            let file_list = file_list.clone();
            let cmd_tx = command_tx.clone();
            let state = state.clone();
            let context_selected_tags = context_selected_tags.clone();
            let action = gio::SimpleAction::new("toggle-tag", Some(glib::VariantTy::STRING));
            action.connect_activate(move |_, parameter| {
                let Some(tag_name) = parameter.and_then(|value| value.get::<String>()) else {
                    return;
                };

                let selected_paths = get_selected_local_paths(&file_list.get());
                if selected_paths.is_empty() {
                    return;
                }

                let should_remove = context_selected_tags.borrow().contains(&tag_name);
                for path in selected_paths {
                    let command = if should_remove {
                        AppCommand::RemoveManualTag {
                            path: path.clone(),
                            tag: tag_name.clone(),
                        }
                    } else {
                        AppCommand::AddManualTag {
                            path: path.clone(),
                            tag: tag_name.clone(),
                        }
                    };
                    let _ = cmd_tx.send(command);
                }

                // Manual tags feed the sidebar counts, so recount the pane's directory.
                let (path, pane_id) = {
                    let s = state.borrow();
                    let pane = s.active_tab().active_pane();
                    (pane.current_path.clone(), pane.id)
                };
                let _ = cmd_tx.send(AppCommand::RefreshTagCounts { pane_id, path });
            });
            action_group.add_action(&action);
        }

        // file.open-with(target): "app:<desktop-id>", "assoc:<index>" or "other"
        // (see file_opener::OpenWithTarget). Applications get the whole
        // selection in one launch; config associations run once per file.
        {
            let file_list = file_list.clone();
            let state = state.clone();
            let open_status = open_status.clone();
            let window_ref = window.clone();
            let action = gio::SimpleAction::new("open-with", Some(glib::VariantTy::STRING));
            action.connect_activate(move |_, parameter| {
                use crate::file_opener::OpenWithTarget;
                let Some(target) = parameter
                    .and_then(|value| value.get::<String>())
                    .and_then(|s| OpenWithTarget::parse(&s))
                else {
                    return;
                };

                let selected = get_selected_entries(&file_list.get());
                if selected.is_empty() {
                    return;
                }
                let display = gtk::prelude::RootExt::display(&window_ref);

                match target {
                    OpenWithTarget::App(id) => {
                        let paths: Vec<RavenPath> =
                            selected.iter().map(|e| e.path.clone()).collect();
                        let result = match crate::file_opener::app_by_id(&id) {
                            Some(app) => crate::file_opener::launch_app_with(&app, &paths, &display),
                            None => Err(format!("Application {} is no longer installed", id)),
                        };
                        if let Err(e) = result {
                            report_open_error(&open_status, &e);
                        }
                    }
                    OpenWithTarget::Association(index) => {
                        let config = state.borrow().config.clone();
                        for entry in selected.iter().filter(|e| !e.is_dir()) {
                            if let Err(e) = crate::file_opener::open_with_association(
                                &entry.path,
                                &config,
                                index,
                            ) {
                                report_open_error(&open_status, &e);
                            }
                        }
                    }
                    OpenWithTarget::Other => {
                        let primary = &selected[0];
                        let content_type = if primary.is_dir() {
                            "inode/directory".to_string()
                        } else {
                            primary
                                .metadata
                                .mime_type
                                .clone()
                                .filter(|m| !m.is_empty())
                                .or_else(|| {
                                    primary
                                        .path
                                        .as_local_path()
                                        .map(|p| crate::file_opener::content_type_for_path(p))
                                })
                                .unwrap_or_else(|| "application/octet-stream".to_string())
                        };
                        let paths: Vec<RavenPath> =
                            selected.iter().map(|e| e.path.clone()).collect();
                        let open_status = open_status.clone();
                        let chooser_type = content_type.clone();
                        crate::widgets::app_chooser_dialog::show_app_chooser(
                            &window_ref,
                            &content_type,
                            move |app, always| {
                                if always {
                                    if let Err(e) = app.set_as_default_for_type(&chooser_type) {
                                        report_open_error(
                                            &open_status,
                                            &format!(
                                                "Could not make {} the default: {}",
                                                app.display_name(),
                                                e.message()
                                            ),
                                        );
                                    }
                                }
                                if let Err(e) =
                                    crate::file_opener::launch_app_with(&app, &paths, &display)
                                {
                                    report_open_error(&open_status, &e);
                                }
                            },
                        );
                    }
                }
            });
            action_group.add_action(&action);
        }

        // file.custom-action(index): a command from actions.toml on the selection
        {
            let file_list = file_list.clone();
            let state = state.clone();
            let action = gio::SimpleAction::new("custom-action", Some(glib::VariantTy::INT32));
            action.connect_activate(move |_, parameter| {
                let Some(idx) = parameter.and_then(|v| v.get::<i32>()) else {
                    return;
                };
                if idx < 0 {
                    return;
                }
                let selected = get_selected_entries(&file_list.get());
                let (custom, current) = {
                    let s = state.borrow();
                    (
                        s.custom_actions.get(idx as usize).cloned(),
                        s.active_tab().active_pane().current_path.clone(),
                    )
                };
                let Some(custom) = custom else { return };
                let Some(current_local) = current.as_local_path() else {
                    return;
                };
                let Some(target) = ActionTarget::from_selection(&selected, current_local) else {
                    return;
                };
                run_custom_action(&custom, &target);
            });
            action_group.add_action(&action);
        }

        // file.plugin-action((plugin_id, action)): hand the selection to the
        // plugin that registered the action. The selection is published first
        // so a plugin asking get_selection sees exactly what it was run on.
        {
            let file_list = file_list.clone();
            let cmd_tx = command_tx.clone();
            let action = gio::SimpleAction::new(
                "plugin-action",
                Some(glib::VariantTy::new("(ss)").expect("valid variant type")),
            );
            action.connect_activate(move |_, parameter| {
                let Some((plugin_id, name)) = parameter.and_then(|v| v.get::<(String, String)>())
                else {
                    return;
                };
                let selected = get_selected_entries(&file_list.get());
                let shown = local_path_strings(&selected);
                raven_plugin::selection::SharedSelection::global().set(shown);
                let _ = cmd_tx.send(AppCommand::RunPluginAction {
                    plugin_id,
                    action: name,
                    paths: selected
                        .iter()
                        .filter_map(|e| e.path.as_local_path().cloned())
                        .collect(),
                });
            });
            action_group.add_action(&action);
        }

        // file.open_containing_folder: show the selection in its own folder, for
        // listings that are not a folder (Recent, search results). Enabled on
        // right-click only when it would go somewhere else.
        let reveal_action = gio::SimpleAction::new("open_containing_folder", None);
        {
            let file_list = file_list.clone();
            let state = state.clone();
            let cmd_tx = command_tx.clone();
            reveal_action.connect_activate(move |_, _| {
                let selected = get_selected_paths(&file_list.get());
                let Some(parent) = selected.first().and_then(|p| p.parent()) else {
                    return;
                };
                // A reveal selects within one folder: the first item's.
                let paths: Vec<RavenPath> = selected
                    .into_iter()
                    .filter(|p| p.parent().as_ref() == Some(&parent))
                    .collect();
                let pane_id = active_pane_id(&state);
                {
                    let mut s = state.borrow_mut();
                    if let Some(pane) = s.pane_by_id_mut(pane_id) {
                        pane.navigate_to(parent.clone());
                        // Pending rather than RevealItems: its SelectItems would
                        // land while the old listing (Recent, search results)
                        // is still on screen, match the file there and be spent
                        // before the folder arrives. DirectoryLoaded applies it.
                        pane.pending_selection = paths;
                        pane.pending_properties = false;
                    }
                }
                let _ = cmd_tx.send(AppCommand::Navigate {
                    path: parent,
                    pane_id,
                });
            });
            action_group.add_action(&reveal_action);
        }

        // Register on the Stack itself so the PopoverMenu (parented to the Stack)
        // can resolve "file.*" actions by walking up the widget tree.
        for pane_view in [&views.left, &views.right] {
            pane_view
                .file_list
                .widget
                .insert_action_group("file", Some(&action_group));
            for list_widget in pane_view.list_widgets() {
                list_widget.insert_action_group("file", Some(&action_group));
            }
            // Below "Open", hidden whenever the action is disabled.
            if let Some(menu) = pane_view
                .context_menu
                .popover
                .menu_model()
                .and_downcast::<gio::Menu>()
            {
                let item = gio::MenuItem::new(
                    Some("Open Containing Folder"),
                    Some("file.open_containing_folder"),
                );
                item.set_attribute_value("hidden-when", Some(&"action-disabled".to_variant()));
                let section = gio::Menu::new();
                section.append_item(&item);
                menu.insert_section(1, None, &section);
            }
        }

        // Right-click gesture for the context menu, on every view of both panes.
        let pane_lists: Vec<(Rc<FileContextMenu>, Rc<FileListView>, gtk::Widget)> =
            [&views.left, &views.right]
                .into_iter()
                .flat_map(|pane_view| {
                    pane_view.list_widgets().into_iter().map(move |widget| {
                        (
                            pane_view.context_menu.clone(),
                            pane_view.file_list.clone(),
                            widget,
                        )
                    })
                })
                .collect();
        for (context_menu, file_list_for_ctx, view_widget) in pane_lists {
            let file_list_for_released = file_list_for_ctx.clone();
            let state = state.clone();
            let context_selected_tags = context_selected_tags.clone();
            let reveal_action = reveal_action.clone();
            let gesture = gtk::GestureClick::new();
            gesture.set_button(3); // Right click

            // connect_pressed: select the item under the cursor immediately on press.
            // Doing this here (not in connect_released) prevents visual race conditions
            // where the popover appears before GTK has finished rendering the selection.
            gesture.connect_pressed(move |gesture, _, x, y| {
                if let Some(view_widget) = gesture.widget() {
                    if let Some(mut cur) = view_widget.pick(x, y, gtk::PickFlags::DEFAULT) {
                        loop {
                            let found_pos: Option<u32> = unsafe {
                                cur.data::<Rc<RefCell<u32>>>("item-pos")
                                    .map(|ptr: std::ptr::NonNull<Rc<RefCell<u32>>>| {
                                        *ptr.as_ref().borrow()
                                    })
                            };
                            if let Some(pos) = found_pos {
                                if pos != u32::MAX
                                    && !file_list_for_ctx.selection.is_selected(pos)
                                {
                                    file_list_for_ctx.selection.select_item(pos, true);
                                }
                                break;
                            }
                            if cur == view_widget {
                                break;
                            }
                            match cur.parent() {
                                Some(p) => cur = p,
                                None => break,
                            }
                        }
                    }
                }
            });

            // connect_released: populate and show the context menu.
            // By this point the item is already selected (set in connect_pressed above).
            gesture.connect_released(move |_, _, x, y| {
                {
                    let selected = get_selected_entries(&file_list_for_released);
                    let actions = state.borrow().custom_actions.clone();
                    context_menu.update_custom_actions(&actions, &selected);
                    // The right-click made this pane the active one.
                    let elsewhere = {
                        let s = state.borrow();
                        let pane = s.active_tab().active_pane();
                        !selected.is_empty()
                            && (pane.recent
                                || selected
                                    .iter()
                                    .any(|e| e.path.parent().as_ref() != Some(&pane.current_path)))
                    };
                    reveal_action.set_enabled(elsewhere);
                }
                if let Some(entry) = get_primary_selected_entry(&file_list_for_released) {
                    let config = state.borrow().config.clone();
                    context_menu.update_open_with(&entry, &config);

                    let (all_tags, current_tags) = load_tag_menu_data(&config, &entry);
                    context_menu.update_tags(&all_tags, &current_tags);
                    *context_selected_tags.borrow_mut() =
                        current_tags.into_iter().collect::<HashSet<String>>();
                } else {
                    context_menu.clear_open_with();
                    context_menu.update_tags(&[], &[]);
                    context_selected_tags.borrow_mut().clear();
                }

                let rect = gtk::gdk::Rectangle::new(x as i32, y as i32, 1, 1);
                context_menu.popover.set_pointing_to(Some(&rect));
                context_menu.popover.popup();
            });
            view_widget.add_controller(gesture);
        }

        // Preview panel, hidden until toggled. It shares the panes' space
        // through a Paned so it can be dragged wider; its width and whether it
        // is open are kept in the config across restarts.
        let preview_panel = PreviewPanel::new();
        preview_panel.widget.set_visible(false);
        let preview_paned = gtk::Paned::new(gtk::Orientation::Horizontal);
        preview_paned.set_hexpand(true);
        preview_paned.set_vexpand(true);
        // Resizing the window resizes the panes, not the panel.
        preview_paned.set_resize_start_child(true);
        preview_paned.set_resize_end_child(false);
        preview_paned.set_shrink_start_child(false);
        preview_paned.set_shrink_end_child(false);
        // The panes were put in the content area above; move them into the
        // Paned, beside the panel.
        content_box.remove(&views.paned);
        preview_paned.set_start_child(Some(&views.paned));
        preview_paned.set_end_child(Some(&preview_panel.widget));
        content_box.append(&preview_paned);

        let preview_btn = gtk::ToggleButton::new();
        preview_btn.set_icon_name("sidebar-show-right-symbolic");
        preview_btn.add_css_class("flat");
        preview_btn.set_tooltip_text(Some("Preview panel (Space)"));
        header.pack_end(&preview_btn);
        {
            let panel = preview_panel.clone();
            let paned = preview_paned.clone();
            let views = views.clone();
            let state = state.clone();
            let cmd_tx = command_tx.clone();
            preview_btn.connect_toggled(move |btn| {
                let show = btn.is_active();
                panel.widget.set_visible(show);
                let width = {
                    let mut s = state.borrow_mut();
                    if s.config.appearance.preview_panel_visible != show {
                        s.config.appearance.preview_panel_visible = show;
                        let _ = s.config.save();
                    }
                    s.config.appearance.preview_panel_width
                };
                if show {
                    place_preview_divider(&paned, width);
                    preview_selection(&views, &panel, &cmd_tx);
                }
            });
        }
        // Remember the width the panel is dragged to, saved once the drag
        // settles rather than on every pixel. Positions GTK picks by itself
        // (before anything was placed) are not the user's choice.
        {
            let panel = preview_panel.clone();
            let state = state.clone();
            let pending: Rc<RefCell<Option<glib::SourceId>>> = Rc::new(RefCell::new(None));
            preview_paned.connect_position_notify(move |paned| {
                let total = paned.width();
                if !panel.widget.is_visible() || !paned.is_position_set() || total <= 0 {
                    return;
                }
                let width = total - paned.position();
                {
                    let mut s = state.borrow_mut();
                    if width < 1 || s.config.appearance.preview_panel_width == width {
                        return;
                    }
                    s.config.appearance.preview_panel_width = width;
                }
                if let Some(id) = pending.borrow_mut().take() {
                    id.remove();
                }
                let state = state.clone();
                let fired = pending.clone();
                let id = glib::timeout_add_local_once(
                    std::time::Duration::from_millis(500),
                    move || {
                        fired.borrow_mut().take();
                        let _ = state.borrow().config.save();
                    },
                );
                *pending.borrow_mut() = Some(id);
            });
        }
        let reopen_preview = state.borrow().config.appearance.preview_panel_visible;
        if reopen_preview {
            preview_btn.set_active(true);
        }

        // Publish the active pane's selection for plugins and D-Bus clients:
        // on every selection or listing change in either pane, and when the
        // other pane becomes active. Coalesced into one idle callback so a
        // select-all or a long arrow-key run costs one walk of the list.
        {
            let scheduled = Rc::new(std::cell::Cell::new(false));
            let schedule: Rc<dyn Fn()> = {
                let views = views.clone();
                let scheduled = scheduled.clone();
                Rc::new(move || {
                    if scheduled.replace(true) {
                        return;
                    }
                    let views = views.clone();
                    let scheduled = scheduled.clone();
                    glib::idle_add_local_once(move || {
                        scheduled.set(false);
                        let selected = get_selected_entries(&views.active().file_list);
                        raven_plugin::selection::SharedSelection::global()
                            .set(local_path_strings(&selected));
                    });
                })
            };
            for side in [crate::state::PaneSide::Left, crate::state::PaneSide::Right] {
                let view = views.view(side);
                {
                    let schedule = schedule.clone();
                    view.file_list
                        .selection
                        .connect_selection_changed(move |_, _, _| schedule());
                }
                {
                    let schedule = schedule.clone();
                    view.file_list
                        .selection
                        .connect_items_changed(move |_, _, _, _| schedule());
                }
                let focus = gtk::EventControllerFocus::new();
                {
                    let schedule = schedule.clone();
                    focus.connect_enter(move |_| schedule());
                }
                view.container.add_controller(focus);
                let click = gtk::GestureClick::new();
                click.set_button(0);
                click.set_propagation_phase(gtk::PropagationPhase::Capture);
                {
                    let schedule = schedule.clone();
                    click.connect_pressed(move |_, _, _, _| schedule());
                }
                view.container.add_controller(click);
            }
        }

        // Follow the selection while the panel is open. Debounced so holding
        // an arrow key through a folder does not queue a preview per file.
        {
            let pending: Rc<RefCell<Option<gtk::glib::SourceId>>> = Rc::new(RefCell::new(None));
            for file_list in [views.left.file_list.clone(), views.right.file_list.clone()] {
                let panel = preview_panel.clone();
                let views = views.clone();
                let cmd_tx = command_tx.clone();
                let pending = pending.clone();
                file_list.selection.connect_selection_changed(move |_, _, _| {
                    if !panel.widget.is_visible() {
                        return;
                    }
                    if let Some(id) = pending.borrow_mut().take() {
                        id.remove();
                    }
                    let panel = panel.clone();
                    let views = views.clone();
                    let cmd_tx = cmd_tx.clone();
                    let fired = pending.clone();
                    let id = gtk::glib::timeout_add_local_once(
                        std::time::Duration::from_millis(80),
                        move || {
                            fired.borrow_mut().take();
                            preview_selection(&views, &panel, &cmd_tx);
                        },
                    );
                    *pending.borrow_mut() = Some(id);
                });
            }
        }

        // Focus or a click moving to the other pane makes it the active one
        // (PaneViews does that); the preview follows. Deferred so it runs
        // after PaneViews has updated the active side.
        for side in [crate::state::PaneSide::Left, crate::state::PaneSide::Right] {
            let container = views.view(side).container.clone();
            let hook = {
                let state = state.clone();
                let views = views.clone();
                let panel = preview_panel.clone();
                let cmd_tx = command_tx.clone();
                // Cheap when nothing changed: the selection already shown is
                // not asked for again, and a valid preview is kept.
                move || {
                    let state = state.clone();
                    let views = views.clone();
                    let panel = panel.clone();
                    let cmd_tx = cmd_tx.clone();
                    glib::idle_add_local_once(move || {
                        sync_preview(&state, &views, &panel, &cmd_tx);
                    });
                }
            };
            let focus = gtk::EventControllerFocus::new();
            {
                let hook = hook.clone();
                focus.connect_enter(move |_| hook());
            }
            container.add_controller(focus);
            let click = gtk::GestureClick::new();
            click.set_button(0);
            click.set_propagation_phase(gtk::PropagationPhase::Capture);
            click.connect_pressed(move |_, _, _, _| hook());
            container.add_controller(click);
        }

        {
            let state = state.clone();
            let cmd_tx = command_tx.clone();
            let open_status = open_status.clone();
            preview_panel.connect_open(move |path| {
                if path.as_local_path().is_some_and(|p| p.is_dir()) {
                    let pane_id = active_pane_id(&state);
                    if let Some(pane) = state.borrow_mut().pane_by_id_mut(pane_id) {
                        pane.navigate_to(path.clone());
                    }
                    let _ = cmd_tx.send(AppCommand::Navigate {
                        path: path.clone(),
                        pane_id,
                    });
                } else {
                    let config = state.borrow().config.clone();
                    if let Err(e) = crate::file_opener::try_open_file(path, &config) {
                        report_open_error(&open_status, &e);
                    }
                }
            });
        }

        toolbar_view.set_content(Some(&content_box));

        // --- Operation panel ---
        let operation_panel = Rc::new(OperationPanel::new(command_tx.clone()));
        toolbar_view.add_bottom_bar(&operation_panel.revealer);

        // --- Status bar ---
        let status_bar = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        status_bar.add_css_class("status-bar");
        let status_label = gtk::Label::new(Some("Ready"));
        status_label.set_halign(gtk::Align::Start);
        status_label.set_hexpand(true);
        status_label.set_ellipsize(gtk::pango::EllipsizeMode::End);
        status_bar.append(&status_label);
        *open_status.borrow_mut() = Some(status_label.clone());

        // Disk usage status indicator (initially hidden)
        let disk_usage_status = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        disk_usage_status.set_visible(false);

        let du_label = gtk::Label::new(Some("Calculating..."));
        disk_usage_status.append(&du_label);

        let disk_usage_bar = gtk::ProgressBar::new();
        disk_usage_bar.set_width_request(120);
        disk_usage_bar.set_valign(gtk::Align::Center);
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
            let views = views.clone();
            hidden_btn.connect_toggled(move |btn| {
                state.borrow_mut().show_hidden = btn.is_active();
                views.render_all();
            });
        }

        // --- View mode toggle callbacks ---
        {
            let views = views.clone();
            let state = state.clone();
            list_btn.connect_toggled(move |btn| {
                if btn.is_active() {
                    views.set_view_mode(ViewMode::List);
                    let mut s = state.borrow_mut();
                    s.view_mode = ViewMode::List;
                    s.config.appearance.view_mode = ViewMode::List;
                    let _ = s.config.save();
                }
            });
        }
        {
            let views = views.clone();
            let state = state.clone();
            icons_btn.connect_toggled(move |btn| {
                if btn.is_active() {
                    views.set_view_mode(ViewMode::Icons);
                    let mut s = state.borrow_mut();
                    s.view_mode = ViewMode::Icons;
                    s.config.appearance.view_mode = ViewMode::Icons;
                    let _ = s.config.save();
                }
            });
        }
        {
            let views = views.clone();
            let state = state.clone();
            previews_btn.connect_toggled(move |btn| {
                if btn.is_active() {
                    views.set_view_mode(ViewMode::Previews);
                    let mut s = state.borrow_mut();
                    s.view_mode = ViewMode::Previews;
                    s.config.appearance.view_mode = ViewMode::Previews;
                    let _ = s.config.save();
                }
            });
        }

        // --- Tab bar callbacks ---
        {
            let views = views.clone();
            let state = state.clone();
            let panel = preview_panel.clone();
            let cmd_tx = command_tx.clone();
            tab_bar.set_on_tab_changed(move |_| {
                views.show_active_tab();
                sync_preview(&state, &views, &panel, &cmd_tx);
            });
        }

        // --- Keyboard shortcuts, from Settings > Keybindings ---
        {
            let key_controller = gtk::EventControllerKey::new();
            let ctx = KeyContext {
                window: window.clone(),
                state: state.clone(),
                cmd_tx: command_tx.clone(),
                views: views.clone(),
                hidden_btn: hidden_btn.clone(),
                search_btn: search_btn.clone(),
                preview_btn: preview_btn.clone(),
                tab_bar: tab_bar.clone(),
                properties_dialog: properties_dialog.clone(),
            };
            key_controller.connect_key_pressed(move |_, key, _, modifiers| {
                let Some(name) = key.name() else {
                    return glib::Propagation::Proceed;
                };
                let mut held: Vec<&str> = Vec::new();
                if modifiers.contains(gtk::gdk::ModifierType::CONTROL_MASK) {
                    held.push("Ctrl");
                }
                if modifiers.contains(gtk::gdk::ModifierType::ALT_MASK) {
                    held.push("Alt");
                }
                if modifiers.contains(gtk::gdk::ModifierType::SHIFT_MASK) {
                    held.push("Shift");
                }
                if modifiers.contains(gtk::gdk::ModifierType::SUPER_MASK) {
                    held.push("Super");
                }
                let action = ctx
                    .state
                    .borrow()
                    .config
                    .keybindings
                    .action_for(&name, &held)
                    .map(str::to_string);
                let Some(action) = action else {
                    return glib::Propagation::Proceed;
                };
                // A bare key while typing in a text field is text, not a shortcut.
                if held.is_empty() && text_entry_has_focus(&ctx.window) {
                    return glib::Propagation::Proceed;
                }
                if dispatch_key_action(&ctx, &action) {
                    glib::Propagation::Stop
                } else {
                    glib::Propagation::Proceed
                }
            });
            window.add_controller(key_controller);
        }

        // Connect volume monitor signals for dynamic device list updates
        sidebar.connect_volume_signals();

        // "Connect to Server" opens one dialog at a time.
        let connect_dialog: Rc<RefCell<Option<Rc<ConnectDialog>>>> =
            Rc::new(RefCell::new(None));
        {
            let holder = connect_dialog.clone();
            let window_ref = window.clone();
            let cmd_tx = command_tx.clone();
            sidebar.set_connect_handler(move || {
                let existing = holder.borrow().clone();
                if let Some(dialog) = existing {
                    dialog.present();
                    return;
                }
                let dialog = ConnectDialog::new(&window_ref, cmd_tx.clone());
                {
                    let holder = holder.clone();
                    dialog.connect_closed(move || {
                        holder.borrow_mut().take();
                    });
                }
                *holder.borrow_mut() = Some(dialog.clone());
                dialog.present();
            });
        }

        // Sidebar "Recent": list recently used files in the active pane.
        {
            let views = views.clone();
            let state = state.clone();
            let tab_bar = tab_bar.clone();
            let search_bar = search_bar.clone();
            let status_label = status_label.clone();
            let panel = preview_panel.clone();
            let cmd_tx = command_tx.clone();
            // Weak: the sidebar owns this handler.
            let sidebar_weak = Rc::downgrade(&sidebar);
            sidebar.set_recent_handler(move || {
                let pane_id = views.active_pane_id();
                search_bar.clear();
                if let Some(sidebar) = sidebar_weak.upgrade() {
                    sidebar.clear_tag_filter();
                }
                // The count arrives once the files have been checked.
                let status_label = status_label.clone();
                views.show_recent(pane_id, move |count| {
                    status_label.set_text(&format!("{} recent files", count));
                });
                tab_bar.refresh();
                sync_preview(&state, &views, &panel, &cmd_tx);
            });
        }

        // Request container info on startup
        let _ = command_tx.send(AppCommand::GetContainerInfo);

        Self {
            window,
            state,
            command_tx,
            views,
            tab_bar,
            search_bar,
            preview_panel,
            operation_panel,
            status_label,
            container_banner,
            disk_usage_status,
            disk_usage_bar,
            properties_dialog,
            sidebar,
            duplicate_dialog,
            organize_dialog,
            conflict_dialogs: RefCell::new(HashMap::new()),
            connect_dialog,
            plugin_actions: RefCell::new(Vec::new()),
            preview_btn: preview_btn.clone(),
            view_buttons: [list_btn.clone(), icons_btn.clone(), previews_btn.clone()],
        }
    }

    /// Rebuild both panes' "Plugins" submenus from the last reported actions.
    fn refresh_plugin_menus(&self) {
        let actions = self.plugin_actions.borrow();
        let names = self.state.borrow().loaded_plugins.clone();
        for view in [&self.views.left, &self.views.right] {
            view.context_menu.update_plugin_actions(&actions, &names);
        }
    }

    /// Reload every shown pane's listing, after an operation changed things.
    fn refresh_active_pane(&self) {
        self.views.reload_all();
    }

    /// Take down the conflict prompt for an operation that is over. Nothing is
    /// waiting on an answer any more, so the prompt is destroyed rather than
    /// closed, which would send one.
    fn dismiss_conflict_dialog(&self, id: OperationId) {
        if let Some(dialog) = self.conflict_dialogs.borrow_mut().remove(&id) {
            dialog.destroy();
        }
    }

    /// Repaint the file list and status bar for `pane_id` from its stored entries
    /// and filters.
    ///
    /// The search filter and the tag filter are independent restrictions on the same
    /// listing, so they are resolved together here; each handler updates its own piece
    /// of pane state and then calls this, rather than painting a view of its own.
    fn refresh_pane_view(&self, pane_id: u32) {
        let Some((visible, show_hidden, mut labels, vcs)) = ({
            let s = self.state.borrow();
            s.pane_by_id(pane_id).map(|pane| {
                (
                    pane.visible_entries(),
                    s.show_hidden,
                    pane.filter_labels(),
                    pane.vcs_statuses.clone(),
                )
            })
        }) else {
            return;
        };

        let Some(view) = self.views.view_for_pane(pane_id) else {
            return;
        };
        view.file_list.set_entries(&visible, show_hidden, &vcs);
        // The status bar describes the active pane only.
        if self.views.active_pane_id() != pane_id {
            return;
        }

        let shown = if show_hidden {
            visible.len()
        } else {
            visible.iter().filter(|e| !e.is_hidden()).count()
        };
        let hidden = visible.len() - shown;
        if hidden > 0 {
            labels.insert(0, format!("{} hidden", hidden));
        }

        let status = if labels.is_empty() {
            format!("{} items", shown)
        } else {
            format!("{} items ({})", shown, labels.join(", "))
        };
        self.status_label.set_text(&status);
    }

    /// Apply a pending reveal to `pane_id` once its listing holds the paths.
    ///
    /// Safe to call at any point: it is a no-op with nothing pending, and it
    /// leaves the request pending when none of the paths are in the model yet,
    /// so the directory load still on its way gets its turn.
    fn apply_pending_selection(&self, pane_id: u32) {
        let (paths, show_properties) = {
            let state = self.state.borrow();
            match state.pane_by_id(pane_id) {
                // Recent may list the file too, but the reveal is waiting on
                // its folder, which is still loading.
                Some(pane) if !pane.pending_selection.is_empty() && !pane.recent => {
                    (pane.pending_selection.clone(), pane.pending_properties)
                }
                _ => return,
            }
        };

        let Some(view) = self.views.view_for_pane(pane_id) else {
            return;
        };
        if view.file_list.select_paths(&paths).is_none() {
            // The quick filter may be hiding the target in this very folder;
            // a reveal outranks a filter, so drop it and try again.
            if !(view.file_list.is_quick_filtered() && view.file_list.holds_any(&paths)) {
                return;
            }
            view.clear_quick_filter();
            if view.file_list.select_paths(&paths).is_none() {
                return;
            }
        }

        {
            let mut state = self.state.borrow_mut();
            if let Some(pane) = state.pane_by_id_mut(pane_id) {
                pane.pending_selection.clear();
                pane.pending_properties = false;
            }
        }

        if show_properties {
            if let Some(entry) = get_primary_selected_entry(&view.file_list) {
                let dialog = PropertiesDialog::new(&self.window, &entry, &self.command_tx);
                dialog.borrow().present();
                *self.properties_dialog.borrow_mut() = Some(dialog);
            }
        }
    }

    pub fn handle_event(&self, event: AppEvent) {
        match event {
            AppEvent::DirectoryLoaded {
                pane_id,
                path,
                entries,
            } => {
                {
                    let mut state = self.state.borrow_mut();
                    if let Some(pane) = state.pane_by_id_mut(pane_id) {
                        pane.current_path = path.clone();
                        pane.entries = entries;
                        // A new listing arrives unfiltered, and its status follows.
                        pane.clear_filters();
                        pane.vcs_statuses.clear();
                        // A folder listing always ends the Recent view.
                        pane.recent = false;
                    }
                    let tab_title = path.file_name().unwrap_or("/").to_string();
                    state.active_tab_mut().title = tab_title;
                }

                self.views.sync_layout();
                // Another folder ends the quick filter; a reload keeps it.
                self.views.sync_quick_filter(pane_id);
                if let Some(view) = self.views.view_for_pane(pane_id) {
                    view.path_bar.set_path(&path, &self.command_tx, pane_id);
                }
                self.tab_bar.refresh();
                // Pane filters were just reset; clear the widgets that display them so
                // the UI does not show a query or tag that is no longer applied.
                self.sidebar.clear_tag_filter();
                self.search_bar.clear();
                self.refresh_pane_view(pane_id);
                // The listing this reveal was waiting on may be this one.
                self.apply_pending_selection(pane_id);
                // A new folder, tab or reload: the preview may no longer apply.
                sync_preview(&self.state, &self.views, &self.preview_panel, &self.command_tx);
            }

            AppEvent::SelectItems {
                pane_id,
                paths,
                show_properties,
            } => {
                {
                    let mut state = self.state.borrow_mut();
                    if let Some(pane) = state.pane_by_id_mut(pane_id) {
                        pane.pending_selection = paths;
                        pane.pending_properties = show_properties;
                    }
                }
                // If the pane already shows the directory, the load that
                // follows is a refresh and there is nothing to wait for.
                self.apply_pending_selection(pane_id);
                // A reveal is a request to look at something, so the window has
                // to come forward -- the caller is another application.
                self.window.present();
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
                self.dismiss_conflict_dialog(id);
                self.refresh_active_pane();
            }

            AppEvent::OperationFailed { id, error } => {
                self.operation_panel.remove_operation(id);
                self.dismiss_conflict_dialog(id);
                // A cancellation is the user's doing, not a failure to report as
                // one; either way the listing shows whatever got done first.
                if error.contains("cancelled") {
                    self.status_label.set_text("Operation cancelled");
                } else {
                    self.status_label
                        .set_text(&format!("Operation failed: {}", error));
                }
                self.refresh_active_pane();
            }

            AppEvent::OperationConflict { conflict } => {
                let id = conflict.operation_id;
                // The backend asks one question per operation at a time, so an
                // earlier prompt for this operation is already stale.
                self.dismiss_conflict_dialog(id);
                let file_name = conflict.destination.file_name().unwrap_or("").to_string();
                self.operation_panel.set_waiting_on_conflict(id, &file_name);
                let dialog = show_conflict_dialog(&self.window, &conflict, &self.command_tx);
                self.conflict_dialogs.borrow_mut().insert(id, dialog);
            }

            // --- Search events ---
            AppEvent::SearchResult { entry, .. } => {
                let show_hidden = self.state.borrow().show_hidden;
                if show_hidden || !entry.is_hidden() {
                    self.views
                        .active()
                        .file_list
                        .model
                        .append(&crate::widgets::file_list::FileEntryObject::new(&entry));
                }
            }

            AppEvent::SearchCompleted { total_matches } => {
                self.status_label
                    .set_text(&format!("Search complete: {} matches", total_matches));
            }

            AppEvent::SearchError { error } => {
                self.status_label
                    .set_text(&format!("Search error: {}", error));
            }

            // --- Preview events ---
            AppEvent::PreviewReady { path, preview } => {
                self.preview_panel.set_preview(&path, &preview);
            }

            AppEvent::PreviewError { path, error } => {
                tracing::warn!("Preview failed for {}: {}", path, error);
            }

            // --- Git events ---
            AppEvent::GitStatusUpdated { path, statuses } => {
                let by_name: HashMap<String, raven_core::events::GitFileStatus> = statuses
                    .into_iter()
                    .filter_map(|e| e.path.file_name().map(|n| (n.to_string(), e.status)))
                    .collect();
                // Every pane showing this directory gets the statuses; the ones
                // on screen are repainted, the rest pick them up when shown.
                let matching: Vec<u32> = {
                    let mut s = self.state.borrow_mut();
                    let mut matching = Vec::new();
                    for tab in s.tabs.iter_mut() {
                        for pane in std::iter::once(&mut tab.pane).chain(tab.secondary_pane.iter_mut()) {
                            if pane.current_path == path {
                                pane.vcs_statuses = by_name.clone();
                                matching.push(pane.id);
                            }
                        }
                    }
                    matching
                };
                for pane_id in matching {
                    if let Some(view) = self.views.view_for_pane(pane_id) {
                        view.file_list.apply_vcs_statuses(&by_name);
                    }
                }
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
            AppEvent::PluginLoaded { plugin_id, name } => {
                tracing::info!("Plugin loaded: {}", name);
                self.state
                    .borrow_mut()
                    .loaded_plugins
                    .insert(plugin_id, name.clone());
                self.status_label
                    .set_text(&format!("Plugin loaded: {}", name));
                // Submenu group titles use plugin names.
                self.refresh_plugin_menus();
            }

            AppEvent::PluginUnloaded { plugin_id } => {
                tracing::info!("Plugin unloaded: {}", plugin_id);
                self.state.borrow_mut().loaded_plugins.remove(&plugin_id);
                self.status_label
                    .set_text(&format!("Plugin unloaded: {}", plugin_id));
            }

            AppEvent::PluginError { plugin_id, error } => {
                tracing::error!("Plugin {} error: {}", plugin_id, error);
                self.status_label
                    .set_text(&format!("Plugin error: {}", error));
            }

            AppEvent::PluginActionsChanged { actions } => {
                *self.plugin_actions.borrow_mut() = actions;
                self.refresh_plugin_menus();
            }

            // --- Network events ---
            AppEvent::RemoteConnected {
                id,
                protocol,
                host,
                initial_path,
            } => {
                let dialog = self.connect_dialog.borrow_mut().take();
                if let Some(dialog) = dialog {
                    dialog.close();
                }
                self.sidebar.add_remote(&id, &host, &initial_path);
                self.status_label
                    .set_text(&format!("Connected to {} ({})", host, protocol));

                // Browse it right away in the active pane.
                let pane_id = {
                    let mut s = self.state.borrow_mut();
                    let pane = s.active_tab_mut().active_pane_mut();
                    pane.navigate_to(initial_path.clone());
                    pane.id
                };
                let _ = self.command_tx.send(AppCommand::Navigate {
                    path: initial_path,
                    pane_id,
                });
            }

            AppEvent::RemoteDisconnected { id } => {
                self.sidebar.remove_remote(&id);
                self.status_label
                    .set_text(&format!("Disconnected from {}", id));

                // A pane still browsing that server has nowhere to go but home.
                let remaining = self.sidebar.remote_ids();
                let stranded: Vec<u32> = {
                    let s = self.state.borrow();
                    s.tabs
                        .iter()
                        .flat_map(|tab| std::iter::once(&tab.pane).chain(tab.secondary_pane.iter()))
                        .filter(|pane| pane_uses_connection(&pane.current_path, &id, &remaining))
                        .map(|pane| pane.id)
                        .collect()
                };
                for pane_id in stranded {
                    let home = RavenPath::local(
                        std::env::var_os("HOME")
                            .map(PathBuf::from)
                            .unwrap_or_else(|| PathBuf::from("/")),
                    );
                    {
                        let mut s = self.state.borrow_mut();
                        if let Some(pane) = s.pane_by_id_mut(pane_id) {
                            pane.navigate_to(home.clone());
                        }
                    }
                    let _ = self.command_tx.send(AppCommand::Navigate { path: home, pane_id });
                }
            }

            AppEvent::RemoteError { id, error } => {
                tracing::error!("Remote {} error: {}", id, error);
                let dialog = self.connect_dialog.borrow().clone();
                match dialog {
                    Some(dialog) => dialog.show_error(&error),
                    None => self
                        .status_label
                        .set_text(&format!("Connection error: {}", error)),
                }
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
                    self.status_label
                        .set_text(&format!("{}: {} in {} items", path, size_str, total_items));
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
                    let state = unit.active_state.as_deref().unwrap_or("unknown");
                    let desc = unit.description.as_deref().unwrap_or(&unit.name);
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

            // --- AI events ---
            AppEvent::DuplicateScanProgress { progress } => {
                if let Some(ref dialog) = *self.duplicate_dialog.borrow() {
                    dialog.update_progress(&progress);
                }
            }

            AppEvent::DuplicateScanCompleted { groups } => {
                if let Some(ref dialog) = *self.duplicate_dialog.borrow() {
                    dialog.set_results(&groups);
                }
            }

            AppEvent::DuplicateScanError { error } => {
                if let Some(ref dialog) = *self.duplicate_dialog.borrow() {
                    dialog.set_error(&error);
                }
                self.status_label
                    .set_text(&format!("Duplicate scan error: {}", error));
            }

            AppEvent::TagCountsUpdated { pane_id, counts } => {
                let active_pane_id = self.state.borrow().active_tab().active_pane().id;
                if pane_id == active_pane_id {
                    self.sidebar.update_tag_counts(&counts);
                }
            }

            AppEvent::TagFilterApplied {
                pane_id,
                path,
                tag,
                paths,
            } => {
                // A slow listing can land after the pane navigated away; those paths
                // describe a directory that is no longer on screen.
                let applied = {
                    let mut s = self.state.borrow_mut();
                    match s.pane_by_id_mut(pane_id) {
                        Some(pane) if pane.current_path == path => {
                            pane.tag_filter = tag.map(|tag| TagFilter {
                                tag,
                                paths: paths.into_iter().collect(),
                            });
                            true
                        }
                        _ => false,
                    }
                };

                if applied {
                    self.refresh_pane_view(pane_id);
                }
            }

            AppEvent::OrganizationAnalysisComplete {
                path: _,
                suggestions,
            } => {
                if let Some(ref dialog) = *self.organize_dialog.borrow() {
                    dialog.set_suggestions(suggestions);
                }
            }

            // --- Filter ---
            AppEvent::FilterApplied { filter, pane_id } => {
                {
                    let mut s = self.state.borrow_mut();
                    if let Some(pane) = s.pane_by_id_mut(pane_id) {
                        pane.filter = filter;
                    }
                }
                self.refresh_pane_view(pane_id);
            }

            // --- Directory size updates ---
            AppEvent::DirSizeCalculated {
                pane_id,
                path,
                size,
            } => {
                // Only update if this is still the active pane
                if let Some(view) = self.views.view_for_pane(pane_id) {
                    view.file_list.update_dir_size(&path, size);
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

fn get_selected_entries(file_list: &FileListView) -> Vec<FileEntry> {
    let mut entries = Vec::new();
    let item_count = file_list.selection.n_items();
    for pos in 0..item_count {
        if !file_list.selection.is_selected(pos) {
            continue;
        }
        if let Some(entry) = file_list
            .selection
            .item(pos)
            .and_then(|item| {
                item.downcast::<crate::widgets::file_list::FileEntryObject>()
                    .ok()
            })
            .and_then(|obj| obj.entry())
        {
            entries.push(entry);
        }
    }
    entries
}

/// Log a failure to open a file and show it in the status bar, when the
/// window has one yet.
fn report_open_error(status: &Rc<RefCell<Option<gtk::Label>>>, message: &str) {
    tracing::warn!("{}", message);
    if let Some(label) = status.borrow().as_ref() {
        label.set_text(message);
    }
}

fn get_primary_selected_entry(file_list: &FileListView) -> Option<FileEntry> {
    get_selected_entries(file_list).into_iter().next()
}

/// The selection as published to plugins and D-Bus clients.
fn local_path_strings(entries: &[FileEntry]) -> Vec<String> {
    raven_plugin::selection::local_path_strings(entries.iter().map(|e| &e.path))
}

fn get_selected_paths(file_list: &FileListView) -> Vec<RavenPath> {
    get_selected_entries(file_list)
        .into_iter()
        .map(|entry| entry.path)
        .collect()
}

fn get_selected_local_paths(file_list: &FileListView) -> Vec<PathBuf> {
    get_selected_entries(file_list)
        .into_iter()
        .filter_map(|entry| entry.path.as_local_path().map(|path| path.to_path_buf()))
        .collect()
}

/// Returns true when the given path is inside the user's Trash/files directory.
/// Used to switch Delete-key and context-menu "Delete" from trash → permanent delete.
fn is_in_trash(path: &RavenPath) -> bool {
    if let Some(local) = path.as_local_path() {
        let home = std::env::var("HOME").unwrap_or_else(|_| "/root".to_string());
        let trash_dir = PathBuf::from(home).join(".local/share/Trash/files");
        local.starts_with(&trash_dir)
    } else {
        false
    }
}

fn load_tag_menu_data(config: &AppConfig, entry: &FileEntry) -> (Vec<String>, Vec<String>) {
    let engine = raven_ai::tag_engine::TagEngine::new(tags_db_path(config));
    let mut all_tags = engine.tag_names();
    let current_tags = engine.tags_for_entry(entry);
    for tag in &current_tags {
        if !all_tags.contains(tag) {
            all_tags.push(tag.clone());
        }
    }
    all_tags.sort();
    all_tags.dedup();
    (all_tags, current_tags)
}

fn tags_db_path(config: &AppConfig) -> PathBuf {
    let tags_dir = config
        .automation
        .rules_dir
        .as_ref()
        .map(|dir| {
            PathBuf::from(dir)
                .parent()
                .unwrap_or(Path::new("."))
                .to_path_buf()
        })
        .unwrap_or_else(|| {
            std::env::var_os("XDG_CONFIG_HOME")
                .map(PathBuf::from)
                .or_else(|| {
                    std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".config"))
                })
                .unwrap_or_else(|| PathBuf::from("."))
                .join("raven")
        });
    tags_dir.join("tags.toml")
}

/// Show a rename dialog for the given entry.
fn show_rename_dialog(
    parent: &adw::ApplicationWindow,
    entry: &raven_core::entry::FileEntry,
    command_tx: &tokio::sync::mpsc::UnboundedSender<AppCommand>,
) {
    let path = entry.path.clone();
    let cmd_tx = command_tx.clone();
    show_name_dialog(
        parent,
        NameDialog {
            title: "Rename",
            prompt: "Enter new name:",
            initial: &entry.name,
            select_stem: !entry.is_dir(),
            confirm_label: "Rename",
        },
        move |new_name| {
            let _ = cmd_tx.send(AppCommand::RenameFile {
                path: path.clone(),
                new_name,
            });
        },
    );
}

#[derive(Clone, Copy)]
enum NewItem {
    Folder,
    File,
}

/// Ask for a name and create a folder or an empty file in the pane's directory.
fn show_new_item_dialog(
    parent: &adw::ApplicationWindow,
    state: &AppState,
    command_tx: &tokio::sync::mpsc::UnboundedSender<AppCommand>,
    pane_id: u32,
    kind: NewItem,
) {
    let Some(parent_path) = state
        .borrow()
        .pane_by_id(pane_id)
        .map(|pane| pane.current_path.clone())
    else {
        return;
    };

    let (title, prompt, initial) = match kind {
        NewItem::Folder => ("New Folder", "Folder name:", "Untitled Folder"),
        NewItem::File => ("New File", "File name:", "Untitled.txt"),
    };

    let state = state.clone();
    let cmd_tx = command_tx.clone();
    show_name_dialog(
        parent,
        NameDialog {
            title,
            prompt,
            initial,
            select_stem: matches!(kind, NewItem::File),
            confirm_label: "Create",
        },
        move |name| {
            // The listing reloads when the operation completes; the new item is
            // selected then, so the user can see where it landed.
            {
                let mut s = state.borrow_mut();
                if let Some(pane) = s.pane_by_id_mut(pane_id) {
                    pane.pending_selection = vec![parent_path.join(&name)];
                    pane.pending_properties = false;
                }
            }
            let parent = parent_path.clone();
            let _ = cmd_tx.send(match kind {
                NewItem::Folder => AppCommand::CreateDirectory { parent, name },
                NewItem::File => AppCommand::CreateFile { parent, name },
            });
        },
    );
}

/// What a name dialog says and offers.
struct NameDialog<'a> {
    title: &'a str,
    prompt: &'a str,
    initial: &'a str,
    /// Pre-select the name up to its extension, so typing replaces only that.
    select_stem: bool,
    confirm_label: &'a str,
}

/// Why a file name cannot be used, if it cannot.
fn invalid_name_reason(name: &str) -> Option<&'static str> {
    if name.is_empty() {
        Some("A name is required")
    } else if name.contains('/') {
        Some("Names cannot contain \"/\"")
    } else if name == "." || name == ".." {
        Some("That name is reserved")
    } else {
        None
    }
}

/// A modal that asks for one name. `on_confirm` runs with the trimmed name
/// once it is valid; the dialog shows why otherwise and stays open.
fn show_name_dialog(
    parent: &adw::ApplicationWindow,
    spec: NameDialog<'_>,
    on_confirm: impl Fn(String) + 'static,
) {
    let dialog = adw::Window::builder()
        .title(spec.title)
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

    let label = gtk::Label::new(Some(spec.prompt));
    label.set_halign(gtk::Align::Start);
    content.append(&label);

    let name_entry = gtk::Entry::new();
    name_entry.set_text(spec.initial);
    if spec.select_stem {
        match spec.initial.rfind('.') {
            Some(dot_pos) if dot_pos > 0 => name_entry.select_region(0, dot_pos as i32),
            _ => name_entry.select_region(0, -1),
        }
    } else {
        name_entry.select_region(0, -1);
    }
    name_entry.set_activates_default(true);
    content.append(&name_entry);

    let error_label = gtk::Label::new(None);
    error_label.set_halign(gtk::Align::Start);
    error_label.add_css_class("error");
    error_label.add_css_class("caption");
    error_label.set_visible(false);
    content.append(&error_label);

    let submit: Rc<dyn Fn()> = {
        let dialog = dialog.clone();
        let name_entry = name_entry.clone();
        let error_label = error_label.clone();
        Rc::new(move || {
            let name = name_entry.text().trim().to_string();
            if let Some(reason) = invalid_name_reason(&name) {
                error_label.set_text(reason);
                error_label.set_visible(true);
                name_entry.add_css_class("error");
                return;
            }
            on_confirm(name);
            dialog.close();
        })
    };
    {
        let error_label = error_label.clone();
        name_entry.connect_changed(move |entry| {
            error_label.set_visible(false);
            entry.remove_css_class("error");
        });
    }

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

    let confirm_btn = gtk::Button::with_label(spec.confirm_label);
    confirm_btn.add_css_class("suggested-action");
    {
        let submit = submit.clone();
        confirm_btn.connect_clicked(move |_| submit());
    }
    btn_box.append(&confirm_btn);

    content.append(&btn_box);

    // Enter submits, Escape closes
    {
        let dialog = dialog.clone();
        let submit = submit.clone();
        let key_controller = gtk::EventControllerKey::new();
        key_controller.connect_key_pressed(move |_, key, _, _| {
            match key {
                gtk::gdk::Key::Escape => {
                    dialog.close();
                    return glib::Propagation::Stop;
                }
                gtk::gdk::Key::Return | gtk::gdk::Key::KP_Enter => {
                    submit();
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

/// The listing of whichever pane is active, looked up when an action runs.
#[derive(Clone)]
struct ActiveList {
    views: Rc<PaneViews>,
}

impl ActiveList {
    fn get(&self) -> Rc<FileListView> {
        self.views.active().file_list.clone()
    }
}

fn active_pane_id(state: &AppState) -> u32 {
    state.borrow().active_tab().active_pane().id
}

/// Whether the focused widget is a text field, where plain keys are typing.
fn text_entry_has_focus(window: &adw::ApplicationWindow) -> bool {
    match gtk::prelude::GtkWindowExt::focus(window) {
        Some(widget) => {
            widget.is::<gtk::Text>()
                || widget.is::<gtk::Entry>()
                || widget.is::<gtk::TextView>()
                || widget.is::<gtk::SearchEntry>()
        }
        None => false,
    }
}

/// What a shortcut needs to reach.
struct KeyContext {
    window: adw::ApplicationWindow,
    state: AppState,
    cmd_tx: tokio::sync::mpsc::UnboundedSender<AppCommand>,
    views: Rc<PaneViews>,
    hidden_btn: gtk::ToggleButton,
    search_btn: gtk::ToggleButton,
    preview_btn: gtk::ToggleButton,
    tab_bar: Rc<TabBar>,
    properties_dialog: Rc<RefCell<Option<Rc<RefCell<PropertiesDialog>>>>>,
}

/// Show the active pane's selected file in the preview panel, or clear the
/// panel when nothing is selected. A file already shown or on its way is not
/// asked for again: a click reaches here both through sync_preview and the
/// debounced selection handler, and each request is a full provider run.
fn preview_selection(
    views: &PaneViews,
    panel: &Rc<PreviewPanel>,
    cmd_tx: &tokio::sync::mpsc::UnboundedSender<AppCommand>,
) {
    match get_primary_selected_entry(&views.active().file_list) {
        Some(entry) if panel.is_showing(&entry.path) => {}
        Some(entry) => {
            panel.request(&entry.path);
            let _ = cmd_tx.send(AppCommand::GeneratePreview { path: entry.path });
        }
        None => panel.clear(),
    }
}

/// Bring an open preview panel in line with the active pane after something
/// other than a selection change: the pane navigated or reloaded, or another
/// pane or tab became active. The active pane's selection is previewed when
/// there is one. Otherwise a preview of a file that is gone, or that is not in
/// the active pane's folder, is cleared -- a reload that merely dropped the
/// selection leaves a still-valid preview alone.
fn sync_preview(
    state: &AppState,
    views: &PaneViews,
    panel: &Rc<PreviewPanel>,
    cmd_tx: &tokio::sync::mpsc::UnboundedSender<AppCommand>,
) {
    if !panel.widget.is_visible() {
        return;
    }
    if let Some(entry) = get_primary_selected_entry(&views.active().file_list) {
        if !panel.is_showing(&entry.path) {
            panel.request(&entry.path);
            let _ = cmd_tx.send(AppCommand::GeneratePreview { path: entry.path });
        }
        return;
    }
    let Some(shown) = panel.shown_path() else {
        return;
    };
    let (folder, recent) = {
        let s = state.borrow();
        let pane = s.active_tab().active_pane();
        (pane.current_path.clone(), pane.recent)
    };
    // Recent lists files from anywhere, so any of them may be on show.
    let still_valid = (recent || crate::widgets::preview_panel::belongs_to_folder(&shown, &folder))
        && shown.as_local_path().is_some_and(|p| p.exists());
    if !still_valid {
        panel.clear();
    }
}

/// Put the divider so the preview panel is `panel_width` wide. The Paned has
/// no width before its first layout (at startup), so then this waits for one.
fn place_preview_divider(paned: &gtk::Paned, panel_width: i32) {
    let place = move |paned: &gtk::Paned| {
        let total = paned.width();
        if total <= 0 {
            return false;
        }
        paned.set_position((total - panel_width).max(0));
        true
    };
    if !place(paned) {
        paned.add_tick_callback(move |paned, _| {
            if place(paned) {
                glib::ControlFlow::Break
            } else {
                glib::ControlFlow::Continue
            }
        });
    }
}

/// Run the named action. Returns `false` for an action this window does not
/// know, so the key press goes on to whatever else wants it.
fn dispatch_key_action(ctx: &KeyContext, action: &str) -> bool {
    let pane_id = active_pane_id(&ctx.state);
    let file_list = ctx.views.active().file_list.clone();
    match action {
        "navigate_back" => navigate_back(&ctx.state, &ctx.cmd_tx, pane_id),
        "navigate_forward" => navigate_forward(&ctx.state, &ctx.cmd_tx, pane_id),
        "navigate_up" => navigate_up(&ctx.state, &ctx.cmd_tx, pane_id),
        "navigate_home" => {
            let home = RavenPath::local(
                std::env::var_os("HOME")
                    .map(PathBuf::from)
                    .unwrap_or_else(|| PathBuf::from("/")),
            );
            {
                let mut s = ctx.state.borrow_mut();
                if let Some(pane) = s.pane_by_id_mut(pane_id) {
                    pane.navigate_to(home.clone());
                }
            }
            let _ = ctx.cmd_tx.send(AppCommand::Navigate { path: home, pane_id });
        }
        // Reloads the folder, or the recent list while Recent is shown.
        "refresh" => ctx.views.reload(pane_id),
        "edit_path" => ctx.views.active().path_bar.toggle_edit_mode(),
        "toggle_hidden" => ctx.hidden_btn.set_active(!ctx.hidden_btn.is_active()),
        "toggle_search" => ctx.search_btn.set_active(!ctx.search_btn.is_active()),
        "toggle_preview" => ctx.preview_btn.set_active(!ctx.preview_btn.is_active()),
        "new_tab" => {
            let path = ctx
                .state
                .borrow()
                .active_tab()
                .active_pane()
                .current_path
                .clone();
            ctx.state.borrow_mut().add_tab(path);
            ctx.tab_bar.refresh();
            ctx.views.show_active_tab();
        }
        "close_tab" => {
            let closed = {
                let mut s = ctx.state.borrow_mut();
                let idx = s.active_tab;
                s.close_tab(idx)
            };
            if closed {
                ctx.tab_bar.refresh();
                ctx.views.show_active_tab();
            }
        }
        "next_tab" | "prev_tab" => {
            let delta = if action == "next_tab" { 1 } else { -1 };
            ctx.state.borrow_mut().cycle_tab(delta);
            ctx.tab_bar.refresh();
            ctx.views.show_active_tab();
        }
        "toggle_dual_pane" => ctx.views.toggle_dual(),
        "switch_pane" => ctx.views.switch_pane(),
        "copy" | "cut" => {
            let paths = get_selected_paths(&file_list);
            if !paths.is_empty() {
                let mut s = ctx.state.borrow_mut();
                s.clipboard = Some(if action == "copy" {
                    ClipboardOp::Copy(paths)
                } else {
                    ClipboardOp::Cut(paths)
                });
            }
        }
        "paste" => {
            let (clipboard, dest) = {
                let s = ctx.state.borrow();
                let dest = s.active_tab().active_pane().current_path.clone();
                (s.clipboard.clone(), dest)
            };
            match clipboard {
                Some(ClipboardOp::Copy(sources)) => {
                    // Copy: keep clipboard so user can paste multiple times
                    let _ = ctx.cmd_tx.send(AppCommand::CopyFiles {
                        sources,
                        destination: dest,
                    });
                }
                Some(ClipboardOp::Cut(sources)) => {
                    // Cut: clear clipboard after paste (each file moves only once)
                    ctx.state.borrow_mut().clipboard = None;
                    let _ = ctx.cmd_tx.send(AppCommand::MoveFiles {
                        sources,
                        destination: dest,
                    });
                }
                None => {}
            }
        }
        "trash" => {
            let paths = get_selected_paths(&file_list);
            if !paths.is_empty() {
                let in_trash = {
                    let s = ctx.state.borrow();
                    is_in_trash(&s.active_tab().active_pane().current_path)
                };
                if in_trash {
                    let _ = ctx.cmd_tx.send(AppCommand::DeleteFiles { paths });
                } else {
                    let _ = ctx.cmd_tx.send(AppCommand::TrashFiles { paths });
                }
            }
        }
        "delete_permanently" => {
            let paths = get_selected_paths(&file_list);
            if !paths.is_empty() {
                let _ = ctx.cmd_tx.send(AppCommand::DeleteFiles { paths });
            }
        }
        "rename" => {
            if let Some(entry) = get_primary_selected_entry(&file_list) {
                show_rename_dialog(&ctx.window, &entry, &ctx.cmd_tx);
            }
        }
        "new_folder" => {
            show_new_item_dialog(&ctx.window, &ctx.state, &ctx.cmd_tx, pane_id, NewItem::Folder)
        }
        "new_file" => {
            show_new_item_dialog(&ctx.window, &ctx.state, &ctx.cmd_tx, pane_id, NewItem::File)
        }
        "select_all" => {
            file_list.selection.select_all();
        }
        "undo" => {
            let _ = ctx.cmd_tx.send(AppCommand::Undo);
        }
        "properties" => {
            if let Some(entry) = get_primary_selected_entry(&file_list) {
                let dialog = PropertiesDialog::new(&ctx.window, &entry, &ctx.cmd_tx);
                dialog.borrow().present();
                *ctx.properties_dialog.borrow_mut() = Some(dialog);
            }
        }
        "open_settings" => {
            let dialog = SettingsDialog::new(&ctx.window, ctx.state.clone(), ctx.cmd_tx.clone());
            dialog.present();
        }
        _ => return false,
    }
    true
}

/// Launch a custom action detached from the file manager. Failures to start
/// are logged; the command's own output is not collected.
fn run_custom_action(action: &CustomAction, target: &ActionTarget) {
    if action.command.trim().is_empty() {
        tracing::warn!("Custom action \"{}\" has no command", action.name);
        return;
    }
    let args = action.expand_args(target);
    tracing::info!(
        "Running action \"{}\": {} {:?} in {}",
        action.name,
        action.command,
        args,
        target.directory.display()
    );
    let result = std::process::Command::new(&action.command)
        .args(&args)
        .current_dir(&target.directory)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
    if let Err(e) = result {
        tracing::error!("Custom action \"{}\" failed to start: {}", action.name, e);
    }
}

/// The connection a remote path belongs to, in the backend's key format.
fn sftp_connection_key(path: &RavenPath) -> Option<String> {
    match path {
        RavenPath::Sftp {
            host, port, user, ..
        } => Some(format!("{}@{}:{}", user, host, port)),
        _ => None,
    }
}

/// Whether `path` is browsed through the connection `id`. SMB ids name a
/// share (`smb://host/share`) or a whole server (`smb://host/`), which
/// covers every share on it.
///
/// `remaining` are the connection ids still open. The backend logs a server
/// out once none of its ids remain, which strands every pane on that server,
/// including one on a share reached from the share list without its own id.
fn pane_uses_connection(path: &RavenPath, id: &str, remaining: &[String]) -> bool {
    if let RavenPath::Smb { host, share, .. } = path {
        let server = format!("smb://{}/", host.to_ascii_lowercase());
        if id == server || id == format!("{}{}", server, share) {
            return true;
        }
        return id.starts_with(&server) && !remaining.iter().any(|r| r.starts_with(&server));
    }
    sftp_connection_key(path).as_deref() == Some(id)
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
    // Up from Recent is the folder behind it, not that folder's parent.
    let folder = state
        .borrow_mut()
        .pane_by_id_mut(pane_id)
        .and_then(|pane| pane.leave_recent());
    if let Some(path) = folder {
        let _ = command_tx.send(AppCommand::Navigate { path, pane_id });
        return;
    }
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

#[cfg(test)]
mod pane_connection_tests {
    use super::*;

    fn smb(host: &str, share: &str) -> RavenPath {
        RavenPath::Smb {
            host: host.into(),
            share: share.into(),
            path: "/".into(),
        }
    }

    #[test]
    fn smb_panes_are_stranded_when_their_server_logs_out() {
        let docs = smb("NAS", "Docs");
        // The disconnected id itself, or the server's share list.
        assert!(pane_uses_connection(&docs, "smb://nas/Docs", &["smb://nas/Media".into()]));
        assert!(pane_uses_connection(&docs, "smb://nas/", &["smb://nas/Media".into()]));
        // Another share on a server that stays logged in keeps browsing.
        assert!(!pane_uses_connection(&docs, "smb://nas/Media", &["smb://nas/Other".into()]));
        // The last location on the server is gone, so the backend logged out.
        assert!(pane_uses_connection(&docs, "smb://nas/Media", &[]));
        assert!(!pane_uses_connection(&docs, "smb://nas2/Media", &[]));
    }
}
