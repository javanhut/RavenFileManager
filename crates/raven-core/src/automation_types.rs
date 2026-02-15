use std::path::PathBuf;
use std::time::Duration;

use serde::{Deserialize, Serialize};

/// A complete automation rule defining trigger, conditions, and actions.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AutomationRule {
    pub id: String,
    pub name: String,
    #[serde(default = "default_true")]
    pub enabled: bool,
    pub trigger: Trigger,
    #[serde(default)]
    pub conditions: Vec<Condition>,
    #[serde(default)]
    pub condition_mode: ConditionMode,
    pub actions: Vec<Action>,
    #[serde(default)]
    pub watch_paths: Vec<PathBuf>,
}

fn default_true() -> bool {
    true
}

/// What triggers a rule to evaluate.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Trigger {
    /// New file appears in a watched directory.
    FileCreated,
    /// Existing file is modified in a watched directory.
    FileModified,
    /// File is deleted from a watched directory.
    FileDeleted,
    /// Cron-style schedule (e.g., "0 * * * *" for every hour).
    Schedule { cron: String },
    /// Manually triggered by user or DBus.
    Manual,
}

/// How multiple conditions are combined.
#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum ConditionMode {
    /// All conditions must match.
    #[default]
    All,
    /// Any condition must match.
    Any,
}

/// A condition that a file must satisfy for the rule to apply.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Condition {
    /// Filename matches a regex pattern.
    NameMatches { pattern: String },
    /// File extension is one of the listed values.
    ExtensionIs { extensions: Vec<String> },
    /// File size is larger than the given number of bytes.
    SizeLargerThan { bytes: u64 },
    /// File size is smaller than the given number of bytes.
    SizeSmallerThan { bytes: u64 },
    /// File is older than the given duration (in seconds).
    OlderThan {
        #[serde(with = "duration_secs")]
        duration: Duration,
    },
    /// File is newer than the given duration (in seconds).
    NewerThan {
        #[serde(with = "duration_secs")]
        duration: Duration,
    },
    /// File MIME type matches.
    MimeTypeIs { mime_type: String },
    /// All sub-conditions must match.
    All { conditions: Vec<Condition> },
    /// Any sub-condition must match.
    Any { conditions: Vec<Condition> },
    /// Negation of a sub-condition.
    Not { condition: Box<Condition> },
}

/// An action to perform on files that match the rule.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Action {
    /// Move matched files to the destination directory.
    MoveTo { destination: PathBuf },
    /// Copy matched files to the destination directory.
    CopyTo { destination: PathBuf },
    /// Rename matched files using a pattern.
    /// Supports placeholders: {name}, {ext}, {date}, {counter}.
    Rename { pattern: String },
    /// Permanently delete matched files.
    Delete,
    /// Move matched files to trash.
    Trash,
    /// Run a shell command. {file} is replaced with the file path.
    RunCommand { command: String, args: Vec<String> },
    /// Send a desktop notification.
    Notify { message: String },
}

/// SSH authentication method for SFTP connections.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SshAuth {
    /// Password authentication.
    Password { password: String },
    /// Public key file authentication.
    KeyFile { path: PathBuf, passphrase: Option<String> },
    /// SSH agent authentication.
    Agent,
}

/// Serde helper for Duration as seconds.
mod duration_secs {
    use std::time::Duration;

    use serde::{Deserialize, Deserializer, Serialize, Serializer};

    pub fn serialize<S: Serializer>(duration: &Duration, s: S) -> Result<S::Ok, S::Error> {
        duration.as_secs().serialize(s)
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Duration, D::Error> {
        let secs = u64::deserialize(d)?;
        Ok(Duration::from_secs(secs))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_automation_rule_serialization() {
        let rule = AutomationRule {
            id: "rule-1".to_string(),
            name: "Move downloads".to_string(),
            enabled: true,
            trigger: Trigger::FileCreated,
            conditions: vec![
                Condition::ExtensionIs {
                    extensions: vec!["pdf".to_string(), "doc".to_string()],
                },
            ],
            condition_mode: ConditionMode::All,
            actions: vec![
                Action::MoveTo {
                    destination: PathBuf::from("/home/user/Documents"),
                },
            ],
            watch_paths: vec![PathBuf::from("/home/user/Downloads")],
        };

        let toml_str = toml::to_string_pretty(&rule).unwrap();
        let deserialized: AutomationRule = toml::from_str(&toml_str).unwrap();
        assert_eq!(deserialized.id, "rule-1");
        assert_eq!(deserialized.name, "Move downloads");
        assert!(deserialized.enabled);
    }

    #[test]
    fn test_condition_combinators() {
        let condition = Condition::All {
            conditions: vec![
                Condition::ExtensionIs {
                    extensions: vec!["jpg".to_string()],
                },
                Condition::SizeLargerThan { bytes: 1024 },
            ],
        };

        let toml_str = toml::to_string_pretty(&condition).unwrap();
        let deserialized: Condition = toml::from_str(&toml_str).unwrap();
        assert_eq!(deserialized, condition);
    }

    #[test]
    fn test_ssh_auth_serialization() {
        let auth = SshAuth::KeyFile {
            path: PathBuf::from("/home/user/.ssh/id_ed25519"),
            passphrase: None,
        };
        let toml_str = toml::to_string_pretty(&auth).unwrap();
        let deserialized: SshAuth = toml::from_str(&toml_str).unwrap();
        match deserialized {
            SshAuth::KeyFile { path, passphrase } => {
                assert_eq!(path, PathBuf::from("/home/user/.ssh/id_ed25519"));
                assert!(passphrase.is_none());
            }
            _ => panic!("Expected KeyFile variant"),
        }
    }

    #[test]
    fn test_trigger_variants() {
        let trigger = Trigger::Schedule {
            cron: "0 * * * *".to_string(),
        };
        let toml_str = toml::to_string_pretty(&trigger).unwrap();
        assert!(toml_str.contains("schedule"));

        let manual = Trigger::Manual;
        let toml_str = toml::to_string_pretty(&manual).unwrap();
        let deserialized: Trigger = toml::from_str(&toml_str).unwrap();
        assert_eq!(deserialized, Trigger::Manual);
    }

    #[test]
    fn test_action_variants() {
        let action = Action::RunCommand {
            command: "convert".to_string(),
            args: vec!["{file}".to_string(), "-resize".to_string(), "50%".to_string()],
        };
        let toml_str = toml::to_string_pretty(&action).unwrap();
        let deserialized: Action = toml::from_str(&toml_str).unwrap();
        assert_eq!(deserialized, action);
    }
}
