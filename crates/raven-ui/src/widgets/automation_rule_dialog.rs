use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;
use std::time::Duration;

use gtk4 as gtk;
use libadwaita as adw;
use libadwaita::prelude::*;

use raven_core::automation_types::{Action, AutomationRule, Condition, ConditionMode, Trigger};
use raven_core::custom_actions::{join_args, split_args};

const TRIGGERS: [&str; 5] = [
    "When a file is created",
    "When a file is modified",
    "When a file is deleted",
    "On a schedule",
    "Manually",
];

const CONDITION_TYPES: [(&str, &str); 7] = [
    ("Name matches (regex)", "e.g. ^IMG_.*\\.jpg$"),
    ("Extension is one of", "e.g. jpg, png, gif"),
    ("Larger than", "e.g. 10MB"),
    ("Smaller than", "e.g. 500KB"),
    ("Older than", "e.g. 30d, 12h, 90m"),
    ("Newer than", "e.g. 1h"),
    ("MIME type is", "e.g. image/png"),
];

const ACTION_TYPES: [(&str, &str); 7] = [
    ("Move to folder", "/path/to/folder"),
    ("Copy to folder", "/path/to/folder"),
    ("Rename with pattern", "{name}-{date}.{ext}"),
    ("Delete permanently", ""),
    ("Move to trash", ""),
    ("Run command", "command --flag {file}"),
    ("Show notification", "Message ({file} is the path)"),
];

/// One editable row: what kind, and its value.
struct EditRow {
    container: gtk::Box,
    kind: gtk::DropDown,
    value: gtk::Entry,
}

/// A list of typed rows with a remove button each and an add button.
struct RowList {
    rows: RefCell<Vec<Rc<EditRow>>>,
    list: gtk::Box,
    kinds: &'static [(&'static str, &'static str)],
    /// Kinds whose value entry is meaningless.
    valueless: &'static [u32],
}

impl RowList {
    fn new(kinds: &'static [(&'static str, &'static str)], valueless: &'static [u32]) -> Rc<Self> {
        Rc::new(Self {
            rows: RefCell::new(Vec::new()),
            list: gtk::Box::new(gtk::Orientation::Vertical, 6),
            kinds,
            valueless,
        })
    }

    fn add(self: &Rc<Self>, kind: u32, value: &str) {
        let container = gtk::Box::new(gtk::Orientation::Horizontal, 6);
        let names: Vec<&str> = self.kinds.iter().map(|(n, _)| *n).collect();
        let dropdown = gtk::DropDown::from_strings(&names);
        dropdown.set_selected(kind);
        container.append(&dropdown);

        let entry = gtk::Entry::new();
        entry.set_hexpand(true);
        entry.set_text(value);
        container.append(&entry);

        let remove_btn = gtk::Button::from_icon_name("list-remove-symbolic");
        remove_btn.add_css_class("flat");
        remove_btn.set_tooltip_text(Some("Remove"));
        container.append(&remove_btn);

        let row = Rc::new(EditRow {
            container,
            kind: dropdown,
            value: entry,
        });
        self.apply_kind(&row);
        {
            let this = self.clone();
            let row_ref = row.clone();
            row.kind.connect_selected_notify(move |_| this.apply_kind(&row_ref));
        }
        {
            let this = self.clone();
            let row_ref = row.clone();
            remove_btn.connect_clicked(move |_| {
                this.list.remove(&row_ref.container);
                this.rows
                    .borrow_mut()
                    .retain(|r| !Rc::ptr_eq(r, &row_ref));
            });
        }
        self.list.append(&row.container);
        self.rows.borrow_mut().push(row);
    }

    fn apply_kind(&self, row: &EditRow) {
        let kind = row.kind.selected();
        let placeholder = self.kinds.get(kind as usize).map(|(_, p)| *p).unwrap_or("");
        row.value.set_placeholder_text(Some(placeholder));
        row.value.set_sensitive(!self.valueless.contains(&kind));
    }

    fn entries(&self) -> Vec<(u32, String)> {
        self.rows
            .borrow()
            .iter()
            .map(|r| (r.kind.selected(), r.value.text().trim().to_string()))
            .collect()
    }
}

/// Edit `existing`, or create a new rule when `None`. `on_save` gets the
/// finished rule; the dialog validates before calling it.
pub fn show_rule_dialog(
    parent: &adw::Window,
    existing: Option<AutomationRule>,
    on_save: impl Fn(AutomationRule) + 'static,
) {
    let dialog = adw::Window::builder()
        .title(if existing.is_some() { "Edit Rule" } else { "New Rule" })
        .default_width(560)
        .default_height(640)
        .modal(true)
        .transient_for(parent)
        .build();

    let toolbar_view = adw::ToolbarView::new();
    toolbar_view.add_top_bar(&adw::HeaderBar::new());

    let scroll = gtk::ScrolledWindow::builder()
        .hscrollbar_policy(gtk::PolicyType::Never)
        .vscrollbar_policy(gtk::PolicyType::Automatic)
        .vexpand(true)
        .build();
    let content = gtk::Box::new(gtk::Orientation::Vertical, 16);
    content.set_margin_start(16);
    content.set_margin_end(16);
    content.set_margin_top(12);
    content.set_margin_bottom(16);

    // --- Name and trigger ---
    let basics = adw::PreferencesGroup::new();
    basics.set_title("Rule");

    let name_row = adw::EntryRow::new();
    name_row.set_title("Name");
    basics.add(&name_row);

    let trigger_row = adw::ComboRow::new();
    trigger_row.set_title("Trigger");
    trigger_row.set_model(Some(&gtk::StringList::new(&TRIGGERS)));
    basics.add(&trigger_row);

    let cron_row = adw::EntryRow::new();
    cron_row.set_title("Schedule (cron: */15 * * * *, 0 */2 * * *, 0 * * * *)");
    cron_row.set_visible(false);
    basics.add(&cron_row);
    {
        let cron_row = cron_row.clone();
        trigger_row.connect_selected_notify(move |row| cron_row.set_visible(row.selected() == 3));
    }

    let paths_row = adw::EntryRow::new();
    paths_row.set_title("Folders to watch (separate with ;)");
    basics.add(&paths_row);

    let enabled_row = adw::SwitchRow::new();
    enabled_row.set_title("Enabled");
    enabled_row.set_active(true);
    basics.add(&enabled_row);
    content.append(&basics);

    // --- Conditions ---
    let conditions_group = adw::PreferencesGroup::new();
    conditions_group.set_title("Conditions");
    conditions_group.set_description(Some("Files the rule applies to. With no conditions, every file matches."));
    let mode_row = adw::ComboRow::new();
    mode_row.set_title("Match");
    mode_row.set_model(Some(&gtk::StringList::new(&["All conditions", "Any condition"])));
    conditions_group.add(&mode_row);
    let conditions = RowList::new(&CONDITION_TYPES, &[]);
    conditions_group.add(&conditions.list);
    let add_condition = gtk::Button::with_label("Add Condition");
    add_condition.set_halign(gtk::Align::Start);
    {
        let conditions = conditions.clone();
        add_condition.connect_clicked(move |_| conditions.add(0, ""));
    }
    conditions_group.add(&add_condition);
    content.append(&conditions_group);

    // --- Actions ---
    let actions_group = adw::PreferencesGroup::new();
    actions_group.set_title("Actions");
    actions_group.set_description(Some("Run in order on each matching file."));
    let actions = RowList::new(&ACTION_TYPES, &[3, 4]);
    actions_group.add(&actions.list);
    let add_action = gtk::Button::with_label("Add Action");
    add_action.set_halign(gtk::Align::Start);
    {
        let actions = actions.clone();
        add_action.connect_clicked(move |_| actions.add(0, ""));
    }
    actions_group.add(&add_action);
    content.append(&actions_group);

    // Conditions the editor cannot show (nested groups) are kept as they are.
    let kept_conditions: Rc<RefCell<Vec<Condition>>> = Rc::new(RefCell::new(Vec::new()));
    let kept_note = gtk::Label::new(None);
    kept_note.add_css_class("dim-label");
    kept_note.add_css_class("caption");
    kept_note.set_halign(gtk::Align::Start);
    kept_note.set_visible(false);
    content.append(&kept_note);

    let error_label = gtk::Label::new(None);
    error_label.add_css_class("error");
    error_label.set_halign(gtk::Align::Start);
    error_label.set_wrap(true);
    error_label.set_xalign(0.0);
    error_label.set_visible(false);
    content.append(&error_label);

    // --- Fill from the existing rule ---
    let existing_id = existing.as_ref().map(|r| r.id.clone());
    if let Some(rule) = &existing {
        name_row.set_text(&rule.name);
        trigger_row.set_selected(match &rule.trigger {
            Trigger::FileCreated => 0,
            Trigger::FileModified => 1,
            Trigger::FileDeleted => 2,
            Trigger::Schedule { cron } => {
                cron_row.set_text(cron);
                3
            }
            Trigger::Manual => 4,
        });
        paths_row.set_text(
            &rule
                .watch_paths
                .iter()
                .map(|p| p.to_string_lossy().to_string())
                .collect::<Vec<_>>()
                .join("; "),
        );
        enabled_row.set_active(rule.enabled);
        mode_row.set_selected(match rule.condition_mode {
            ConditionMode::All => 0,
            ConditionMode::Any => 1,
        });
        let mut kept = 0;
        for condition in &rule.conditions {
            match condition_to_row(condition) {
                Some((kind, value)) => conditions.add(kind, &value),
                None => {
                    kept_conditions.borrow_mut().push(condition.clone());
                    kept += 1;
                }
            }
        }
        if kept > 0 {
            kept_note.set_text(&format!(
                "{} nested condition group(s) from the rule file are kept as they are.",
                kept
            ));
            kept_note.set_visible(true);
        }
        for action in &rule.actions {
            let (kind, value) = action_to_row(action);
            actions.add(kind, &value);
        }
    } else {
        actions.add(0, "");
    }

    // --- Buttons ---
    let btn_box = gtk::Box::new(gtk::Orientation::Horizontal, 8);
    btn_box.set_halign(gtk::Align::End);
    let cancel_btn = gtk::Button::with_label("Cancel");
    {
        let dialog = dialog.clone();
        cancel_btn.connect_clicked(move |_| dialog.close());
    }
    btn_box.append(&cancel_btn);
    let save_btn = gtk::Button::with_label("Save");
    save_btn.add_css_class("suggested-action");
    {
        let dialog = dialog.clone();
        let conditions = conditions.clone();
        let actions = actions.clone();
        let kept_conditions = kept_conditions.clone();
        let error_label = error_label.clone();
        save_btn.connect_clicked(move |_| {
            let input = RuleInput {
                id: existing_id.clone(),
                name: name_row.text().trim().to_string(),
                trigger: trigger_row.selected(),
                cron: cron_row.text().trim().to_string(),
                watch_paths: paths_row.text().to_string(),
                enabled: enabled_row.is_active(),
                any_mode: mode_row.selected() == 1,
                conditions: conditions.entries(),
                kept_conditions: kept_conditions.borrow().clone(),
                actions: actions.entries(),
            };
            match build_rule(input) {
                Ok(rule) => {
                    on_save(rule);
                    dialog.close();
                }
                Err(reason) => {
                    error_label.set_text(&reason);
                    error_label.set_visible(true);
                }
            }
        });
    }
    btn_box.append(&save_btn);
    content.append(&btn_box);

    scroll.set_child(Some(&content));
    toolbar_view.set_content(Some(&scroll));
    dialog.set_content(Some(&toolbar_view));
    dialog.present();
}

/// Everything read from the form, before validation.
struct RuleInput {
    id: Option<String>,
    name: String,
    trigger: u32,
    cron: String,
    watch_paths: String,
    enabled: bool,
    any_mode: bool,
    conditions: Vec<(u32, String)>,
    kept_conditions: Vec<Condition>,
    actions: Vec<(u32, String)>,
}

fn build_rule(input: RuleInput) -> Result<AutomationRule, String> {
    if input.name.is_empty() {
        return Err("The rule needs a name".to_string());
    }
    let trigger = match input.trigger {
        0 => Trigger::FileCreated,
        1 => Trigger::FileModified,
        2 => Trigger::FileDeleted,
        3 => {
            if input.cron.is_empty() {
                return Err("A schedule needs a cron expression".to_string());
            }
            if raven_automation::scheduler::parse_cron_interval(&input.cron).is_none() {
                return Err(
                    "Only simple schedules are supported: */N * * * *, 0 */N * * *, * * * * *, 0 * * * *"
                        .to_string(),
                );
            }
            Trigger::Schedule { cron: input.cron }
        }
        _ => Trigger::Manual,
    };
    let watch_paths: Vec<PathBuf> = input
        .watch_paths
        .split(';')
        .map(|p| p.trim())
        .filter(|p| !p.is_empty())
        .map(expand_tilde)
        .collect();
    if watch_paths.is_empty() {
        return Err("The rule needs at least one folder to watch".to_string());
    }

    let mut conditions = Vec::new();
    for (kind, value) in input.conditions {
        conditions.push(row_to_condition(kind, &value)?);
    }
    conditions.extend(input.kept_conditions);

    let mut actions = Vec::new();
    for (kind, value) in input.actions {
        actions.push(row_to_action(kind, &value)?);
    }
    if actions.is_empty() {
        return Err("The rule needs at least one action".to_string());
    }

    Ok(AutomationRule {
        id: input.id.unwrap_or_else(|| new_rule_id(&input.name)),
        name: input.name,
        enabled: input.enabled,
        trigger,
        conditions,
        condition_mode: if input.any_mode {
            ConditionMode::Any
        } else {
            ConditionMode::All
        },
        actions,
        watch_paths,
    })
}

fn condition_to_row(condition: &Condition) -> Option<(u32, String)> {
    Some(match condition {
        Condition::NameMatches { pattern } => (0, pattern.clone()),
        Condition::ExtensionIs { extensions } => (1, extensions.join(", ")),
        Condition::SizeLargerThan { bytes } => (2, format_size_input(*bytes)),
        Condition::SizeSmallerThan { bytes } => (3, format_size_input(*bytes)),
        Condition::OlderThan { duration } => (4, format_duration_input(*duration)),
        Condition::NewerThan { duration } => (5, format_duration_input(*duration)),
        Condition::MimeTypeIs { mime_type } => (6, mime_type.clone()),
        Condition::All { .. } | Condition::Any { .. } | Condition::Not { .. } => return None,
    })
}

fn row_to_condition(kind: u32, value: &str) -> Result<Condition, String> {
    let need = |what: &str| -> Result<String, String> {
        if value.is_empty() {
            Err(format!("The \"{}\" condition needs a value", what))
        } else {
            Ok(value.to_string())
        }
    };
    Ok(match kind {
        0 => {
            let pattern = need("Name matches")?;
            regex_check(&pattern)?;
            Condition::NameMatches { pattern }
        }
        1 => Condition::ExtensionIs {
            extensions: need("Extension is one of")?
                .split(',')
                .map(|e| e.trim().trim_start_matches('.').to_ascii_lowercase())
                .filter(|e| !e.is_empty())
                .collect(),
        },
        2 => Condition::SizeLargerThan {
            bytes: parse_size(&need("Larger than")?)?,
        },
        3 => Condition::SizeSmallerThan {
            bytes: parse_size(&need("Smaller than")?)?,
        },
        4 => Condition::OlderThan {
            duration: parse_duration(&need("Older than")?)?,
        },
        5 => Condition::NewerThan {
            duration: parse_duration(&need("Newer than")?)?,
        },
        _ => Condition::MimeTypeIs {
            mime_type: need("MIME type is")?,
        },
    })
}

fn regex_check(pattern: &str) -> Result<(), String> {
    // The engine compiles the pattern with the same crate; a bad one would
    // only fail at match time, so refuse it here.
    regex_lite_check(pattern)
}

fn regex_lite_check(pattern: &str) -> Result<(), String> {
    let mut depth = 0i32;
    for c in pattern.chars() {
        match c {
            '(' => depth += 1,
            ')' => depth -= 1,
            _ => {}
        }
        if depth < 0 {
            return Err("The pattern has an unmatched ')'".to_string());
        }
    }
    if depth != 0 {
        return Err("The pattern has an unmatched '('".to_string());
    }
    Ok(())
}

fn action_to_row(action: &Action) -> (u32, String) {
    match action {
        Action::MoveTo { destination } => (0, destination.to_string_lossy().to_string()),
        Action::CopyTo { destination } => (1, destination.to_string_lossy().to_string()),
        Action::Rename { pattern } => (2, pattern.clone()),
        Action::Delete => (3, String::new()),
        Action::Trash => (4, String::new()),
        Action::RunCommand { command, args } => {
            let mut all = vec![command.clone()];
            all.extend(args.iter().cloned());
            (5, join_args(&all))
        }
        Action::Notify { message } => (6, message.clone()),
    }
}

fn row_to_action(kind: u32, value: &str) -> Result<Action, String> {
    let need = |what: &str| -> Result<String, String> {
        if value.is_empty() {
            Err(format!("The \"{}\" action needs a value", what))
        } else {
            Ok(value.to_string())
        }
    };
    Ok(match kind {
        0 => Action::MoveTo {
            destination: expand_tilde(&need("Move to folder")?),
        },
        1 => Action::CopyTo {
            destination: expand_tilde(&need("Copy to folder")?),
        },
        2 => Action::Rename {
            pattern: need("Rename with pattern")?,
        },
        3 => Action::Delete,
        4 => Action::Trash,
        5 => {
            let mut parts = split_args(&need("Run command")?);
            if parts.is_empty() {
                return Err("The \"Run command\" action needs a command".to_string());
            }
            let command = parts.remove(0);
            Action::RunCommand {
                command,
                args: parts,
            }
        }
        _ => Action::Notify {
            message: need("Show notification")?,
        },
    })
}

fn expand_tilde(path: &str) -> PathBuf {
    if let Some(rest) = path.strip_prefix("~/") {
        if let Some(home) = std::env::var_os("HOME") {
            return PathBuf::from(home).join(rest);
        }
    }
    PathBuf::from(path)
}

/// A file-system-safe id from the name plus a few characters of the clock,
/// so two rules with the same name do not overwrite each other's file.
fn new_rule_id(name: &str) -> String {
    let mut slug: String = name
        .to_ascii_lowercase()
        .chars()
        .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
        .collect();
    while slug.contains("--") {
        slug = slug.replace("--", "-");
    }
    let slug = slug.trim_matches('-');
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    let slug = if slug.is_empty() { "rule" } else { slug };
    format!("{}-{:04x}", slug, nanos & 0xffff)
}

/// "10MB", "500 KiB", "1024" -> bytes.
pub fn parse_size(text: &str) -> Result<u64, String> {
    let text = text.trim().to_ascii_lowercase().replace(' ', "");
    let split = text
        .find(|c: char| !c.is_ascii_digit() && c != '.')
        .unwrap_or(text.len());
    let (number, unit) = text.split_at(split);
    let value: f64 = number
        .parse()
        .map_err(|_| format!("\"{}\" is not a size", text))?;
    let multiplier: f64 = match unit.trim_end_matches('b').trim_end_matches('i') {
        "" => 1.0,
        "k" => 1024.0,
        "m" => 1024.0 * 1024.0,
        "g" => 1024.0 * 1024.0 * 1024.0,
        "t" => 1024.0 * 1024.0 * 1024.0 * 1024.0,
        _ => return Err(format!("\"{}\" is not a size; use B, KB, MB, GB or TB", text)),
    };
    Ok((value * multiplier).round() as u64)
}

pub fn format_size_input(bytes: u64) -> String {
    const UNITS: [(&str, u64); 4] = [
        ("TB", 1 << 40),
        ("GB", 1 << 30),
        ("MB", 1 << 20),
        ("KB", 1 << 10),
    ];
    for (unit, size) in UNITS {
        if bytes >= size && bytes % size == 0 {
            return format!("{}{}", bytes / size, unit);
        }
    }
    format!("{}", bytes)
}

/// "30d", "12h", "90m", "45s", "600" (seconds) -> duration.
pub fn parse_duration(text: &str) -> Result<Duration, String> {
    let text = text.trim().to_ascii_lowercase().replace(' ', "");
    let split = text
        .find(|c: char| !c.is_ascii_digit())
        .unwrap_or(text.len());
    let (number, unit) = text.split_at(split);
    let value: u64 = number
        .parse()
        .map_err(|_| format!("\"{}\" is not a duration", text))?;
    let seconds = match unit {
        "" | "s" | "sec" | "secs" => value,
        "m" | "min" | "mins" => value * 60,
        "h" | "hr" | "hrs" => value * 3600,
        "d" | "day" | "days" => value * 86_400,
        "w" | "week" | "weeks" => value * 7 * 86_400,
        _ => return Err(format!("\"{}\" is not a duration; use s, m, h, d or w", text)),
    };
    Ok(Duration::from_secs(seconds))
}

pub fn format_duration_input(duration: Duration) -> String {
    let secs = duration.as_secs();
    const UNITS: [(&str, u64); 4] = [("w", 7 * 86_400), ("d", 86_400), ("h", 3600), ("m", 60)];
    for (unit, size) in UNITS {
        if secs >= size && secs % size == 0 {
            return format!("{}{}", secs / size, unit);
        }
    }
    format!("{}s", secs)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_parse_and_format() {
        assert_eq!(parse_size("10MB").unwrap(), 10 * 1024 * 1024);
        assert_eq!(parse_size("500 KiB").unwrap(), 500 * 1024);
        assert_eq!(parse_size("1024").unwrap(), 1024);
        assert!(parse_size("ten").is_err());
        assert_eq!(format_size_input(10 * 1024 * 1024), "10MB");
        assert_eq!(format_size_input(1500), "1500");
    }

    #[test]
    fn durations_parse_and_format() {
        assert_eq!(parse_duration("30d").unwrap(), Duration::from_secs(30 * 86_400));
        assert_eq!(parse_duration("90m").unwrap(), Duration::from_secs(5400));
        assert_eq!(parse_duration("600").unwrap(), Duration::from_secs(600));
        assert!(parse_duration("soon").is_err());
        assert_eq!(format_duration_input(Duration::from_secs(5400)), "90m");
        assert_eq!(format_duration_input(Duration::from_secs(2 * 86_400)), "2d");
    }

    #[test]
    fn rows_round_trip_through_rule_types() {
        let condition = Condition::ExtensionIs {
            extensions: vec!["jpg".into(), "png".into()],
        };
        let (kind, value) = condition_to_row(&condition).unwrap();
        assert_eq!(row_to_condition(kind, &value).unwrap(), condition);

        let action = Action::RunCommand {
            command: "notify-send".into(),
            args: vec!["new file".into(), "{file}".into()],
        };
        let (kind, value) = action_to_row(&action);
        assert_eq!(row_to_action(kind, &value).unwrap(), action);
    }

    #[test]
    fn build_rule_validates() {
        let base = || RuleInput {
            id: None,
            name: "Sort photos".into(),
            trigger: 0,
            cron: String::new(),
            watch_paths: "/tmp/in; /tmp/other".into(),
            enabled: true,
            any_mode: false,
            conditions: vec![(1, "jpg".into())],
            kept_conditions: vec![],
            actions: vec![(0, "/tmp/out".into())],
        };
        let rule = build_rule(base()).unwrap();
        assert!(rule.id.starts_with("sort-photos-"));
        assert_eq!(rule.watch_paths.len(), 2);
        assert_eq!(rule.trigger, Trigger::FileCreated);

        let mut no_actions = base();
        no_actions.actions.clear();
        assert!(build_rule(no_actions).is_err());

        let mut no_paths = base();
        no_paths.watch_paths = " ; ".into();
        assert!(build_rule(no_paths).is_err());

        let mut bad_cron = base();
        bad_cron.trigger = 3;
        bad_cron.cron = "every tuesday".into();
        assert!(build_rule(bad_cron).is_err());

        let mut edit = base();
        edit.id = Some("keep-me".into());
        assert_eq!(build_rule(edit).unwrap().id, "keep-me");
    }
}
