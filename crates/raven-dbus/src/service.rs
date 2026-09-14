//! Raven's own scripting interface on the session bus.
//!
//! As with FileManager1, the logic -- which method becomes which command, what
//! a query answers -- lives in [`Dispatcher`] and is tested without a bus;
//! [`RavenObject`] is the thin zbus shim over it and [`DbusService::serve`]
//! owns the name.

use std::sync::{Arc, OnceLock, RwLock};

use tokio::sync::{mpsc, Mutex};
use zbus::object_server::SignalEmitter;

use raven_core::commands::AppCommand;
use raven_core::events::AppEvent;

use crate::interface::{
    event_to_signal, method_to_command, validate_method, DbusMethod, DbusResult, DbusSignal,
};

/// Interface name of [`RavenObject`]. Fixed rather than derived from the
/// configured bus name: the interface is the contract a script codes
/// against, the bus name only says where to find it.
pub const RAVEN_INTERFACE: &str = "com.ravenfilemanager.Raven1";

/// Signals kept while nothing is serving them. Without a bus they have no
/// reader but tests and `drain_signals`, so the queue must not grow for the
/// life of a window that has D-Bus turned off.
const MAX_PENDING_SIGNALS: usize = 256;

/// Shared state for the DBus service.
#[derive(Debug)]
pub struct DbusState {
    pub current_path: String,
    /// The selection reported when no shared selection was attached.
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

/// The object path for `bus_name`, by the usual convention of swapping dots
/// for slashes (`com.example.App` is served at `/com/example/App`).
///
/// Bus name elements may contain `-`, which an object path may not, so any
/// character outside `[A-Za-z0-9_]` becomes `_`. Empty elements are dropped.
pub fn object_path_for(bus_name: &str) -> String {
    let elements: Vec<String> = bus_name
        .split('.')
        .filter(|e| !e.is_empty())
        .map(|e| {
            e.chars()
                .map(|c| if c.is_ascii_alphanumeric() || c == '_' { c } else { '_' })
                .collect()
        })
        .collect();
    if elements.is_empty() {
        "/".to_string()
    } else {
        format!("/{}", elements.join("/"))
    }
}

/// Answers method calls. Clonable and free of the connection, so the served
/// object can own one without the connection ending up owning itself.
#[derive(Clone)]
pub struct Dispatcher {
    command_tx: mpsc::UnboundedSender<AppCommand>,
    state: Arc<Mutex<DbusState>>,
    /// The window's published selection, when attached.
    selection: Option<Arc<RwLock<Vec<String>>>>,
}

impl Dispatcher {
    /// Handle a DBus method call.
    pub async fn handle_method(&self, method: DbusMethod) -> DbusResult {
        if let Err(e) = validate_method(&method) {
            return DbusResult::Error(e);
        }
        match &method {
            DbusMethod::GetCurrentPath => {
                let state = self.state.lock().await;
                DbusResult::Path(state.current_path.clone())
            }
            DbusMethod::GetSelection => match &self.selection {
                Some(shared) => DbusResult::Paths(
                    shared.read().map(|s| s.clone()).unwrap_or_else(|e| e.into_inner().clone()),
                ),
                None => {
                    let state = self.state.lock().await;
                    DbusResult::Paths(state.selection.clone())
                }
            },
            other => {
                if let Some(command) = method_to_command(other) {
                    match self.command_tx.send(command) {
                        Ok(()) => DbusResult::Success,
                        Err(e) => DbusResult::Failed(format!("Failed to send command: {}", e)),
                    }
                } else {
                    DbusResult::Failed("Unknown method".to_string())
                }
            }
        }
    }
}

/// The DBus service that bridges external calls to the app backend.
pub struct DbusService {
    dispatcher: Dispatcher,
    state: Arc<Mutex<DbusState>>,
    bus_name: String,
    /// Set once [`serve`](Self::serve) succeeds; signals go out on it from
    /// then on instead of queueing.
    connection: OnceLock<zbus::Connection>,
}

impl DbusService {
    pub fn new(command_tx: mpsc::UnboundedSender<AppCommand>, bus_name: String) -> Self {
        let state = Arc::new(Mutex::new(DbusState::new()));
        Self {
            dispatcher: Dispatcher {
                command_tx,
                state: state.clone(),
                selection: None,
            },
            state,
            bus_name,
            connection: OnceLock::new(),
        }
    }

    /// Answer `GetSelection` from the selection the window publishes rather
    /// than from [`DbusState::selection`], which nothing updates.
    pub fn with_selection(mut self, selection: Arc<RwLock<Vec<String>>>) -> Self {
        self.dispatcher.selection = Some(selection);
        self
    }

    /// Get the bus name.
    pub fn bus_name(&self) -> &str {
        &self.bus_name
    }

    /// The object path the interface is served at.
    pub fn object_path(&self) -> String {
        object_path_for(&self.bus_name)
    }

    /// Get a reference to the shared state.
    pub fn state(&self) -> Arc<Mutex<DbusState>> {
        self.state.clone()
    }

    /// Handle a DBus method call.
    pub async fn handle_method(&self, method: DbusMethod) -> DbusResult {
        self.dispatcher.handle_method(method).await
    }

    /// Process an AppEvent -- update internal state and generate signals.
    pub async fn handle_event(&self, event: &AppEvent) {
        let signal = {
            let mut state = self.state.lock().await;

            // Update state based on events
            if let AppEvent::DirectoryLoaded { path, .. } = event {
                state.current_path = path.to_string();
            }

            let Some(signal) = event_to_signal(event) else {
                return;
            };
            if self.connection.get().is_none() {
                if state.pending_signals.len() >= MAX_PENDING_SIGNALS {
                    state.pending_signals.remove(0);
                }
                state.pending_signals.push(signal);
                return;
            }
            signal
        };

        // Emitted outside the lock: a slow bus must not hold up method calls.
        if let Some(connection) = self.connection.get() {
            if let Err(e) = emit_signal(connection, &self.object_path(), &signal).await {
                tracing::debug!("Could not emit {:?} on {}: {}", signal, self.bus_name, e);
            }
        }
    }

    /// Drain pending signals.
    pub async fn drain_signals(&self) -> Vec<DbusSignal> {
        let mut state = self.state.lock().await;
        std::mem::take(&mut state.pending_signals)
    }

    /// Serve [`RAVEN_INTERFACE`] on the session bus under the configured bus
    /// name.
    ///
    /// The name is requested the way [`crate::fm1::serve`] requests
    /// FileManager1, and for the same reason: each window is its own process,
    /// so the first owns the name, the rest wait in the bus's queue, and the
    /// next in line takes over when the owner closes. The `bool` says whether
    /// this process owns it right now.
    ///
    /// The returned connection must be held for as long as the name should be
    /// owned. The service keeps a clone only to emit signals on; the served
    /// object holds the dispatcher, not the service, so there is no cycle
    /// keeping the connection alive after both are dropped.
    pub async fn serve(&self) -> zbus::Result<(zbus::Connection, bool)> {
        let path = self.object_path();
        let connection = zbus::connection::Builder::session()?
            .serve_at(
                path.as_str(),
                RavenObject {
                    dispatcher: self.dispatcher.clone(),
                },
            )?
            .build()
            .await?;
        // No flags: queue behind an existing owner instead of replacing it.
        let flags = enumflags2::BitFlags::<zbus::fdo::RequestNameFlags>::empty();
        let reply = connection
            .request_name_with_flags(self.bus_name.as_str(), flags)
            .await?;
        let owns = matches!(
            reply,
            zbus::fdo::RequestNameReply::PrimaryOwner | zbus::fdo::RequestNameReply::AlreadyOwner
        );
        // A second serve keeps the first connection for signals.
        let _ = self.connection.set(connection.clone());
        Ok((connection, owns))
    }
}

async fn emit_signal(
    connection: &zbus::Connection,
    path: &str,
    signal: &DbusSignal,
) -> zbus::Result<()> {
    let emitter = SignalEmitter::new(connection, path)?;
    match signal {
        DbusSignal::DirectoryChanged { path } => {
            RavenObject::directory_changed(&emitter, path).await
        }
        DbusSignal::OperationCompleted { op_id, kind } => {
            RavenObject::operation_completed(&emitter, op_id, kind).await
        }
        DbusSignal::FilesSelected { paths } => {
            RavenObject::files_selected(&emitter, paths.clone()).await
        }
    }
}

/// Map a dispatch result onto a method with no return value.
fn into_unit(result: DbusResult) -> zbus::fdo::Result<()> {
    match result {
        DbusResult::Success => Ok(()),
        // Only a rejected request is the caller's fault; a backend failure
        // must not read as InvalidArgs or clients will not retry.
        DbusResult::Error(e) => Err(zbus::fdo::Error::InvalidArgs(e)),
        DbusResult::Failed(e) => Err(zbus::fdo::Error::Failed(e)),
        other => Err(zbus::fdo::Error::Failed(format!("unexpected reply {:?}", other))),
    }
}

/// Map a dispatch result onto a method returning one path.
fn into_path(result: DbusResult) -> zbus::fdo::Result<String> {
    match result {
        DbusResult::Path(p) => Ok(p),
        DbusResult::Error(e) | DbusResult::Failed(e) => Err(zbus::fdo::Error::Failed(e)),
        other => Err(zbus::fdo::Error::Failed(format!("unexpected reply {:?}", other))),
    }
}

/// Map a dispatch result onto a method returning a list of paths.
fn into_paths(result: DbusResult) -> zbus::fdo::Result<Vec<String>> {
    match result {
        DbusResult::Paths(p) => Ok(p),
        DbusResult::Error(e) | DbusResult::Failed(e) => Err(zbus::fdo::Error::Failed(e)),
        other => Err(zbus::fdo::Error::Failed(format!("unexpected reply {:?}", other))),
    }
}

/// The object served at [`DbusService::object_path`].
pub struct RavenObject {
    dispatcher: Dispatcher,
}

#[zbus::interface(name = "com.ravenfilemanager.Raven1")]
impl RavenObject {
    async fn navigate(&self, path: String) -> zbus::fdo::Result<()> {
        into_unit(self.dispatcher.handle_method(DbusMethod::Navigate { path }).await)
    }

    async fn get_current_path(&self) -> zbus::fdo::Result<String> {
        into_path(self.dispatcher.handle_method(DbusMethod::GetCurrentPath).await)
    }

    async fn get_selection(&self) -> zbus::fdo::Result<Vec<String>> {
        into_paths(self.dispatcher.handle_method(DbusMethod::GetSelection).await)
    }

    async fn copy_files(&self, sources: Vec<String>, dest: String) -> zbus::fdo::Result<()> {
        into_unit(
            self.dispatcher
                .handle_method(DbusMethod::CopyFiles { sources, dest })
                .await,
        )
    }

    async fn move_files(&self, sources: Vec<String>, dest: String) -> zbus::fdo::Result<()> {
        into_unit(
            self.dispatcher
                .handle_method(DbusMethod::MoveFiles { sources, dest })
                .await,
        )
    }

    async fn delete_files(&self, paths: Vec<String>) -> zbus::fdo::Result<()> {
        into_unit(self.dispatcher.handle_method(DbusMethod::DeleteFiles { paths }).await)
    }

    async fn trash_files(&self, paths: Vec<String>) -> zbus::fdo::Result<()> {
        into_unit(self.dispatcher.handle_method(DbusMethod::TrashFiles { paths }).await)
    }

    async fn search(&self, query: String, path: String) -> zbus::fdo::Result<()> {
        into_unit(self.dispatcher.handle_method(DbusMethod::Search { query, path }).await)
    }

    async fn trigger_automation_rule(&self, rule_id: String) -> zbus::fdo::Result<()> {
        into_unit(
            self.dispatcher
                .handle_method(DbusMethod::TriggerAutomationRule { rule_id })
                .await,
        )
    }

    #[zbus(signal)]
    async fn directory_changed(emitter: &SignalEmitter<'_>, path: &str) -> zbus::Result<()>;

    #[zbus(signal)]
    async fn operation_completed(
        emitter: &SignalEmitter<'_>,
        op_id: &str,
        kind: &str,
    ) -> zbus::Result<()>;

    #[zbus(signal)]
    async fn files_selected(emitter: &SignalEmitter<'_>, paths: Vec<String>) -> zbus::Result<()>;
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

    #[test]
    fn interface_constant_matches_the_macro_name() {
        // The attribute needs a literal, so the two are kept in step by hand.
        use zbus::object_server::Interface;
        assert_eq!(RavenObject::name().as_str(), RAVEN_INTERFACE);
    }

    #[test]
    fn object_path_follows_the_bus_name() {
        assert_eq!(object_path_for("com.ravenfilemanager.Raven"), "/com/ravenfilemanager/Raven");
        assert_eq!(object_path_for("org.my-app.Files"), "/org/my_app/Files");
        assert_eq!(object_path_for("a..b."), "/a/b");
        assert_eq!(object_path_for(""), "/");
        // Every result must parse as an object path.
        for name in ["com.ravenfilemanager.Raven", "org.my-app.Files", "", "x.9y"] {
            assert!(zbus::zvariant::ObjectPath::try_from(object_path_for(name)).is_ok());
        }
    }

    #[test]
    fn result_mapping_for_the_bus() {
        assert!(into_unit(DbusResult::Success).is_ok());
        assert!(matches!(
            into_unit(DbusResult::Error("bad".into())),
            Err(zbus::fdo::Error::InvalidArgs(m)) if m == "bad"
        ));
        // A backend failure is not the caller's fault.
        assert!(matches!(
            into_unit(DbusResult::Failed("down".into())),
            Err(zbus::fdo::Error::Failed(m)) if m == "down"
        ));
        assert!(into_unit(DbusResult::Path("/".into())).is_err());
        assert_eq!(into_path(DbusResult::Path("/x".into())).unwrap(), "/x");
        assert!(into_path(DbusResult::Success).is_err());
        assert_eq!(
            into_paths(DbusResult::Paths(vec!["/a".into()])).unwrap(),
            vec!["/a".to_string()]
        );
        assert!(into_paths(DbusResult::Error("e".into())).is_err());
    }

    #[tokio::test]
    async fn object_methods_dispatch_to_the_backend() {
        let (service, mut rx) = create_service();
        let object = RavenObject {
            dispatcher: service.dispatcher.clone(),
        };

        object.navigate("/srv".into()).await.unwrap();
        assert!(matches!(rx.try_recv().unwrap(), AppCommand::Navigate { .. }));

        object
            .trash_files(vec!["/tmp/t".into()])
            .await
            .unwrap();
        assert!(matches!(rx.try_recv().unwrap(), AppCommand::TrashFiles { .. }));

        // A relative path is refused and nothing reaches the backend.
        assert!(object.delete_files(vec!["oops".into()]).await.is_err());
        assert!(rx.try_recv().is_err());

        assert_eq!(object.get_current_path().await.unwrap(), "/");
    }

    #[tokio::test]
    async fn get_selection_reads_the_attached_shared_selection() {
        let (tx, _rx) = mpsc::unbounded_channel();
        let shared = Arc::new(RwLock::new(vec!["/home/u/pic.png".to_string()]));
        let service =
            DbusService::new(tx, "org.raven.FileManager".to_string()).with_selection(shared.clone());

        // The unused fallback does not leak through.
        service.state.lock().await.selection = vec!["/stale".into()];

        match service.handle_method(DbusMethod::GetSelection).await {
            DbusResult::Paths(p) => assert_eq!(p, vec!["/home/u/pic.png".to_string()]),
            other => panic!("expected Paths, got {:?}", other),
        }

        *shared.write().unwrap() = Vec::new();
        match service.handle_method(DbusMethod::GetSelection).await {
            DbusResult::Paths(p) => assert!(p.is_empty()),
            other => panic!("expected Paths, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn pending_signals_are_capped_while_unserved() {
        let (service, _rx) = create_service();
        for i in 0..(MAX_PENDING_SIGNALS as u64 + 10) {
            service
                .handle_event(&AppEvent::OperationCompleted { id: OperationId(i) })
                .await;
        }
        let signals = service.drain_signals().await;
        assert_eq!(signals.len(), MAX_PENDING_SIGNALS);
        // The oldest were dropped, the newest kept.
        match signals.last().unwrap() {
            DbusSignal::OperationCompleted { op_id, .. } => {
                assert_eq!(op_id, &(MAX_PENDING_SIGNALS as u64 + 9).to_string())
            }
            other => panic!("unexpected {:?}", other),
        }
    }

    /// Talks to the real session bus, so it is not run by default:
    /// `cargo test -p raven-dbus -- --ignored serves_on_the_session_bus`.
    #[tokio::test(flavor = "multi_thread")]
    #[ignore]
    async fn serves_on_the_session_bus() {
        let bus_name = format!("com.ravenfilemanager.RavenTest.p{}", std::process::id());
        let (tx, mut rx) = mpsc::unbounded_channel();
        let shared = Arc::new(RwLock::new(vec!["/tmp/selected".to_string()]));
        let service = DbusService::new(tx, bus_name.clone()).with_selection(shared);

        let (_conn, owns) = service.serve().await.expect("serve");
        assert!(owns);

        // A second window queues rather than taking the name.
        let second = DbusService::new(mpsc::unbounded_channel().0, bus_name.clone());
        let (_conn2, owns2) = second.serve().await.expect("serve second");
        assert!(!owns2);

        let client = zbus::Connection::session().await.expect("client");
        let proxy = zbus::Proxy::new(
            &client,
            bus_name.as_str(),
            service.object_path(),
            RAVEN_INTERFACE,
        )
        .await
        .expect("proxy");

        let selection: Vec<String> = proxy.call("GetSelection", &()).await.expect("GetSelection");
        assert_eq!(selection, vec!["/tmp/selected".to_string()]);

        let () = proxy.call("Navigate", &("/tmp",)).await.expect("Navigate");
        assert!(matches!(rx.recv().await, Some(AppCommand::Navigate { .. })));

        let refused: zbus::Result<()> = proxy.call("Navigate", &("relative",)).await;
        assert!(refused.is_err());
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
            DbusResult::Failed(msg) => {
                assert!(msg.contains("Failed to send command"));
            }
            other => panic!("expected Failed, got {:?}", other),
        }
    }

    #[tokio::test]
    async fn bus_name_returns_configured_name() {
        let (service, _rx) = create_service();
        assert_eq!(service.bus_name(), "org.raven.FileManager");
        assert_eq!(service.object_path(), "/org/raven/FileManager");
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
