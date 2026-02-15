use std::sync::Arc;

use tokio::sync::{mpsc, Mutex};

use raven_core::commands::AppCommand;
use raven_core::events::AppEvent;

use crate::interface::{event_to_signal, method_to_command, DbusMethod, DbusResult, DbusSignal};

/// Shared state for the DBus service.
#[derive(Debug)]
pub struct DbusState {
    pub current_path: String,
    pub selection: Vec<String>,
    pub pending_signals: Vec<DbusSignal>,
}

impl DbusState {
    pub fn new() -> Self {
        Self {
            current_path: "/".to_string(),
            selection: Vec::new(),
            pending_signals: Vec::new(),
        }
    }
}

impl Default for DbusState {
    fn default() -> Self {
        Self::new()
    }
}

/// The DBus service that bridges external calls to the app backend.
pub struct DbusService {
    command_tx: mpsc::UnboundedSender<AppCommand>,
    state: Arc<Mutex<DbusState>>,
    bus_name: String,
}

impl DbusService {
    pub fn new(command_tx: mpsc::UnboundedSender<AppCommand>, bus_name: String) -> Self {
        Self {
            command_tx,
            state: Arc::new(Mutex::new(DbusState::new())),
            bus_name,
        }
    }

    /// Get the bus name.
    pub fn bus_name(&self) -> &str {
        &self.bus_name
    }

    /// Get a reference to the shared state.
    pub fn state(&self) -> Arc<Mutex<DbusState>> {
        self.state.clone()
    }

    /// Handle a DBus method call.
    pub async fn handle_method(&self, method: DbusMethod) -> DbusResult {
        match &method {
            DbusMethod::GetCurrentPath => {
                let state = self.state.lock().await;
                DbusResult::Path(state.current_path.clone())
            }
            DbusMethod::GetSelection => {
                let state = self.state.lock().await;
                DbusResult::Paths(state.selection.clone())
            }
            other => {
                if let Some(command) = method_to_command(other) {
                    match self.command_tx.send(command) {
                        Ok(()) => DbusResult::Success,
                        Err(e) => DbusResult::Error(format!("Failed to send command: {}", e)),
                    }
                } else {
                    DbusResult::Error("Unknown method".to_string())
                }
            }
        }
    }

    /// Process an AppEvent -- update internal state and generate signals.
    pub async fn handle_event(&self, event: &AppEvent) {
        let mut state = self.state.lock().await;

        // Update state based on events
        match event {
            AppEvent::DirectoryLoaded { path, .. } => {
                state.current_path = path.to_string();
            }
            _ => {}
        }

        // Generate signals
        if let Some(signal) = event_to_signal(event) {
            state.pending_signals.push(signal);
        }
    }

    /// Drain pending signals.
    pub async fn drain_signals(&self) -> Vec<DbusSignal> {
        let mut state = self.state.lock().await;
        std::mem::take(&mut state.pending_signals)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use raven_core::operations::OperationId;
    use raven_core::path::RavenPath;
    use std::path::PathBuf;

    fn create_service() -> (DbusService, mpsc::UnboundedReceiver<AppCommand>) {
        let (tx, rx) = mpsc::unbounded_channel();
        let service = DbusService::new(tx, "org.raven.FileManager".to_string());
        (service, rx)
    }

    #[tokio::test]
    async fn handle_method_navigate_sends_command() {
        let (service, mut rx) = create_service();

        let result = service
            .handle_method(DbusMethod::Navigate {
                path: "/home/user".to_string(),
            })
            .await;

        assert!(matches!(result, DbusResult::Success));

        let cmd = rx.try_recv().expect("should have received a command");
        match cmd {
            AppCommand::Navigate { path, pane_id } => {
                assert_eq!(path, RavenPath::local(PathBuf::from("/home/user")));
                assert_eq!(pane_id, 0);
            }
            other => panic!("expected Navigate, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn handle_method_get_current_path_returns_state() {
        let (service, _rx) = create_service();

        // Default state should be "/"
        let result = service.handle_method(DbusMethod::GetCurrentPath).await;
        match result {
            DbusResult::Path(p) => assert_eq!(p, "/"),
            other => panic!("expected Path, got {:?}", other),
        }

        // Update state manually and check again
        {
            let mut state = service.state.lock().await;
            state.current_path = "/home/user/Documents".to_string();
        }

        let result = service.handle_method(DbusMethod::GetCurrentPath).await;
        match result {
            DbusResult::Path(p) => assert_eq!(p, "/home/user/Documents"),
            other => panic!("expected Path, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn handle_method_get_selection_returns_state() {
        let (service, _rx) = create_service();

        // Default state should be empty
        let result = service.handle_method(DbusMethod::GetSelection).await;
        match result {
            DbusResult::Paths(paths) => assert!(paths.is_empty()),
            other => panic!("expected Paths, got {:?}", other),
        }

        // Set some selection and check
        {
            let mut state = service.state.lock().await;
            state.selection = vec!["/tmp/a.txt".to_string(), "/tmp/b.txt".to_string()];
        }

        let result = service.handle_method(DbusMethod::GetSelection).await;
        match result {
            DbusResult::Paths(paths) => {
                assert_eq!(paths.len(), 2);
                assert_eq!(paths[0], "/tmp/a.txt");
                assert_eq!(paths[1], "/tmp/b.txt");
            }
            other => panic!("expected Paths, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn handle_method_copy_files_sends_correct_command() {
        let (service, mut rx) = create_service();

        let result = service
            .handle_method(DbusMethod::CopyFiles {
                sources: vec!["/tmp/a.txt".to_string(), "/tmp/b.txt".to_string()],
                dest: "/home/user/dest".to_string(),
            })
            .await;

        assert!(matches!(result, DbusResult::Success));

        let cmd = rx.try_recv().expect("should have received a command");
        match cmd {
            AppCommand::CopyFiles {
                sources,
                destination,
            } => {
                assert_eq!(sources.len(), 2);
                assert_eq!(sources[0], RavenPath::local(PathBuf::from("/tmp/a.txt")));
                assert_eq!(sources[1], RavenPath::local(PathBuf::from("/tmp/b.txt")));
                assert_eq!(
                    destination,
                    RavenPath::local(PathBuf::from("/home/user/dest"))
                );
            }
            other => panic!("expected CopyFiles, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn handle_method_move_files_sends_correct_command() {
        let (service, mut rx) = create_service();

        let result = service
            .handle_method(DbusMethod::MoveFiles {
                sources: vec!["/tmp/move_me.txt".to_string()],
                dest: "/home/user/target".to_string(),
            })
            .await;

        assert!(matches!(result, DbusResult::Success));

        let cmd = rx.try_recv().expect("should have received a command");
        match cmd {
            AppCommand::MoveFiles {
                sources,
                destination,
            } => {
                assert_eq!(sources.len(), 1);
                assert_eq!(
                    sources[0],
                    RavenPath::local(PathBuf::from("/tmp/move_me.txt"))
                );
                assert_eq!(
                    destination,
                    RavenPath::local(PathBuf::from("/home/user/target"))
                );
            }
            other => panic!("expected MoveFiles, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn handle_method_delete_files_sends_correct_command() {
        let (service, mut rx) = create_service();

        let result = service
            .handle_method(DbusMethod::DeleteFiles {
                paths: vec!["/tmp/delete_me.txt".to_string()],
            })
            .await;

        assert!(matches!(result, DbusResult::Success));

        let cmd = rx.try_recv().expect("should have received a command");
        match cmd {
            AppCommand::DeleteFiles { paths } => {
                assert_eq!(paths.len(), 1);
                assert_eq!(
                    paths[0],
                    RavenPath::local(PathBuf::from("/tmp/delete_me.txt"))
                );
            }
            other => panic!("expected DeleteFiles, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn handle_method_trash_files_sends_correct_command() {
        let (service, mut rx) = create_service();

        let result = service
            .handle_method(DbusMethod::TrashFiles {
                paths: vec!["/tmp/trash_me.txt".to_string()],
            })
            .await;

        assert!(matches!(result, DbusResult::Success));

        let cmd = rx.try_recv().expect("should have received a command");
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

    #[tokio::test]
    async fn handle_method_search_sends_correct_command() {
        let (service, mut rx) = create_service();

        let result = service
            .handle_method(DbusMethod::Search {
                query: "*.rs".to_string(),
                path: "/home/user/code".to_string(),
            })
            .await;

        assert!(matches!(result, DbusResult::Success));

        let cmd = rx.try_recv().expect("should have received a command");
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
                assert_eq!(search_mode, raven_core::commands::SearchMode::Filename);
            }
            other => panic!("expected Search, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn handle_method_trigger_automation_sends_correct_command() {
        let (service, mut rx) = create_service();

        let result = service
            .handle_method(DbusMethod::TriggerAutomationRule {
                rule_id: "rule-99".to_string(),
            })
            .await;

        assert!(matches!(result, DbusResult::Success));

        let cmd = rx.try_recv().expect("should have received a command");
        match cmd {
            AppCommand::TriggerAutomationRule { rule_id } => {
                assert_eq!(rule_id, "rule-99");
            }
            other => panic!("expected TriggerAutomationRule, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn handle_event_directory_loaded_updates_state() {
        let (service, _rx) = create_service();

        let event = AppEvent::DirectoryLoaded {
            pane_id: 0,
            path: RavenPath::local(PathBuf::from("/home/user/Music")),
            entries: vec![],
        };

        service.handle_event(&event).await;

        let state = service.state.lock().await;
        assert_eq!(state.current_path, "/home/user/Music");
    }

    #[tokio::test]
    async fn handle_event_directory_loaded_generates_signal() {
        let (service, _rx) = create_service();

        let event = AppEvent::DirectoryLoaded {
            pane_id: 0,
            path: RavenPath::local(PathBuf::from("/home/user/Videos")),
            entries: vec![],
        };

        service.handle_event(&event).await;

        let signals = service.drain_signals().await;
        assert_eq!(signals.len(), 1);
        match &signals[0] {
            DbusSignal::DirectoryChanged { path } => {
                assert_eq!(path, "/home/user/Videos");
            }
            other => panic!("expected DirectoryChanged, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn handle_event_operation_completed_generates_signal() {
        let (service, _rx) = create_service();

        let event = AppEvent::OperationCompleted {
            id: OperationId(42),
        };

        service.handle_event(&event).await;

        let signals = service.drain_signals().await;
        assert_eq!(signals.len(), 1);
        match &signals[0] {
            DbusSignal::OperationCompleted { op_id, kind } => {
                assert_eq!(op_id, "42");
                assert_eq!(kind, "completed");
            }
            other => panic!("expected OperationCompleted, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn drain_signals_returns_and_clears() {
        let (service, _rx) = create_service();

        // Generate two signals
        let event1 = AppEvent::DirectoryLoaded {
            pane_id: 0,
            path: RavenPath::local(PathBuf::from("/tmp")),
            entries: vec![],
        };
        let event2 = AppEvent::OperationCompleted {
            id: OperationId(7),
        };

        service.handle_event(&event1).await;
        service.handle_event(&event2).await;

        // First drain should return both signals
        let signals = service.drain_signals().await;
        assert_eq!(signals.len(), 2);

        // Second drain should return empty
        let signals = service.drain_signals().await;
        assert!(signals.is_empty());
    }

    #[tokio::test]
    async fn multiple_events_accumulate_signals() {
        let (service, _rx) = create_service();

        for i in 0..5 {
            let event = AppEvent::OperationCompleted {
                id: OperationId(i),
            };
            service.handle_event(&event).await;
        }

        let signals = service.drain_signals().await;
        assert_eq!(signals.len(), 5);

        for (i, signal) in signals.iter().enumerate() {
            match signal {
                DbusSignal::OperationCompleted { op_id, .. } => {
                    assert_eq!(op_id, &i.to_string());
                }
                other => panic!("expected OperationCompleted, got {:?}", other),
            }
        }
    }

    #[tokio::test]
    async fn handle_method_returns_error_when_channel_closed() {
        let (tx, rx) = mpsc::unbounded_channel();
        let service = DbusService::new(tx, "org.raven.FileManager".to_string());

        // Drop the receiver to close the channel
        drop(rx);

        let result = service
            .handle_method(DbusMethod::Navigate {
                path: "/tmp".to_string(),
            })
            .await;

        match result {
            DbusResult::Error(msg) => {
                assert!(msg.contains("Failed to send command"));
            }
            other => panic!("expected Error, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn bus_name_returns_configured_name() {
        let (service, _rx) = create_service();
        assert_eq!(service.bus_name(), "org.raven.FileManager");
    }

    #[tokio::test]
    async fn non_signal_events_do_not_accumulate() {
        let (service, _rx) = create_service();

        // SearchCompleted doesn't map to a signal
        let event = AppEvent::SearchCompleted { total_matches: 10 };
        service.handle_event(&event).await;

        let signals = service.drain_signals().await;
        assert!(signals.is_empty());
    }
}
