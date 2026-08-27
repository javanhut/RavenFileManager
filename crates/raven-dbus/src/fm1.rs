//! The `org.freedesktop.FileManager1` interface.
//!
//! This is the interface a desktop's "show this file in the file manager"
//! action looks for. Chromium-based browsers -- Brave included -- call
//! `ShowItems` for the "Show in folder" entry on a download and only fall back
//! to `xdg-open` on the containing directory when the call fails. The fallback
//! loses the two things that make the action useful: the browser can no longer
//! say *which* file it meant, so nothing is selected, and `xdg-open` spawns a
//! process rather than reaching an already-running window.
//!
//! The mapping from a request to an [`AppCommand`] is [`request_to_commands`],
//! kept free of any bus types so it can be tested without a session bus, in the
//! style of the rest of this crate. [`serve`] is the thin part that owns the
//! name and dispatches onto it.

use tokio::sync::mpsc::UnboundedSender;

use raven_core::commands::AppCommand;
use raven_core::path::RavenPath;

use crate::uri::file_uri_to_path;

/// Well-known name for the file manager interface.
pub const FM1_BUS_NAME: &str = "org.freedesktop.FileManager1";

/// Well-known object path for the file manager interface.
pub const FM1_OBJECT_PATH: &str = "/org/freedesktop/FileManager1";

/// Which of the three interface methods a call arrived on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShowRequest {
    /// Open the URIs as directories.
    Folders,
    /// Open the containing directory and select the URIs within it.
    Items,
    /// As `Items`, and raise the properties dialog for the first one.
    ItemProperties,
}

/// Translate a FileManager1 request into backend commands.
///
/// Returns an empty vector when nothing actionable survives -- no URI decoded
/// to a local path, or the caller named the filesystem root, which has no
/// containing directory to open. An empty vector is not an error: the spec
/// gives these methods no return value and no error to raise, so a request that
/// cannot be honoured is dropped rather than reported.
pub fn request_to_commands(
    request: ShowRequest,
    uris: &[String],
    pane_id: u32,
) -> Vec<AppCommand> {
    let paths: Vec<RavenPath> = uris
        .iter()
        .filter_map(|uri| file_uri_to_path(uri))
        .map(RavenPath::local)
        .collect();

    let Some(first) = paths.first() else {
        return Vec::new();
    };

    match request {
        // One pane shows one directory, so extra folders have nowhere to go.
        // Nautilus answers this by opening a window each; doing the same here
        // would mean deciding that a browser's "show in folder" may open an
        // unbounded number of windows, so the first URI wins and the rest are
        // dropped.
        ShowRequest::Folders => vec![AppCommand::Navigate {
            path: first.clone(),
            pane_id,
        }],

        ShowRequest::Items | ShowRequest::ItemProperties => {
            let Some(parent) = first.parent() else {
                return Vec::new();
            };
            // Only items from the directory actually being opened. Selecting a
            // path that is not in the listing would silently do nothing, so the
            // filter makes the outcome match what the caller can see.
            let selected: Vec<RavenPath> = paths
                .iter()
                .filter(|p| p.parent().as_ref() == Some(&parent))
                .cloned()
                .collect();

            vec![AppCommand::RevealItems {
                paths: selected,
                pane_id,
                show_properties: request == ShowRequest::ItemProperties,
            }]
        }
    }
}

/// The object served at [`FM1_OBJECT_PATH`].
pub struct FileManager1 {
    command_tx: UnboundedSender<AppCommand>,
    pane_id: u32,
}

impl FileManager1 {
    pub fn new(command_tx: UnboundedSender<AppCommand>, pane_id: u32) -> Self {
        Self {
            command_tx,
            pane_id,
        }
    }

    fn dispatch(&self, request: ShowRequest, uris: &[String]) {
        let commands = request_to_commands(request, uris, self.pane_id);
        if commands.is_empty() {
            tracing::warn!(
                ?request,
                ?uris,
                "FileManager1 request named nothing that could be opened"
            );
            return;
        }
        for command in commands {
            if self.command_tx.send(command).is_err() {
                // The backend is gone, which means the app is shutting down.
                tracing::warn!("FileManager1 request dropped: backend channel closed");
                return;
            }
        }
    }
}

// StartupId is accepted and ignored. Honouring it means handing the token to
// the compositor for focus stealing prevention; Huginn has no such protocol
// today, and the signature has to match regardless or the call fails to
// dispatch.
#[zbus::interface(name = "org.freedesktop.FileManager1")]
impl FileManager1 {
    async fn show_folders(&self, uris: Vec<String>, _startup_id: String) {
        self.dispatch(ShowRequest::Folders, &uris);
    }

    async fn show_items(&self, uris: Vec<String>, _startup_id: String) {
        self.dispatch(ShowRequest::Items, &uris);
    }

    async fn show_item_properties(&self, uris: Vec<String>, _startup_id: String) {
        self.dispatch(ShowRequest::ItemProperties, &uris);
    }
}

/// Claim [`FM1_BUS_NAME`] and serve the interface on the session bus.
///
/// The returned connection must be held for as long as the name should be
/// owned -- dropping it releases the name and the next `ShowItems` goes to
/// whatever claims it after us, or fails.
///
/// Acquiring the name fails if another file manager is already running and
/// holds it. That is a normal condition rather than a fault, so the error is
/// returned for the caller to log and carry on with.
pub async fn serve(
    command_tx: UnboundedSender<AppCommand>,
    pane_id: u32,
) -> zbus::Result<zbus::Connection> {
    zbus::connection::Builder::session()?
        .name(FM1_BUS_NAME)?
        .serve_at(FM1_OBJECT_PATH, FileManager1::new(command_tx, pane_id))?
        .build()
        .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn uris(list: &[&str]) -> Vec<String> {
        list.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn show_folders_navigates_to_the_folder() {
        let cmds = request_to_commands(
            ShowRequest::Folders,
            &uris(&["file:///home/user/Downloads"]),
            0,
        );
        assert_eq!(cmds.len(), 1);
        match &cmds[0] {
            AppCommand::Navigate { path, pane_id } => {
                assert_eq!(*path, RavenPath::local(PathBuf::from("/home/user/Downloads")));
                assert_eq!(*pane_id, 0);
            }
            other => panic!("expected Navigate, got {:?}", other),
        }
    }

    #[test]
    fn show_items_reveals_in_the_containing_directory() {
        let cmds = request_to_commands(
            ShowRequest::Items,
            &uris(&["file:///home/user/Downloads/report.pdf"]),
            3,
        );
        assert_eq!(cmds.len(), 1);
        match &cmds[0] {
            AppCommand::RevealItems {
                paths,
                pane_id,
                show_properties,
            } => {
                assert_eq!(paths.len(), 1);
                assert_eq!(
                    paths[0],
                    RavenPath::local(PathBuf::from("/home/user/Downloads/report.pdf"))
                );
                assert_eq!(*pane_id, 3);
                assert!(!show_properties);
            }
            other => panic!("expected RevealItems, got {:?}", other),
        }
    }

    #[test]
    fn show_items_decodes_percent_escapes() {
        let cmds = request_to_commands(
            ShowRequest::Items,
            &uris(&["file:///home/user/Downloads/Big%20Report.pdf"]),
            0,
        );
        match &cmds[0] {
            AppCommand::RevealItems { paths, .. } => {
                assert_eq!(
                    paths[0],
                    RavenPath::local(PathBuf::from("/home/user/Downloads/Big Report.pdf"))
                );
            }
            other => panic!("expected RevealItems, got {:?}", other),
        }
    }

    #[test]
    fn show_item_properties_sets_the_flag() {
        let cmds = request_to_commands(
            ShowRequest::ItemProperties,
            &uris(&["file:///tmp/a.txt"]),
            0,
        );
        match &cmds[0] {
            AppCommand::RevealItems {
                show_properties, ..
            } => assert!(show_properties),
            other => panic!("expected RevealItems, got {:?}", other),
        }
    }

    #[test]
    fn items_outside_the_first_parent_are_dropped() {
        // One pane shows one directory; selecting into another would not show.
        let cmds = request_to_commands(
            ShowRequest::Items,
            &uris(&[
                "file:///home/user/Downloads/a.txt",
                "file:///home/user/Downloads/b.txt",
                "file:///etc/hosts",
            ]),
            0,
        );
        match &cmds[0] {
            AppCommand::RevealItems { paths, .. } => {
                assert_eq!(paths.len(), 2);
                assert!(paths
                    .iter()
                    .all(|p| p.parent()
                        == Some(RavenPath::local(PathBuf::from("/home/user/Downloads")))));
            }
            other => panic!("expected RevealItems, got {:?}", other),
        }
    }

    #[test]
    fn undecodable_uris_are_skipped() {
        let cmds = request_to_commands(
            ShowRequest::Items,
            &uris(&["sftp://host/a", "file:///tmp/real.txt"]),
            0,
        );
        match &cmds[0] {
            AppCommand::RevealItems { paths, .. } => {
                assert_eq!(paths.len(), 1);
                assert_eq!(paths[0], RavenPath::local(PathBuf::from("/tmp/real.txt")));
            }
            other => panic!("expected RevealItems, got {:?}", other),
        }
    }

    #[test]
    fn no_usable_uri_yields_no_commands() {
        assert!(request_to_commands(ShowRequest::Items, &uris(&[]), 0).is_empty());
        assert!(request_to_commands(ShowRequest::Folders, &uris(&["https://x/"]), 0).is_empty());
        assert!(request_to_commands(ShowRequest::Items, &uris(&["trash:///"]), 0).is_empty());
    }

    #[test]
    fn root_has_no_containing_directory_to_reveal_in() {
        assert!(request_to_commands(ShowRequest::Items, &uris(&["file:///"]), 0).is_empty());
    }

    #[test]
    fn folders_takes_the_first_uri_only() {
        let cmds = request_to_commands(
            ShowRequest::Folders,
            &uris(&["file:///tmp/one", "file:///tmp/two"]),
            0,
        );
        assert_eq!(cmds.len(), 1);
        match &cmds[0] {
            AppCommand::Navigate { path, .. } => {
                assert_eq!(*path, RavenPath::local(PathBuf::from("/tmp/one")));
            }
            other => panic!("expected Navigate, got {:?}", other),
        }
    }

    #[test]
    fn dispatch_forwards_to_the_backend_channel() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let fm = FileManager1::new(tx, 0);
        fm.dispatch(ShowRequest::Items, &uris(&["file:///tmp/x/y.txt"]));

        let cmd = rx.try_recv().expect("should have received a command");
        assert!(matches!(cmd, AppCommand::RevealItems { .. }));
    }

    #[test]
    fn dispatch_of_an_unusable_request_sends_nothing() {
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let fm = FileManager1::new(tx, 0);
        fm.dispatch(ShowRequest::Items, &uris(&["https://example.com/"]));

        assert!(rx.try_recv().is_err());
    }
}
