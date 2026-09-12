use std::path::PathBuf;
use std::rc::Rc;

use gtk4 as gtk;
use libadwaita as adw;
use libadwaita::prelude::*;

use raven_core::commands::AppCommand;
use raven_core::path::RavenPath;
use raven_plugin::manager::PluginManager;
use raven_plugin::manifest::{PluginManifest, PluginRuntime};

use crate::state::AppState;

/// The "Plugins" settings page: what was found, what is loaded, and switches
/// to load or unload each one.
pub fn build_plugins_page(
    state: AppState,
    command_tx: tokio::sync::mpsc::UnboundedSender<AppCommand>,
) -> gtk::ScrolledWindow {
    let scroll = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vscrollbar_policy(gtk::PolicyType::Automatic)
        .build();

    let content = gtk::Box::new(gtk::Orientation::Vertical, 16);
    content.set_margin_start(24);
    content.set_margin_end(24);
    content.set_margin_top(16);
    content.set_margin_bottom(16);

    let dirs = plugin_dirs(&state);

    let general = adw::PreferencesGroup::new();
    general.set_title("Plugins");
    general.set_description(Some(&format!(
        "Each plugin is a folder with a plugin.toml manifest and its entry point. Folders searched: {}",
        dirs.iter()
            .map(|d| d.display().to_string())
            .collect::<Vec<_>>()
            .join(", ")
    )));

    let enabled_row = adw::SwitchRow::new();
    enabled_row.set_title("Load plugins at startup");
    enabled_row.set_active(state.borrow().config.plugins.enabled);
    {
        let state = state.clone();
        enabled_row.connect_active_notify(move |row| {
            let mut s = state.borrow_mut();
            s.config.plugins.enabled = row.is_active();
            let _ = s.config.save();
        });
    }
    general.add(&enabled_row);
    content.append(&general);

    let list_group = adw::PreferencesGroup::new();
    list_group.set_title("Installed Plugins");
    let list = Rc::new(gtk::ListBox::new());
    list.add_css_class("boxed-list");
    list.set_selection_mode(gtk::SelectionMode::None);
    list_group.add(list.as_ref());

    let render: Rc<dyn Fn()> = {
        let list = list.clone();
        let state = state.clone();
        let command_tx = command_tx.clone();
        let dirs = dirs.clone();
        Rc::new(move || {
            while let Some(child) = list.first_child() {
                list.remove(&child);
            }
            let discovered = discover(&dirs);
            if discovered.is_empty() {
                let row = adw::ActionRow::new();
                row.set_title("No plugins found");
                row.set_subtitle("Put a plugin folder in one of the folders above and rescan.");
                list.append(&row);
            }
            for (dir, manifest) in discovered {
                list.append(&build_plugin_row(&dir, &manifest, &state, &command_tx));
            }
        })
    };
    render();

    let buttons = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    let rescan_btn = gtk::Button::with_label("Rescan");
    {
        let render = render.clone();
        rescan_btn.connect_clicked(move |_| render());
    }
    buttons.append(&rescan_btn);

    let open_btn = gtk::Button::with_label("Open Plugins Folder");
    {
        let state = state.clone();
        let command_tx = command_tx.clone();
        let user_dir = dirs.first().cloned();
        open_btn.connect_clicked(move |_| {
            let Some(dir) = &user_dir else { return };
            if let Err(e) = std::fs::create_dir_all(dir) {
                tracing::warn!("Could not create {}: {}", dir.display(), e);
                return;
            }
            let pane_id = state.borrow().active_tab().active_pane().id;
            let _ = command_tx.send(AppCommand::Navigate {
                path: RavenPath::local(dir.clone()),
                pane_id,
            });
        });
    }
    buttons.append(&open_btn);

    content.append(&list_group);
    content.append(&buttons);
    scroll.set_child(Some(&content));
    scroll
}

/// The user's plugin folder first, then the configured and system ones.
fn plugin_dirs(state: &AppState) -> Vec<PathBuf> {
    let mut dirs = PluginManager::default_plugin_dirs();
    for dir in &state.borrow().config.plugins.plugin_dirs {
        let dir = PathBuf::from(dir);
        if !dirs.contains(&dir) {
            dirs.push(dir);
        }
    }
    dirs
}

fn discover(dirs: &[PathBuf]) -> Vec<(PathBuf, PluginManifest)> {
    // Discovery only reads manifests; the manager's event channel is unused.
    let (tx, _rx) = tokio::sync::mpsc::unbounded_channel();
    let mut manager = PluginManager::new(tx);
    for dir in dirs {
        manager.add_plugin_dir(dir.clone());
    }
    let mut found = manager.discover();
    found.sort_by(|a, b| a.1.name.to_lowercase().cmp(&b.1.name.to_lowercase()));
    found
}

fn runtime_label(runtime: PluginRuntime) -> &'static str {
    match runtime {
        PluginRuntime::Command => "command",
        PluginRuntime::Lua => "Lua (no runtime available)",
        PluginRuntime::Wasm => "WASM (no runtime available)",
    }
}

fn build_plugin_row(
    dir: &std::path::Path,
    manifest: &PluginManifest,
    state: &AppState,
    command_tx: &tokio::sync::mpsc::UnboundedSender<AppCommand>,
) -> adw::ActionRow {
    let row = adw::ActionRow::new();
    row.set_title(&format!("{} {}", manifest.name, manifest.version));
    let mut subtitle = manifest.description.clone();
    if !manifest.author.is_empty() {
        subtitle.push_str(&format!(" \u{2014} {}", manifest.author));
    }
    subtitle.push_str(&format!(
        "\n{} \u{00b7} {} \u{00b7} {}",
        manifest.id,
        runtime_label(manifest.runtime),
        dir.display()
    ));
    row.set_subtitle(&subtitle);
    row.set_subtitle_lines(3);

    let loaded = state.borrow().loaded_plugins.contains_key(&manifest.id);
    let status = gtk::Label::new(Some(if loaded { "Loaded" } else { "Not loaded" }));
    status.add_css_class("dim-label");
    status.set_valign(gtk::Align::Center);
    row.add_suffix(&status);

    let switch = gtk::Switch::new();
    switch.set_valign(gtk::Align::Center);
    switch.set_active(loaded);
    switch.set_sensitive(manifest.runtime == PluginRuntime::Command);
    if manifest.runtime != PluginRuntime::Command {
        switch.set_tooltip_text(Some("This plugin's runtime is not available"));
    }
    {
        let state = state.clone();
        let command_tx = command_tx.clone();
        let id = manifest.id.clone();
        let dir = dir.to_path_buf();
        let status = status.clone();
        switch.connect_active_notify(move |sw| {
            let on = sw.is_active();
            {
                let mut s = state.borrow_mut();
                s.config.plugins.set_plugin_enabled(&id, on);
                let _ = s.config.save();
            }
            if on {
                status.set_text("Loading...");
                let _ = command_tx.send(AppCommand::LoadPlugin { path: dir.clone() });
            } else {
                status.set_text("Not loaded");
                let _ = command_tx.send(AppCommand::UnloadPlugin {
                    plugin_id: id.clone(),
                });
            }
        });
    }
    row.add_suffix(&switch);
    row.set_activatable_widget(Some(&switch));
    row
}
