use std::path::PathBuf;

use raven_core::commands::{AppCommand, SearchMode};
use raven_core::events::AppEvent;
use raven_core::path::RavenPath;

/// Represents a method call on the Raven DBus interface.
/// This abstraction allows testing without a real DBus connection.
#[derive(Debug, Clone)]
pub enum DbusMethod {
    Navigate { path: String },
    GetCurrentPath,
    GetSelection,
    CopyFiles { sources: Vec<String>, dest: String },
    MoveFiles { sources: Vec<String>, dest: String },
    DeleteFiles { paths: Vec<String> },
    TrashFiles { paths: Vec<String> },
    Search { query: String, path: String },
    TriggerAutomationRule { rule_id: String },
}

/// Represents a DBus signal emitted by Raven.
#[derive(Debug, Clone)]
pub enum DbusSignal {
    DirectoryChanged { path: String },
    OperationCompleted { op_id: String, kind: String },
    FilesSelected { paths: Vec<String> },
}

/// Result of a DBus method call.
#[derive(Debug, Clone)]
pub enum DbusResult {
    Success,
    Path(String),
    Paths(Vec<String>),
    Error(String),
}

/// Convert a DBus method call to an AppCommand.
/// Returns None for query-only methods (GetCurrentPath, GetSelection).
pub fn method_to_command(method: &DbusMethod) -> Option<AppCommand> {
    match method {
        DbusMethod::Navigate { path } => Some(AppCommand::Navigate {
            path: RavenPath::local(PathBuf::from(path)),
            pane_id: 0,
        }),
        DbusMethod::CopyFiles { sources, dest } => Some(AppCommand::CopyFiles {
            sources: sources
                .iter()
                .map(|s| RavenPath::local(PathBuf::from(s)))
                .collect(),
            destination: RavenPath::local(PathBuf::from(dest)),
        }),
        DbusMethod::MoveFiles { sources, dest } => Some(AppCommand::MoveFiles {
            sources: sources
                .iter()
                .map(|s| RavenPath::local(PathBuf::from(s)))
                .collect(),
            destination: RavenPath::local(PathBuf::from(dest)),
        }),
        DbusMethod::DeleteFiles { paths } => Some(AppCommand::DeleteFiles {
            paths: paths
                .iter()
                .map(|s| RavenPath::local(PathBuf::from(s)))
                .collect(),
        }),
        DbusMethod::TrashFiles { paths } => Some(AppCommand::TrashFiles {
            paths: paths
                .iter()
                .map(|s| RavenPath::local(PathBuf::from(s)))
                .collect(),
        }),
        DbusMethod::Search { query, path } => Some(AppCommand::Search {
            query: query.clone(),
            path: RavenPath::local(PathBuf::from(path)),
            search_mode: SearchMode::Filename,
        }),
        DbusMethod::TriggerAutomationRule { rule_id } => {
            Some(AppCommand::TriggerAutomationRule {
                rule_id: rule_id.clone(),
            })
        }
        DbusMethod::GetCurrentPath | DbusMethod::GetSelection => None,
    }
}

/// Convert an AppEvent to a DbusSignal, if applicable.
/// Returns None for events that don't map to signals.
pub fn event_to_signal(event: &AppEvent) -> Option<DbusSignal> {
    match event {
        AppEvent::DirectoryLoaded { path, .. } => Some(DbusSignal::DirectoryChanged {
            path: path.to_string(),
        }),
        AppEvent::OperationCompleted { id } => Some(DbusSignal::OperationCompleted {
            op_id: id.0.to_string(),
            kind: "completed".to_string(),
        }),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use raven_core::operations::OperationId;

    #[test]
    fn method_to_command_navigate() {
        let method = DbusMethod::Navigate {
            path: "/home/user/Documents".to_string(),
        };
        let cmd = method_to_command(&method).expect("should produce a command");
        match cmd {
            AppCommand::Navigate { path, pane_id } => {
                assert_eq!(path, RavenPath::local(PathBuf::from("/home/user/Documents")));
                assert_eq!(pane_id, 0);
            }
            other => panic!("expected Navigate, got {:?}", other),
        }
    }

    #[test]
    fn method_to_command_copy_files() {
        let method = DbusMethod::CopyFiles {
            sources: vec!["/tmp/a.txt".to_string(), "/tmp/b.txt".to_string()],
            dest: "/home/user".to_string(),
        };
        let cmd = method_to_command(&method).expect("should produce a command");
        match cmd {
            AppCommand::CopyFiles {
                sources,
                destination,
            } => {
                assert_eq!(sources.len(), 2);
                assert_eq!(sources[0], RavenPath::local(PathBuf::from("/tmp/a.txt")));
                assert_eq!(sources[1], RavenPath::local(PathBuf::from("/tmp/b.txt")));
                assert_eq!(destination, RavenPath::local(PathBuf::from("/home/user")));
            }
            other => panic!("expected CopyFiles, got {:?}", other),
        }
    }

    #[test]
    fn method_to_command_move_files() {
        let method = DbusMethod::MoveFiles {
            sources: vec!["/tmp/c.txt".to_string()],
            dest: "/home/user/dest".to_string(),
        };
        let cmd = method_to_command(&method).expect("should produce a command");
        match cmd {
            AppCommand::MoveFiles {
                sources,
                destination,
            } => {
                assert_eq!(sources.len(), 1);
                assert_eq!(sources[0], RavenPath::local(PathBuf::from("/tmp/c.txt")));
                assert_eq!(
                    destination,
                    RavenPath::local(PathBuf::from("/home/user/dest"))
                );
            }
            other => panic!("expected MoveFiles, got {:?}", other),
        }
    }

    #[test]
    fn method_to_command_delete_files() {
        let method = DbusMethod::DeleteFiles {
            paths: vec!["/tmp/del.txt".to_string()],
        };
        let cmd = method_to_command(&method).expect("should produce a command");
        match cmd {
            AppCommand::DeleteFiles { paths } => {
                assert_eq!(paths.len(), 1);
                assert_eq!(paths[0], RavenPath::local(PathBuf::from("/tmp/del.txt")));
            }
            other => panic!("expected DeleteFiles, got {:?}", other),
        }
    }

    #[test]
    fn method_to_command_trash_files() {
        let method = DbusMethod::TrashFiles {
            paths: vec!["/tmp/trash_me.txt".to_string()],
        };
        let cmd = method_to_command(&method).expect("should produce a command");
        match cmd {
            AppCommand::TrashFiles { paths } => {
                assert_eq!(paths.len(), 1);
                assert_eq!(
                    paths[0],
                    RavenPath::local(PathBuf::from("/tmp/trash_me.txt"))
                );
            }
            other => panic!("expected TrashFiles, got {:?}", other),
        }
    }

    #[test]
    fn method_to_command_search() {
        let method = DbusMethod::Search {
            query: "*.rs".to_string(),
            path: "/home/user/code".to_string(),
        };
        let cmd = method_to_command(&method).expect("should produce a command");
        match cmd {
            AppCommand::Search {
                query,
                path,
                search_mode,
            } => {
                assert_eq!(query, "*.rs");
                assert_eq!(
                    path,
                    RavenPath::local(PathBuf::from("/home/user/code"))
                );
                assert_eq!(search_mode, SearchMode::Filename);
            }
            other => panic!("expected Search, got {:?}", other),
        }
    }

    #[test]
    fn method_to_command_trigger_automation_rule() {
        let method = DbusMethod::TriggerAutomationRule {
            rule_id: "rule-42".to_string(),
        };
        let cmd = method_to_command(&method).expect("should produce a command");
        match cmd {
            AppCommand::TriggerAutomationRule { rule_id } => {
                assert_eq!(rule_id, "rule-42");
            }
            other => panic!("expected TriggerAutomationRule, got {:?}", other),
        }
    }

    #[test]
    fn method_to_command_returns_none_for_get_current_path() {
        let method = DbusMethod::GetCurrentPath;
        assert!(method_to_command(&method).is_none());
    }

    #[test]
    fn method_to_command_returns_none_for_get_selection() {
        let method = DbusMethod::GetSelection;
        assert!(method_to_command(&method).is_none());
    }

    #[test]
    fn event_to_signal_directory_loaded() {
        let event = AppEvent::DirectoryLoaded {
            pane_id: 0,
            path: RavenPath::local(PathBuf::from("/home/user")),
            entries: vec![],
        };
        let signal = event_to_signal(&event).expect("should produce a signal");
        match signal {
            DbusSignal::DirectoryChanged { path } => {
                assert_eq!(path, "/home/user");
            }
            other => panic!("expected DirectoryChanged, got {:?}", other),
        }
    }

    #[test]
    fn event_to_signal_operation_completed() {
        let event = AppEvent::OperationCompleted {
            id: OperationId(123),
        };
        let signal = event_to_signal(&event).expect("should produce a signal");
        match signal {
            DbusSignal::OperationCompleted { op_id, kind } => {
                assert_eq!(op_id, "123");
                assert_eq!(kind, "completed");
            }
            other => panic!("expected OperationCompleted, got {:?}", other),
        }
    }

    #[test]
    fn event_to_signal_returns_none_for_unrelated_events() {
        let event = AppEvent::SearchCompleted { total_matches: 5 };
        assert!(event_to_signal(&event).is_none());

        let event = AppEvent::Notification {
            title: "test".to_string(),
            message: "test msg".to_string(),
            level: raven_core::events::NotificationLevel::Info,
        };
        assert!(event_to_signal(&event).is_none());
    }
}
