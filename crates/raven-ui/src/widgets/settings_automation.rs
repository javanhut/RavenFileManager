use std::path::PathBuf;
use std::rc::Rc;

use gtk4 as gtk;
use libadwaita as adw;
use libadwaita::prelude::*;

use raven_automation::config as rules_config;
use raven_core::automation_types::{Action, AutomationRule, Trigger};
use raven_core::commands::AppCommand;

use crate::state::AppState;
use crate::widgets::automation_rule_dialog::show_rule_dialog;

/// The "Automation" settings page: the engine switch and the rule list.
pub fn build_automation_page(
    parent: &adw::Window,
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

    let rules_dir = rules_dir(&state);

    let general = adw::PreferencesGroup::new();
    general.set_title("Automation");
    general.set_description(Some(&format!(
        "Rules watch folders and act on files that match. Each rule is a TOML file in {}.",
        rules_dir.display()
    )));
    let enabled_row = adw::SwitchRow::new();
    enabled_row.set_title("Run automation rules");
    enabled_row.set_active(state.borrow().config.automation.enabled);
    {
        let state = state.clone();
        let command_tx = command_tx.clone();
        enabled_row.connect_active_notify(move |row| {
            let on = row.is_active();
            {
                let mut s = state.borrow_mut();
                s.config.automation.enabled = on;
                let _ = s.config.save();
            }
            let _ = command_tx.send(if on {
                AppCommand::StartAutomation
            } else {
                AppCommand::StopAutomation
            });
        });
    }
    general.add(&enabled_row);
    content.append(&general);

    let rules_group = adw::PreferencesGroup::new();
    rules_group.set_title("Rules");
    let list = Rc::new(gtk::ListBox::new());
    list.add_css_class("boxed-list");
    list.set_selection_mode(gtk::SelectionMode::None);
    rules_group.add(list.as_ref());

    // Rendering re-reads the rule files, which are the source of truth the
    // backend also loads from.
    let render: Rc<dyn Fn()> = {
        let list = list.clone();
        let parent = parent.clone();
        let command_tx = command_tx.clone();
        let rules_dir = rules_dir.clone();
        let render_cell: Rc<std::cell::RefCell<Option<Rc<dyn Fn()>>>> =
            Rc::new(std::cell::RefCell::new(None));
        let cell_for_closure = render_cell.clone();
        let render: Rc<dyn Fn()> = Rc::new(move || {
            while let Some(child) = list.first_child() {
                list.remove(&child);
            }
            let mut rules = if rules_dir.is_dir() {
                rules_config::load_rules(&rules_dir)
            } else {
                Vec::new()
            };
            rules.sort_by(|a, b| a.name.to_lowercase().cmp(&b.name.to_lowercase()));
            if rules.is_empty() {
                let row = adw::ActionRow::new();
                row.set_title("No rules yet");
                row.set_subtitle("Add a rule to move, rename, or act on files automatically.");
                list.append(&row);
            }
            for rule in rules {
                let rerender = cell_for_closure.borrow().clone();
                list.append(&build_rule_row(&rule, &parent, &rules_dir, &command_tx, rerender));
            }
        });
        *render_cell.borrow_mut() = Some(render.clone());
        render
    };
    render();

    let add_btn = gtk::Button::with_label("Add Rule");
    add_btn.add_css_class("suggested-action");
    add_btn.set_halign(gtk::Align::Start);
    {
        let parent = parent.clone();
        let command_tx = command_tx.clone();
        let rules_dir = rules_dir.clone();
        let render = render.clone();
        add_btn.connect_clicked(move |_| {
            let command_tx = command_tx.clone();
            let rules_dir = rules_dir.clone();
            let render = render.clone();
            show_rule_dialog(&parent, None, move |rule| {
                store_rule(&rules_dir, &rule, &command_tx);
                render();
            });
        });
    }

    content.append(&rules_group);
    content.append(&add_btn);
    scroll.set_child(Some(&content));
    scroll
}

fn rules_dir(state: &AppState) -> PathBuf {
    state
        .borrow()
        .config
        .automation
        .rules_dir
        .as_ref()
        .map(PathBuf::from)
        .unwrap_or_else(rules_config::default_rules_dir)
}

/// Write the rule file and hand the rule to the engine, which replaces any
/// rule with the same id.
fn store_rule(
    rules_dir: &std::path::Path,
    rule: &AutomationRule,
    command_tx: &tokio::sync::mpsc::UnboundedSender<AppCommand>,
) {
    if let Err(e) = rules_config::save_rule(rules_dir, rule) {
        tracing::warn!("Failed to save rule {}: {}", rule.id, e);
    }
    let _ = command_tx.send(AppCommand::AddAutomationRule { rule: rule.clone() });
}

fn describe_trigger(trigger: &Trigger) -> String {
    match trigger {
        Trigger::FileCreated => "when a file is created".to_string(),
        Trigger::FileModified => "when a file is modified".to_string(),
        Trigger::FileDeleted => "when a file is deleted".to_string(),
        Trigger::Schedule { cron } => format!("on schedule {}", cron),
        Trigger::Manual => "manually".to_string(),
    }
}

fn describe_action(action: &Action) -> String {
    match action {
        Action::MoveTo { destination } => format!("move to {}", destination.display()),
        Action::CopyTo { destination } => format!("copy to {}", destination.display()),
        Action::Rename { pattern } => format!("rename to {}", pattern),
        Action::Delete => "delete".to_string(),
        Action::Trash => "trash".to_string(),
        Action::RunCommand { command, .. } => format!("run {}", command),
        Action::Notify { .. } => "notify".to_string(),
    }
}

fn build_rule_row(
    rule: &AutomationRule,
    parent: &adw::Window,
    rules_dir: &std::path::Path,
    command_tx: &tokio::sync::mpsc::UnboundedSender<AppCommand>,
    rerender: Option<Rc<dyn Fn()>>,
) -> adw::ActionRow {
    let row = adw::ActionRow::new();
    row.set_title(&rule.name);
    let paths = rule
        .watch_paths
        .iter()
        .map(|p| p.display().to_string())
        .collect::<Vec<_>>()
        .join(", ");
    let actions = rule
        .actions
        .iter()
        .map(describe_action)
        .collect::<Vec<_>>()
        .join(", ");
    let conditions = match rule.conditions.len() {
        0 => "any file".to_string(),
        1 => "1 condition".to_string(),
        n => format!("{} conditions", n),
    };
    row.set_subtitle(&format!(
        "{} in {}\n{} \u{2192} {}",
        describe_trigger(&rule.trigger),
        paths,
        conditions,
        actions
    ));
    row.set_subtitle_lines(3);

    let run_btn = gtk::Button::from_icon_name("media-playback-start-symbolic");
    run_btn.add_css_class("flat");
    run_btn.set_valign(gtk::Align::Center);
    run_btn.set_tooltip_text(Some("Run now on the watched folders"));
    {
        let command_tx = command_tx.clone();
        let rule_id = rule.id.clone();
        run_btn.connect_clicked(move |_| {
            let _ = command_tx.send(AppCommand::TriggerAutomationRule {
                rule_id: rule_id.clone(),
            });
        });
    }
    row.add_suffix(&run_btn);

    let edit_btn = gtk::Button::from_icon_name("document-edit-symbolic");
    edit_btn.add_css_class("flat");
    edit_btn.set_valign(gtk::Align::Center);
    edit_btn.set_tooltip_text(Some("Edit"));
    {
        let parent = parent.clone();
        let command_tx = command_tx.clone();
        let rules_dir = rules_dir.to_path_buf();
        let rule = rule.clone();
        let rerender = rerender.clone();
        edit_btn.connect_clicked(move |_| {
            let command_tx = command_tx.clone();
            let rules_dir = rules_dir.clone();
            let rerender = rerender.clone();
            show_rule_dialog(&parent, Some(rule.clone()), move |updated| {
                store_rule(&rules_dir, &updated, &command_tx);
                if let Some(rerender) = &rerender {
                    rerender();
                }
            });
        });
    }
    row.add_suffix(&edit_btn);

    let delete_btn = gtk::Button::from_icon_name("user-trash-symbolic");
    delete_btn.add_css_class("flat");
    delete_btn.add_css_class("destructive-action");
    delete_btn.set_valign(gtk::Align::Center);
    delete_btn.set_tooltip_text(Some("Delete rule"));
    {
        let command_tx = command_tx.clone();
        let rules_dir = rules_dir.to_path_buf();
        let rule_id = rule.id.clone();
        let rerender = rerender.clone();
        delete_btn.connect_clicked(move |_| {
            if let Err(e) = rules_config::delete_rule(&rules_dir, &rule_id) {
                tracing::warn!("{}", e);
            }
            let _ = command_tx.send(AppCommand::RemoveAutomationRule {
                rule_id: rule_id.clone(),
            });
            if let Some(rerender) = &rerender {
                rerender();
            }
        });
    }
    row.add_suffix(&delete_btn);

    let switch = gtk::Switch::new();
    switch.set_valign(gtk::Align::Center);
    switch.set_active(rule.enabled);
    switch.set_tooltip_text(Some("Enabled"));
    {
        let command_tx = command_tx.clone();
        let rules_dir = rules_dir.to_path_buf();
        let rule = rule.clone();
        switch.connect_active_notify(move |sw| {
            let on = sw.is_active();
            let mut updated = rule.clone();
            updated.enabled = on;
            if let Err(e) = rules_config::save_rule(&rules_dir, &updated) {
                tracing::warn!("Failed to save rule {}: {}", updated.id, e);
            }
            let _ = command_tx.send(if on {
                AppCommand::EnableAutomationRule {
                    rule_id: rule.id.clone(),
                }
            } else {
                AppCommand::DisableAutomationRule {
                    rule_id: rule.id.clone(),
                }
            });
        });
    }
    row.add_suffix(&switch);
    row
}
