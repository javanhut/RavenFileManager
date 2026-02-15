use std::path::{Path, PathBuf};

use tracing::{info, warn};

use raven_core::automation_types::AutomationRule;

/// Load all automation rules from a directory of TOML files.
///
/// Each file should be named `{rule_id}.toml` and contain a single serialized
/// `AutomationRule`. Files that fail to parse are logged and skipped.
pub fn load_rules(dir: &Path) -> Vec<AutomationRule> {
    let mut rules = Vec::new();

    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(e) => {
            warn!(dir = %dir.display(), error = %e, "failed to read rules directory");
            return rules;
        }
    };

    for entry in entries {
        let entry = match entry {
            Ok(e) => e,
            Err(e) => {
                warn!(error = %e, "failed to read directory entry");
                continue;
            }
        };

        let path = entry.path();
        if path.extension().and_then(|e| e.to_str()) != Some("toml") {
            continue;
        }

        match std::fs::read_to_string(&path) {
            Ok(content) => match toml::from_str::<AutomationRule>(&content) {
                Ok(rule) => {
                    info!(rule_id = %rule.id, name = %rule.name, "loaded automation rule");
                    rules.push(rule);
                }
                Err(e) => {
                    warn!(path = %path.display(), error = %e, "failed to parse rule file");
                }
            },
            Err(e) => {
                warn!(path = %path.display(), error = %e, "failed to read rule file");
            }
        }
    }

    rules
}

/// Save a rule to a TOML file in the specified directory.
///
/// The file is named `{rule_id}.toml`.
pub fn save_rule(dir: &Path, rule: &AutomationRule) -> Result<(), String> {
    std::fs::create_dir_all(dir)
        .map_err(|e| format!("failed to create rules directory: {}", e))?;

    let content =
        toml::to_string_pretty(rule).map_err(|e| format!("failed to serialize rule: {}", e))?;

    let file_path = dir.join(format!("{}.toml", rule.id));
    std::fs::write(&file_path, content)
        .map_err(|e| format!("failed to write rule file: {}", e))?;

    info!(rule_id = %rule.id, path = %file_path.display(), "saved automation rule");
    Ok(())
}

/// Delete a rule file from the specified directory.
pub fn delete_rule(dir: &Path, rule_id: &str) -> Result<(), String> {
    let file_path = dir.join(format!("{}.toml", rule_id));

    if !file_path.exists() {
        return Err(format!("rule file not found: {}", file_path.display()));
    }

    std::fs::remove_file(&file_path)
        .map_err(|e| format!("failed to delete rule file: {}", e))?;

    info!(rule_id, path = %file_path.display(), "deleted automation rule file");
    Ok(())
}

/// Get the default rules directory (`~/.config/raven/rules/`).
pub fn default_rules_dir() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
    PathBuf::from(home)
        .join(".config")
        .join("raven")
        .join("rules")
}

#[cfg(test)]
mod tests {
    use super::*;
    use raven_core::automation_types::*;
    use tempfile::TempDir;

    fn sample_rule(id: &str) -> AutomationRule {
        AutomationRule {
            id: id.to_string(),
            name: format!("Test Rule {}", id),
            enabled: true,
            trigger: Trigger::FileCreated,
            conditions: vec![Condition::ExtensionIs {
                extensions: vec!["pdf".to_string(), "doc".to_string()],
            }],
            condition_mode: ConditionMode::All,
            actions: vec![Action::MoveTo {
                destination: PathBuf::from("/home/user/Documents"),
            }],
            watch_paths: vec![PathBuf::from("/home/user/Downloads")],
        }
    }

    #[test]
    fn test_save_and_load_roundtrip() {
        let dir = TempDir::new().unwrap();
        let rule = sample_rule("rule-1");

        save_rule(dir.path(), &rule).unwrap();

        // Verify the file was created
        let file_path = dir.path().join("rule-1.toml");
        assert!(file_path.exists());

        // Load rules back
        let rules = load_rules(dir.path());
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].id, "rule-1");
        assert_eq!(rules[0].name, "Test Rule rule-1");
        assert!(rules[0].enabled);
    }

    #[test]
    fn test_save_multiple_and_load() {
        let dir = TempDir::new().unwrap();

        save_rule(dir.path(), &sample_rule("alpha")).unwrap();
        save_rule(dir.path(), &sample_rule("beta")).unwrap();
        save_rule(dir.path(), &sample_rule("gamma")).unwrap();

        let rules = load_rules(dir.path());
        assert_eq!(rules.len(), 3);

        let ids: Vec<&str> = rules.iter().map(|r| r.id.as_str()).collect();
        assert!(ids.contains(&"alpha"));
        assert!(ids.contains(&"beta"));
        assert!(ids.contains(&"gamma"));
    }

    #[test]
    fn test_delete_rule() {
        let dir = TempDir::new().unwrap();
        save_rule(dir.path(), &sample_rule("to-delete")).unwrap();

        assert!(dir.path().join("to-delete.toml").exists());

        delete_rule(dir.path(), "to-delete").unwrap();
        assert!(!dir.path().join("to-delete.toml").exists());
    }

    #[test]
    fn test_delete_nonexistent_rule() {
        let dir = TempDir::new().unwrap();
        let result = delete_rule(dir.path(), "nonexistent");
        assert!(result.is_err());
    }

    #[test]
    fn test_load_empty_directory() {
        let dir = TempDir::new().unwrap();
        let rules = load_rules(dir.path());
        assert!(rules.is_empty());
    }

    #[test]
    fn test_load_nonexistent_directory() {
        let rules = load_rules(Path::new("/tmp/raven_nonexistent_rules_dir_12345"));
        assert!(rules.is_empty());
    }

    #[test]
    fn test_load_skips_non_toml_files() {
        let dir = TempDir::new().unwrap();

        // Save a valid rule
        save_rule(dir.path(), &sample_rule("valid")).unwrap();

        // Write a non-TOML file
        std::fs::write(dir.path().join("readme.txt"), "not a rule").unwrap();

        let rules = load_rules(dir.path());
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].id, "valid");
    }

    #[test]
    fn test_load_skips_malformed_toml() {
        let dir = TempDir::new().unwrap();

        // Save a valid rule
        save_rule(dir.path(), &sample_rule("good")).unwrap();

        // Write a malformed TOML file
        std::fs::write(dir.path().join("bad.toml"), "this is not valid toml [[[").unwrap();

        let rules = load_rules(dir.path());
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].id, "good");
    }

    #[test]
    fn test_save_creates_directory() {
        let dir = TempDir::new().unwrap();
        let nested = dir.path().join("a").join("b").join("c");

        save_rule(&nested, &sample_rule("nested-rule")).unwrap();
        assert!(nested.join("nested-rule.toml").exists());
    }

    #[test]
    fn test_save_overwrites_existing() {
        let dir = TempDir::new().unwrap();
        let mut rule = sample_rule("overwrite");
        save_rule(dir.path(), &rule).unwrap();

        // Modify and save again
        rule.name = "Updated Name".to_string();
        save_rule(dir.path(), &rule).unwrap();

        let rules = load_rules(dir.path());
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].name, "Updated Name");
    }

    #[test]
    fn test_default_rules_dir() {
        let dir = default_rules_dir();
        // Should end with .config/raven/rules
        let path_str = dir.to_string_lossy();
        assert!(path_str.ends_with(".config/raven/rules"));
    }

    #[test]
    fn test_roundtrip_with_complex_rule() {
        let dir = TempDir::new().unwrap();

        let rule = AutomationRule {
            id: "complex".to_string(),
            name: "Complex Rule".to_string(),
            enabled: false,
            trigger: Trigger::Schedule {
                cron: "*/5 * * * *".to_string(),
            },
            conditions: vec![
                Condition::ExtensionIs {
                    extensions: vec!["jpg".to_string(), "png".to_string()],
                },
                Condition::SizeLargerThan { bytes: 1_048_576 },
                Condition::Not {
                    condition: Box::new(Condition::NameMatches {
                        pattern: "^temp".to_string(),
                    }),
                },
            ],
            condition_mode: ConditionMode::Any,
            actions: vec![
                Action::CopyTo {
                    destination: PathBuf::from("/backup/images"),
                },
                Action::Notify {
                    message: "Backed up {file}".to_string(),
                },
            ],
            watch_paths: vec![
                PathBuf::from("/home/user/Pictures"),
                PathBuf::from("/home/user/Screenshots"),
            ],
        };

        save_rule(dir.path(), &rule).unwrap();
        let rules = load_rules(dir.path());
        assert_eq!(rules.len(), 1);
        assert_eq!(rules[0].id, "complex");
        assert!(!rules[0].enabled);
        assert_eq!(rules[0].conditions.len(), 3);
        assert_eq!(rules[0].actions.len(), 2);
        assert_eq!(rules[0].watch_paths.len(), 2);
    }
}
