//! The `org.freedesktop.impl.portal.FileChooser` backend.
//!
//! Applications that go through xdg-desktop-portal for their open and save
//! dialogs -- Chromium-based browsers, Brave included, and anything built with
//! `GTK_USE_PORTAL=1` -- never talk to this interface directly. They call
//! `org.freedesktop.portal.FileChooser`, and the portal forwards the call to
//! whichever backend `portals.conf` names for the interface. Serving this name
//! is what lets that be Raven instead of the GTK file chooser.
//!
//! The bus side does no UI. Each call is parsed into a [`ChooserRequest`] and
//! handed over a channel to whoever shows the dialog (the `--portal` mode of
//! the binary); the method call stays pending until that side answers on the
//! request's reply channel. Parsing is kept free of connection types so it can
//! be tested without a session bus, in the style of the rest of this crate.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use tokio::sync::{mpsc::UnboundedSender, oneshot};
use zbus::zvariant::{ObjectPath, OwnedObjectPath, OwnedValue, Value};

/// Well-known name for the Raven portal backend.
pub const PORTAL_BUS_NAME: &str = "org.freedesktop.impl.portal.desktop.raven";

/// Object path every portal backend is served at.
pub const PORTAL_OBJECT_PATH: &str = "/org/freedesktop/portal/desktop";

/// Response codes defined by `org.freedesktop.impl.portal.Request`.
pub const RESPONSE_SUCCESS: u32 = 0;
pub const RESPONSE_CANCELLED: u32 = 1;
pub const RESPONSE_OTHER: u32 = 2;

/// Which of the interface's methods a request arrived on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChooserMode {
    /// Pick one or more existing files (or folders with `directory`).
    Open,
    /// Pick a single destination path for a new file.
    Save,
    /// Pick a folder into which several named files are saved.
    SaveFiles,
}

/// One pattern within a filter.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FilterRule {
    /// Shell glob such as `*.png`.
    Glob(String),
    /// MIME type such as `image/png` or `image/*`.
    Mime(String),
}

/// A named group of patterns, as shown in the dialog's filter selector.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileFilter {
    pub name: String,
    pub rules: Vec<FilterRule>,
}

/// Everything the dialog needs to know about one call.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ChooserOptions {
    pub accept_label: Option<String>,
    pub multiple: bool,
    pub directory: bool,
    pub filters: Vec<FileFilter>,
    pub current_filter: Option<FileFilter>,
    /// Suggested name for Save.
    pub current_name: Option<String>,
    /// Folder the requester wants the dialog to start in.
    pub current_folder: Option<PathBuf>,
    /// Existing file a Save is expected to replace.
    pub current_file: Option<PathBuf>,
    /// Names written by SaveFiles.
    pub files: Vec<String>,
}

/// What the dialog answers with when the user accepts.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChooserSelection {
    pub paths: Vec<PathBuf>,
    pub current_filter: Option<FileFilter>,
}

/// A request forwarded to the UI side.
#[derive(Debug)]
pub struct ChooserRequest {
    pub mode: ChooserMode,
    pub app_id: String,
    pub parent_window: String,
    pub title: String,
    pub options: ChooserOptions,
    /// `None` means cancelled.
    pub reply: oneshot::Sender<Option<ChooserSelection>>,
    /// Fires when the requester closes the request before the user answers.
    pub closed: oneshot::Receiver<()>,
}

fn string_opt(options: &HashMap<String, OwnedValue>, key: &str) -> Option<String> {
    match options.get(key).map(|v| &**v) {
        Some(Value::Str(s)) => Some(s.to_string()),
        _ => None,
    }
}

fn bool_opt(options: &HashMap<String, OwnedValue>, key: &str) -> bool {
    matches!(options.get(key).map(|v| &**v), Some(Value::Bool(true)))
}

/// Decode an `ay` holding a NUL-terminated path.
fn bytes_to_path(value: &Value<'_>) -> Option<PathBuf> {
    let bytes = <Vec<u8>>::try_from(value.try_clone().ok()?).ok()?;
    bytes_slice_to_path(&bytes)
}

fn bytes_slice_to_path(bytes: &[u8]) -> Option<PathBuf> {
    use std::os::unix::ffi::OsStrExt;
    let end = bytes.iter().position(|b| *b == 0).unwrap_or(bytes.len());
    if end == 0 {
        return None;
    }
    Some(PathBuf::from(std::ffi::OsStr::from_bytes(&bytes[..end])))
}

fn path_opt(options: &HashMap<String, OwnedValue>, key: &str) -> Option<PathBuf> {
    options.get(key).and_then(|v| bytes_to_path(v))
}

/// Decode an `(sa(us))` filter.
fn parse_filter(value: &Value<'_>) -> Option<FileFilter> {
    let (name, rules) = <(String, Vec<(u32, String)>)>::try_from(value.try_clone().ok()?).ok()?;
    let rules = rules
        .into_iter()
        .filter_map(|(kind, pattern)| match kind {
            0 => Some(FilterRule::Glob(pattern)),
            1 => Some(FilterRule::Mime(pattern)),
            _ => None,
        })
        .collect();
    Some(FileFilter { name, rules })
}

/// Parse the `a{sv}` options of any of the three methods.
///
/// Unknown keys and values of the wrong type are ignored rather than failing
/// the call: the requester still gets a dialog, only without that option.
pub fn parse_options(options: &HashMap<String, OwnedValue>) -> ChooserOptions {
    let filters = options
        .get("filters")
        .and_then(|v| match &**v {
            Value::Array(array) => Some(array.iter().filter_map(parse_filter).collect()),
            _ => None,
        })
        .unwrap_or_default();

    let files = options
        .get("files")
        .and_then(|v| match &**v {
            Value::Array(array) => Some(
                array
                    .iter()
                    .filter_map(bytes_to_path)
                    .map(|p| p.to_string_lossy().into_owned())
                    .collect(),
            ),
            _ => None,
        })
        .unwrap_or_default();

    ChooserOptions {
        accept_label: string_opt(options, "accept_label"),
        multiple: bool_opt(options, "multiple"),
        directory: bool_opt(options, "directory"),
        filters,
        current_filter: options.get("current_filter").and_then(|v| parse_filter(v)),
        current_name: string_opt(options, "current_name"),
        current_folder: path_opt(options, "current_folder"),
        current_file: path_opt(options, "current_file"),
        files,
    }
}

/// Encode a local path as a `file://` URI.
///
/// Everything outside the RFC 3986 unreserved set (plus `/`) is escaped, so a
/// name with spaces or non-ASCII bytes round-trips through
/// [`file_uri_to_path`].
pub fn path_to_file_uri(path: &Path) -> String {
    use std::os::unix::ffi::OsStrExt;
    let mut uri = String::from("file://");
    for &b in path.as_os_str().as_bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'/' | b'-' | b'_' | b'.' | b'~') {
            uri.push(b as char);
        } else {
            uri.push_str(&format!("%{:02X}", b));
        }
    }
    uri
}

/// Build the `a{sv}` results for an accepted selection.
///
/// For SaveFiles the selection is the destination folder, and the URIs are the
/// requested names inside it.
pub fn build_results(
    mode: ChooserMode,
    options: &ChooserOptions,
    selection: &ChooserSelection,
) -> HashMap<String, OwnedValue> {
    let paths: Vec<PathBuf> = match mode {
        ChooserMode::SaveFiles => match selection.paths.first() {
            Some(folder) => options.files.iter().map(|n| folder.join(n)).collect(),
            None => Vec::new(),
        },
        _ => selection.paths.clone(),
    };
    let uris: Vec<String> = paths.iter().map(|p| path_to_file_uri(p)).collect();

    let mut results = HashMap::new();
    results.insert(
        "uris".to_string(),
        OwnedValue::try_from(Value::from(uris)).expect("string array has no fds"),
    );
    if let Some(filter) = &selection.current_filter {
        let rules: Vec<(u32, String)> = filter
            .rules
            .iter()
            .map(|r| match r {
                FilterRule::Glob(p) => (0, p.clone()),
                FilterRule::Mime(p) => (1, p.clone()),
            })
            .collect();
        if let Ok(v) = OwnedValue::try_from(Value::from((filter.name.clone(), rules))) {
            results.insert("current_filter".to_string(), v);
        }
    }
    results
}

/// The `org.freedesktop.impl.portal.Request` object for one pending call.
struct Request {
    closed: std::sync::Mutex<Option<oneshot::Sender<()>>>,
}

#[zbus::interface(name = "org.freedesktop.impl.portal.Request")]
impl Request {
    async fn close(&self) {
        if let Some(tx) = self.closed.lock().ok().and_then(|mut g| g.take()) {
            let _ = tx.send(());
        }
    }
}

/// The object served at [`PORTAL_OBJECT_PATH`].
pub struct FileChooser {
    request_tx: UnboundedSender<ChooserRequest>,
}

impl FileChooser {
    pub fn new(request_tx: UnboundedSender<ChooserRequest>) -> Self {
        Self { request_tx }
    }

    #[allow(clippy::too_many_arguments)]
    async fn run(
        &self,
        server: &zbus::ObjectServer,
        mode: ChooserMode,
        handle: OwnedObjectPath,
        app_id: String,
        parent_window: String,
        title: String,
        options: HashMap<String, OwnedValue>,
    ) -> (u32, HashMap<String, OwnedValue>) {
        let options = parse_options(&options);
        let (closed_tx, closed_rx) = oneshot::channel();
        let (reply_tx, reply_rx) = oneshot::channel();

        // Registering the handle is what lets the requester cancel. A failure
        // here only loses cancellation, so the dialog is still shown.
        let registered = server
            .at(
                handle.as_ref(),
                Request {
                    closed: std::sync::Mutex::new(Some(closed_tx)),
                },
            )
            .await
            .unwrap_or(false);

        let request = ChooserRequest {
            mode,
            app_id,
            parent_window,
            title,
            options: options.clone(),
            reply: reply_tx,
            closed: closed_rx,
        };

        let outcome = if self.request_tx.send(request).is_err() {
            tracing::warn!("FileChooser request dropped: dialog side is gone");
            (RESPONSE_OTHER, HashMap::new())
        } else {
            match reply_rx.await {
                Ok(Some(selection)) => (RESPONSE_SUCCESS, build_results(mode, &options, &selection)),
                Ok(None) => (RESPONSE_CANCELLED, HashMap::new()),
                Err(_) => (RESPONSE_OTHER, HashMap::new()),
            }
        };

        if registered {
            let path: ObjectPath<'_> = handle.as_ref();
            let _ = server.remove::<Request, _>(path).await;
        }
        outcome
    }
}

#[zbus::interface(name = "org.freedesktop.impl.portal.FileChooser")]
impl FileChooser {
    #[allow(clippy::too_many_arguments)]
    async fn open_file(
        &self,
        #[zbus(object_server)] server: &zbus::ObjectServer,
        handle: OwnedObjectPath,
        app_id: String,
        parent_window: String,
        title: String,
        options: HashMap<String, OwnedValue>,
    ) -> (u32, HashMap<String, OwnedValue>) {
        self.run(server, ChooserMode::Open, handle, app_id, parent_window, title, options)
            .await
    }

    #[allow(clippy::too_many_arguments)]
    async fn save_file(
        &self,
        #[zbus(object_server)] server: &zbus::ObjectServer,
        handle: OwnedObjectPath,
        app_id: String,
        parent_window: String,
        title: String,
        options: HashMap<String, OwnedValue>,
    ) -> (u32, HashMap<String, OwnedValue>) {
        self.run(server, ChooserMode::Save, handle, app_id, parent_window, title, options)
            .await
    }

    #[allow(clippy::too_many_arguments)]
    async fn save_files(
        &self,
        #[zbus(object_server)] server: &zbus::ObjectServer,
        handle: OwnedObjectPath,
        app_id: String,
        parent_window: String,
        title: String,
        options: HashMap<String, OwnedValue>,
    ) -> (u32, HashMap<String, OwnedValue>) {
        self.run(server, ChooserMode::SaveFiles, handle, app_id, parent_window, title, options)
            .await
    }

    #[zbus(property, name = "version")]
    async fn version(&self) -> u32 {
        4
    }
}

/// Own [`PORTAL_BUS_NAME`] and serve the FileChooser at the portal path.
///
/// Unlike FileManager1 there is no queueing: a second backend process has
/// nothing to add, so the name is requested with zbus's default flags and the
/// call fails when another process already owns it.
pub async fn serve(request_tx: UnboundedSender<ChooserRequest>) -> zbus::Result<zbus::Connection> {
    zbus::connection::Builder::session()?
        .name(PORTAL_BUS_NAME)?
        .serve_at(PORTAL_OBJECT_PATH, FileChooser::new(request_tx))?
        .build()
        .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::uri::file_uri_to_path;

    fn owned(v: Value<'_>) -> OwnedValue {
        OwnedValue::try_from(v).unwrap()
    }

    fn nul(s: &str) -> Vec<u8> {
        let mut b = s.as_bytes().to_vec();
        b.push(0);
        b
    }

    #[test]
    fn parses_flags_labels_and_paths() {
        let mut o = HashMap::new();
        o.insert("multiple".into(), owned(Value::Bool(true)));
        o.insert("directory".into(), owned(Value::Bool(false)));
        o.insert("accept_label".into(), owned(Value::from("_Upload")));
        o.insert("current_name".into(), owned(Value::from("report.pdf")));
        o.insert("current_folder".into(), owned(Value::from(nul("/home/u/Downloads"))));
        let p = parse_options(&o);
        assert!(p.multiple);
        assert!(!p.directory);
        assert_eq!(p.accept_label.as_deref(), Some("_Upload"));
        assert_eq!(p.current_name.as_deref(), Some("report.pdf"));
        assert_eq!(p.current_folder, Some(PathBuf::from("/home/u/Downloads")));
        assert_eq!(p.current_file, None);
    }

    #[test]
    fn parses_filters_and_current_filter() {
        let images = ("Images".to_string(), vec![(0u32, "*.png".to_string()), (1, "image/jpeg".to_string())]);
        let all = ("All".to_string(), vec![(0u32, "*".to_string())]);
        let mut o = HashMap::new();
        o.insert("filters".into(), owned(Value::from(vec![images.clone(), all])));
        o.insert("current_filter".into(), owned(Value::from(images)));
        let p = parse_options(&o);
        assert_eq!(p.filters.len(), 2);
        assert_eq!(
            p.filters[0].rules,
            vec![FilterRule::Glob("*.png".into()), FilterRule::Mime("image/jpeg".into())]
        );
        assert_eq!(p.current_filter.as_ref().map(|f| f.name.as_str()), Some("Images"));
    }

    #[test]
    fn empty_or_mistyped_options_are_ignored() {
        let mut o = HashMap::new();
        o.insert("multiple".into(), owned(Value::from("yes")));
        o.insert("current_folder".into(), owned(Value::from(vec![0u8])));
        let p = parse_options(&o);
        assert_eq!(p, ChooserOptions::default());
    }

    #[test]
    fn uri_encoding_round_trips() {
        let path = Path::new("/home/u/Big Report #1 ü.pdf");
        let uri = path_to_file_uri(path);
        assert_eq!(uri, "file:///home/u/Big%20Report%20%231%20%C3%BC.pdf");
        assert_eq!(file_uri_to_path(&uri), Some(path.to_path_buf()));
    }

    #[test]
    fn save_files_results_join_names_onto_the_folder() {
        let options = ChooserOptions {
            files: vec!["a.txt".into(), "b c.txt".into()],
            ..Default::default()
        };
        let sel = ChooserSelection {
            paths: vec![PathBuf::from("/tmp/out")],
            current_filter: None,
        };
        let r = build_results(ChooserMode::SaveFiles, &options, &sel);
        let uris = <Vec<String>>::try_from(r["uris"].try_clone().unwrap()).unwrap();
        assert_eq!(uris, vec!["file:///tmp/out/a.txt", "file:///tmp/out/b%20c.txt"]);
        assert!(!r.contains_key("current_filter"));
    }

    #[test]
    fn results_carry_the_chosen_filter() {
        let filter = FileFilter {
            name: "Text".into(),
            rules: vec![FilterRule::Mime("text/plain".into())],
        };
        let sel = ChooserSelection {
            paths: vec![PathBuf::from("/a")],
            current_filter: Some(filter),
        };
        let r = build_results(ChooserMode::Open, &ChooserOptions::default(), &sel);
        let (name, rules) =
            <(String, Vec<(u32, String)>)>::try_from(r["current_filter"].try_clone().unwrap()).unwrap();
        assert_eq!(name, "Text");
        assert_eq!(rules, vec![(1, "text/plain".to_string())]);
    }
}
