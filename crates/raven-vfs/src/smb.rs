//! SMB2/3 network shares through the pure-Rust `smb2` client, so browsing a
//! Windows or Samba share needs neither libsmbclient nor gvfs on the machine.
//!
//! One authenticated client is kept per server and every share opened on it
//! is cached, so browsing does not log in again for each listing. Credentials
//! are held in memory only, for as long as the server stays connected: they
//! are never written anywhere, and `forget_server` drops them.

use std::collections::HashMap;
use std::fmt;
use std::future::Future;
use std::ops::ControlFlow;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use tokio::sync::{Mutex, OwnedMutexGuard};
use tracing::debug;

use raven_core::entry::{EntryKind, EntryMetadata, FileEntry};
use raven_core::error::{RavenError, RavenResult};
use raven_core::path::RavenPath;
use raven_core::vfs::VirtualFileSystem;

use smb2::{ClientConfig, DirectoryEntry, ErrorKind, FileInfo, ReconnectEvent, SmbClient, Tree};

/// The port SMB listens on when a host does not name one.
const SMB_PORT: u16 = 445;

/// How long the TCP dial may take before the server counts as unreachable.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// Ceiling on the whole login (dial, negotiate, authenticate). A server that
/// accepts the socket and then never answers would otherwise leave the
/// Connect dialog spinning; individual requests are bounded by the client's
/// own 30 second response timeout.
const LOGIN_TIMEOUT: Duration = Duration::from_secs(30);

/// Who to log in as. An empty user is a guest login.
#[derive(Clone, Default, PartialEq, Eq)]
pub struct SmbCredentials {
    pub user: String,
    pub password: String,
    pub domain: String,
}

impl SmbCredentials {
    pub fn guest() -> Self {
        Self::default()
    }

    pub fn is_guest(&self) -> bool {
        self.user.is_empty()
    }
}

/// Written by hand so a password never reaches a log line.
impl fmt::Debug for SmbCredentials {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SmbCredentials")
            .field("user", &self.user)
            .field("password", &if self.password.is_empty() { "" } else { "<redacted>" })
            .field("domain", &self.domain)
            .finish()
    }
}

/// A parsed `smb://[domain;]user[:password]@host[:port]/share/path` URL, or
/// the UNC form `\\host\share\path`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SmbUrl {
    /// Host, with `:port` kept when one was given.
    pub host: String,
    /// Empty when the URL names only the server.
    pub share: String,
    /// Absolute path inside the share, `/` for its root.
    pub path: String,
    pub user: Option<String>,
    pub password: Option<String>,
    pub domain: Option<String>,
}

/// Read what a person typed or pasted as an SMB location.
pub fn parse_smb_url(input: &str) -> Result<SmbUrl, &'static str> {
    let trimmed = input.trim();
    let rest = if let Some(unc) = trimmed.strip_prefix("\\\\") {
        unc.replace('\\', "/")
    } else {
        let lower = trimmed.to_ascii_lowercase();
        if lower.starts_with("smb://") {
            trimmed["smb://".len()..].to_string()
        } else if lower.starts_with("cifs://") {
            trimmed["cifs://".len()..].to_string()
        } else if lower.contains("://") {
            return Err("Only smb:// addresses can be opened as a Windows share");
        } else {
            trimmed.to_string()
        }
    };

    let (authority, tail) = match rest.split_once('/') {
        Some((a, t)) => (a, t),
        None => (rest.as_str(), ""),
    };

    let (userinfo, host) = match authority.rsplit_once('@') {
        Some((u, h)) => (Some(u), h),
        None => (None, authority),
    };

    let (mut user, mut password, mut domain) = (None, None, None);
    if let Some(info) = userinfo {
        let (account, pass) = match info.split_once(':') {
            Some((a, p)) => (a, Some(percent_decode(p))),
            None => (info, None),
        };
        let (dom, name) = match account.split_once(';') {
            Some((d, n)) => (Some(percent_decode(d)), n),
            None => (None, account),
        };
        user = Some(percent_decode(name)).filter(|u| !u.is_empty());
        password = pass;
        domain = dom.filter(|d| !d.is_empty());
    }

    if host.is_empty() {
        return Err("A host is required");
    }
    if let Some(port) = host_port(host) {
        match port.parse::<u16>() {
            Ok(p) if p > 0 => {}
            _ => return Err("The port must be a number between 1 and 65535"),
        }
    }

    let mut segments = tail
        .split('/')
        .filter(|s| !s.is_empty() && *s != ".")
        .map(percent_decode);
    let share = segments.next().unwrap_or_default();
    let inner: Vec<String> = segments.collect();

    Ok(SmbUrl {
        host: host.to_string(),
        share,
        path: format!("/{}", inner.join("/")),
        user,
        password,
        domain,
    })
}

/// The `:port` part of a host, if it names one. A bare IPv6 address has
/// colons of its own and only counts as having a port in brackets.
fn host_port(host: &str) -> Option<&str> {
    if let Some(rest) = host.strip_prefix('[') {
        return rest.split_once("]:").map(|(_, p)| p);
    }
    match host.matches(':').count() {
        1 => host.rsplit_once(':').map(|(_, p)| p),
        _ => None,
    }
}

/// The `host:port` socket address to dial.
fn server_addr(host: &str) -> String {
    if host_port(host).is_some() {
        host.to_string()
    } else if host.contains(':') && !host.starts_with('[') {
        format!("[{}]:{}", host, SMB_PORT)
    } else {
        format!("{}:{}", host, SMB_PORT)
    }
}

/// Host names are case-insensitive, so one server is one connection however
/// it is spelled.
fn host_key(host: &str) -> String {
    host.to_ascii_lowercase()
}

/// Decode `%XX` escapes; anything malformed is kept as written.
fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let (Some(h), Some(l)) = (hex(bytes[i + 1]), hex(bytes[i + 2])) {
                out.push(h * 16 + l);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn hex(b: u8) -> Option<u8> {
    (b as char).to_digit(16).map(|d| d as u8)
}

/// Split a `RavenPath::Smb` into the share and the share-relative wire path
/// (no leading slash, `""` for the share root). A path under the server root
/// (empty share) carries the share as its first component, which is what
/// joining a name onto `smb://host/` produces.
fn split_location(share: &str, path: &str) -> (String, String) {
    let mut parts = path.split('/').filter(|s| !s.is_empty() && *s != ".");
    let share = if share.is_empty() {
        parts.next().unwrap_or_default().to_string()
    } else {
        share.to_string()
    };
    (share, parts.collect::<Vec<_>>().join("/"))
}

/// The `RavenPath::Smb` of `name` inside the share-relative directory `rel`.
fn child_path(host: &str, share: &str, rel: &str, name: &str) -> RavenPath {
    let path = if rel.is_empty() {
        format!("/{}", name)
    } else {
        format!("/{}/{}", rel, name)
    };
    RavenPath::Smb {
        host: host.to_string(),
        share: share.to_string(),
        path,
    }
}

fn filetime(t: smb2::pack::FileTime) -> Option<DateTime<Utc>> {
    if t.0 == 0 {
        return None;
    }
    t.to_system_time().map(DateTime::<Utc>::from)
}

/// SMB carries no POSIX modes; directories are shown as searchable and
/// files as plain read-write so the permission columns read sensibly.
fn smb_metadata(
    name: &str,
    is_dir: bool,
    size: u64,
    created: smb2::pack::FileTime,
    modified: smb2::pack::FileTime,
    accessed: Option<smb2::pack::FileTime>,
) -> EntryMetadata {
    EntryMetadata {
        size: if is_dir { 0 } else { size },
        modified: filetime(modified),
        accessed: accessed.and_then(filetime),
        created: filetime(created),
        permissions: if is_dir { 0o755 } else { 0o644 },
        is_hidden: name.starts_with('.'),
        ..EntryMetadata::default()
    }
}

fn kind_of(is_dir: bool) -> EntryKind {
    if is_dir {
        EntryKind::Directory
    } else {
        EntryKind::File
    }
}

/// A listing row as a `FileEntry` under `rel` in `share`.
fn entry_from_listing(host: &str, share: &str, rel: &str, e: &DirectoryEntry) -> FileEntry {
    let metadata = smb_metadata(&e.name, e.is_directory, e.size, e.created, e.modified, None);
    FileEntry::new(
        e.name.clone(),
        child_path(host, share, rel, &e.name),
        kind_of(e.is_directory),
        metadata,
    )
}

/// A stat result as a `FileEntry` for `path`.
fn entry_from_info(path: &RavenPath, name: &str, info: &FileInfo) -> FileEntry {
    let metadata = smb_metadata(
        name,
        info.is_directory,
        info.size,
        info.created,
        info.modified,
        Some(info.accessed),
    );
    FileEntry::new(name.to_string(), path.clone(), kind_of(info.is_directory), metadata)
}

/// A share on the server list, which browses as a folder.
fn share_entry(host: &str, name: &str) -> FileEntry {
    FileEntry::new(
        name.to_string(),
        RavenPath::Smb {
            host: host.to_string(),
            share: name.to_string(),
            path: "/".to_string(),
        },
        EntryKind::Directory,
        EntryMetadata {
            permissions: 0o755,
            ..EntryMetadata::default()
        },
    )
}

fn raven_path_buf(path: &RavenPath) -> PathBuf {
    PathBuf::from(path.to_string())
}

/// Turn an `smb2` error into what the rest of the app understands, with a
/// message that says what to do about it where there is something to do.
fn map_error(err: &smb2::Error, op: &str, path: &RavenPath) -> RavenError {
    match err.kind() {
        ErrorKind::NotFound => RavenError::NotFound {
            path: raven_path_buf(path),
        },
        ErrorKind::AccessDenied => RavenError::PermissionDenied {
            path: raven_path_buf(path),
        },
        ErrorKind::AlreadyExists => RavenError::AlreadyExists {
            path: raven_path_buf(path),
        },
        ErrorKind::NotADirectory => RavenError::NotADirectory {
            path: raven_path_buf(path),
        },
        ErrorKind::AuthRequired => RavenError::Network {
            message: format!(
                "SMB login was rejected while trying to {} {}: check the user name, password and domain",
                op, path
            ),
        },
        ErrorKind::SigningRequired => RavenError::Network {
            message: format!(
                "The server requires signed sessions, which a guest login cannot provide ({}): log in with an account",
                path
            ),
        },
        ErrorKind::TimedOut => RavenError::Network {
            message: format!("SMB server did not answer in time to {} {}", op, path),
        },
        ErrorKind::ConnectionLost | ErrorKind::SessionExpired => RavenError::Network {
            message: format!("Lost the SMB connection while trying to {} {}: {}", op, path, err),
        },
        ErrorKind::SharingViolation => RavenError::Conflict {
            message: format!("{} is in use by another program on the server", path),
        },
        ErrorKind::DiskFull => RavenError::Vfs {
            message: format!("The share is full; could not {} {}", op, path),
        },
        ErrorKind::InvalidName => RavenError::Vfs {
            message: format!("The server does not accept the name {}", path),
        },
        _ => RavenError::Network {
            message: format!("SMB {} failed for {}: {}", op, path, err),
        },
    }
}

/// Whether the in-share path `inner` is `outer` or lies under it. Compared
/// without case, the way Windows and default Samba shares resolve names.
fn is_same_or_inside(outer: &str, inner: &str) -> bool {
    let outer = outer.trim_matches('/').to_lowercase();
    let inner = inner.trim_matches('/').to_lowercase();
    inner == outer || outer.is_empty() || inner.starts_with(&format!("{}/", outer))
}

/// Errors after which the cached client is not worth another try: the next
/// operation logs in again from scratch instead.
fn connection_is_dead(err: &smb2::Error) -> bool {
    matches!(
        err.kind(),
        ErrorKind::ConnectionLost | ErrorKind::SessionExpired | ErrorKind::TimedOut
    )
}

/// One logged-in server and the shares opened on it.
struct ServerConn {
    client: SmbClient,
    /// Each tree with the connection generation it was opened in.
    trees: HashMap<String, (Tree, u64)>,
    /// Bumped whenever smb2 revives the connection on a fresh socket. A
    /// revive invalidates every tree connect of the old session, so a tree
    /// from an older generation must be opened again rather than reused
    /// (reusing it gets NETWORK_NAME_DELETED, which reads as a dead link).
    generation: Arc<AtomicU64>,
}

impl ServerConn {
    fn new(client: SmbClient) -> Self {
        let generation = Arc::new(AtomicU64::new(0));
        let bump = generation.clone();
        client.on_reconnect(Some(Arc::new(move |event| {
            if matches!(event, ReconnectEvent::Succeeded { .. }) {
                bump.fetch_add(1, Ordering::SeqCst);
            }
        })));
        Self {
            client,
            trees: HashMap::new(),
            generation,
        }
    }

    fn current_generation(&self) -> u64 {
        self.generation.load(Ordering::SeqCst)
    }

    async fn tree(&mut self, share: &str) -> smb2::Result<Tree> {
        let current = self.current_generation();
        if let Some((tree, generation)) = self.trees.get(share) {
            if *generation == current {
                return Ok(tree.clone());
            }
        }
        let tree = self.client.connect_share(share).await?;
        self.remember(share, tree.clone());
        Ok(tree)
    }

    /// Cache `tree` as current. Only called with a tree that just worked, so
    /// it belongs to the live session even when a request revived it.
    fn remember(&mut self, share: &str, tree: Tree) {
        let current = self.current_generation();
        self.trees.insert(share.to_string(), (tree, current));
    }
}

type Progress<'a> = Option<&'a (dyn Fn(u64, u64) + Send + Sync)>;
type BoxedOp<'a> = Pin<Box<dyn Future<Output = RavenResult<()>> + Send + 'a>>;

/// SMB filesystem backend. Paths name the server they live on, so a single
/// `SmbFs` serves every connected server.
pub struct SmbFs {
    /// Keyed by `host_key`. Never persisted.
    credentials: std::sync::Mutex<HashMap<String, SmbCredentials>>,
    servers: Mutex<HashMap<String, Arc<Mutex<ServerConn>>>>,
}

impl SmbFs {
    pub fn new() -> Self {
        Self {
            credentials: std::sync::Mutex::new(HashMap::new()),
            servers: Mutex::new(HashMap::new()),
        }
    }

    /// Log in to `host` with `credentials`, replacing any earlier connection
    /// to it, and remember the credentials for reconnecting later.
    pub async fn connect(&self, host: &str, credentials: SmbCredentials) -> RavenResult<()> {
        let client = Self::login(host, &credentials).await?;
        // Log out the session being replaced instead of just dropping it.
        self.forget_server(host).await;
        let key = host_key(host);
        if let Ok(mut creds) = self.credentials.lock() {
            creds.insert(key.clone(), credentials);
        }
        let conn = Arc::new(Mutex::new(ServerConn::new(client)));
        self.servers.lock().await.insert(key, conn);
        Ok(())
    }

    /// Whether credentials for `host` are held, i.e. it was connected.
    pub fn is_connected(&self, host: &str) -> bool {
        self.credentials_for(host).is_some()
    }

    /// Record `host` as connected without a server, for tests elsewhere in
    /// the crate.
    #[cfg(test)]
    pub(crate) fn connect_for_test(&self, host: &str, credentials: SmbCredentials) {
        self.credentials
            .lock()
            .unwrap()
            .insert(host_key(host), credentials);
    }

    /// The credentials `host` is logged in with, if it is connected.
    pub fn credentials_for(&self, host: &str) -> Option<SmbCredentials> {
        self.credentials
            .lock()
            .ok()
            .and_then(|c| c.get(&host_key(host)).cloned())
    }

    /// Drop the connection to `host` and the credentials used for it.
    pub async fn forget_server(&self, host: &str) {
        let key = host_key(host);
        if let Ok(mut creds) = self.credentials.lock() {
            creds.remove(&key);
        }
        let conn = self.servers.lock().await.remove(&key);
        if let Some(conn) = conn {
            let mut conn = conn.lock().await;
            let trees: Vec<Tree> = conn.trees.drain().map(|(_, (t, _))| t).collect();
            for tree in trees {
                let _ = conn.client.disconnect_share(&tree).await;
            }
            debug!("SMB connection to {} closed", host);
        }
    }

    /// The disk shares `host` offers, by name.
    pub async fn list_shares(&self, host: &str) -> RavenResult<Vec<String>> {
        let root = RavenPath::Smb {
            host: host.to_string(),
            share: String::new(),
            path: "/".to_string(),
        };
        let server = self.server(host).await?;
        let mut conn = server.lock_owned().await;
        match conn.client.list_shares().await {
            Ok(shares) => Ok(shares
                .into_iter()
                .map(|s| s.name)
                .filter(|n| !n.ends_with('$'))
                .collect()),
            Err(e) => {
                drop(conn);
                Err(self.fail(host, &e, "list the shares of", &root).await)
            }
        }
    }

    async fn login(host: &str, credentials: &SmbCredentials) -> RavenResult<SmbClient> {
        let addr = server_addr(host);
        debug!("Connecting to SMB server at {} as {:?}", addr, credentials);
        let config = ClientConfig {
            addr: addr.clone(),
            timeout: CONNECT_TIMEOUT,
            username: credentials.user.clone(),
            password: credentials.password.clone(),
            domain: credentials.domain.clone(),
            auto_reconnect: true,
            compression: true,
            dfs_enabled: true,
            dfs_target_overrides: HashMap::new(),
        };
        let who = if credentials.is_guest() {
            "as guest".to_string()
        } else {
            format!("as {}", credentials.user)
        };
        match tokio::time::timeout(LOGIN_TIMEOUT, SmbClient::connect(config)).await {
            Err(_) => Err(RavenError::Network {
                message: format!("SMB server {} did not finish logging in within {} seconds", host, LOGIN_TIMEOUT.as_secs()),
            }),
            Ok(Ok(client)) => Ok(client),
            Ok(Err(e)) => Err(RavenError::Network {
                message: match e.kind() {
                    ErrorKind::AuthRequired if credentials.is_guest() => format!(
                        "SMB server {} does not allow guest access: log in with a user name and password",
                        host
                    ),
                    ErrorKind::AuthRequired | ErrorKind::AccessDenied => format!(
                        "SMB login to {} {} was rejected: check the user name, password and domain",
                        host, who
                    ),
                    ErrorKind::SigningRequired => format!(
                        "SMB server {} requires signed sessions, which a guest login cannot provide: log in with an account",
                        host
                    ),
                    ErrorKind::TimedOut => format!("SMB server {} did not answer in time", host),
                    _ => format!("Could not connect to SMB server {} ({}): {}", host, addr, e),
                },
            }),
        }
    }

    /// The logged-in client for `host`, logging in again with the stored
    /// credentials when the earlier connection was dropped.
    async fn server(&self, host: &str) -> RavenResult<Arc<Mutex<ServerConn>>> {
        let key = host_key(host);
        if let Some(conn) = self.servers.lock().await.get(&key) {
            return Ok(conn.clone());
        }
        let credentials = self
            .credentials
            .lock()
            .ok()
            .and_then(|c| c.get(&key).cloned())
            .ok_or_else(|| RavenError::Network {
                message: format!(
                    "Not connected to SMB server {}: connect to it with Connect to Server first",
                    host
                ),
            })?;

        // Log in without holding the map, so a slow server does not stall
        // browsing on the others.
        let client = Self::login(host, &credentials).await?;
        let mut servers = self.servers.lock().await;
        let conn = servers
            .entry(key)
            .or_insert_with(|| Arc::new(Mutex::new(ServerConn::new(client))))
            .clone();
        Ok(conn)
    }

    /// Lock the connection to `host` and open `share` on it.
    async fn open(
        &self,
        host: &str,
        share: &str,
        path: &RavenPath,
    ) -> RavenResult<(OwnedMutexGuard<ServerConn>, Tree)> {
        let server = self.server(host).await?;
        let mut conn = server.lock_owned().await;
        match conn.tree(share).await {
            Ok(tree) => Ok((conn, tree)),
            Err(e) => {
                drop(conn);
                Err(self.fail(host, &e, "open", path).await)
            }
        }
    }

    /// Record the outcome of one request: keep a tree the client updated (a
    /// DFS redirect rewrites it), and forget a connection that died.
    async fn finish<T>(
        &self,
        host: &str,
        share: &str,
        mut conn: OwnedMutexGuard<ServerConn>,
        tree: Tree,
        result: smb2::Result<T>,
        op: &str,
        path: &RavenPath,
    ) -> RavenResult<T> {
        match result {
            Ok(value) => {
                conn.remember(share, tree);
                Ok(value)
            }
            Err(e) => {
                drop(conn);
                Err(self.fail(host, &e, op, path).await)
            }
        }
    }

    async fn fail(&self, host: &str, err: &smb2::Error, op: &str, path: &RavenPath) -> RavenError {
        if connection_is_dead(err) {
            debug!("Dropping SMB connection to {} after: {}", host, err);
            self.servers.lock().await.remove(&host_key(host));
        }
        map_error(err, op, path)
    }

    fn parts(path: &RavenPath) -> RavenResult<(&str, String, String)> {
        match path {
            RavenPath::Smb { host, share, path } => {
                let (share, rel) = split_location(share, path);
                Ok((host.as_str(), share, rel))
            }
            other => Err(RavenError::UnsupportedProtocol {
                protocol: other.to_string(),
            }),
        }
    }

    /// Like `parts`, for operations that need a file or folder inside a share.
    fn parts_in_share<'p>(path: &'p RavenPath, op: &str) -> RavenResult<(&'p str, String, String)> {
        let (host, share, rel) = Self::parts(path)?;
        if share.is_empty() || rel.is_empty() {
            return Err(RavenError::Vfs {
                message: format!(
                    "Cannot {} {}: it is a server or share, not an item inside a share",
                    op, path
                ),
            });
        }
        Ok((host, share, rel))
    }

    async fn stat_info(&self, path: &RavenPath) -> RavenResult<FileInfo> {
        let (host, share, rel) = Self::parts(path)?;
        let (mut conn, mut tree) = self.open(host, &share, path).await?;
        let result = conn.client.stat(&mut tree, &rel).await;
        self.finish(host, &share, conn, tree, result, "read the details of", path)
            .await
    }

    async fn read_with_progress(&self, path: &RavenPath, progress: Progress<'_>) -> RavenResult<Vec<u8>> {
        let (host, share, rel) = Self::parts_in_share(path, "read")?;
        let (mut conn, mut tree) = self.open(host, &share, path).await?;
        let result = match progress {
            Some(cb) => {
                conn.client
                    .read_file_with_progress(&mut tree, &rel, |p| {
                        cb(p.bytes_transferred, p.total_bytes.unwrap_or(0));
                        ControlFlow::Continue(())
                    })
                    .await
            }
            None => conn.client.read_file_pipelined(&mut tree, &rel).await,
        };
        self.finish(host, &share, conn, tree, result, "read", path).await
    }

    async fn write_with_progress(
        &self,
        path: &RavenPath,
        contents: &[u8],
        progress: Progress<'_>,
    ) -> RavenResult<()> {
        let (host, share, rel) = Self::parts_in_share(path, "write")?;
        let (mut conn, mut tree) = self.open(host, &share, path).await?;
        let total = contents.len() as u64;
        let result = match progress {
            Some(cb) => {
                conn.client
                    .write_file_with_progress(&mut tree, &rel, contents, |p| {
                        cb(p.bytes_transferred, total);
                        ControlFlow::Continue(())
                    })
                    .await
            }
            None => conn.client.write_file_pipelined(&mut tree, &rel, contents).await,
        };
        self.finish(host, &share, conn, tree, result, "write", path)
            .await
            .map(|_| ())
    }

    /// Copy between two SMB locations, which may be on different shares or
    /// servers. The data passes through this machine: server-side copy only
    /// works within one server and is not worth the extra failure modes.
    fn copy_inner<'a>(
        &'a self,
        source: &'a RavenPath,
        destination: &'a RavenPath,
        progress: Progress<'a>,
    ) -> BoxedOp<'a> {
        Box::pin(async move {
            let info = self.stat_info(source).await?;
            if info.is_directory {
                // List before creating the target, so a target that ends up
                // inside the source is never part of what gets walked.
                let children = self.list_dir(source).await?;
                self.create_dir(destination).await?;
                for child in children {
                    let target = destination.join(&child.name);
                    self.copy_inner(&child.path, &target, progress).await?;
                }
                return Ok(());
            }
            let data = self.read_with_progress(source, None).await?;
            if let Some(cb) = progress {
                cb(0, data.len() as u64);
            }
            self.write_with_progress(destination, &data, progress).await?;
            if let Some(cb) = progress {
                cb(data.len() as u64, data.len() as u64);
            }
            Ok(())
        })
    }

    /// SMB only removes empty directories, so empty them first.
    fn delete_inner<'a>(&'a self, path: &'a RavenPath) -> BoxedOp<'a> {
        Box::pin(async move {
            let (host, share, rel) = Self::parts_in_share(path, "delete")?;
            let info = self.stat_info(path).await?;
            if info.is_directory {
                for child in self.list_dir(path).await? {
                    self.delete_inner(&child.path).await?;
                }
                let (mut conn, mut tree) = self.open(host, &share, path).await?;
                let result = conn.client.delete_directory(&mut tree, &rel).await;
                self.finish(host, &share, conn, tree, result, "delete", path).await
            } else {
                let (mut conn, mut tree) = self.open(host, &share, path).await?;
                let result = conn.client.delete_file(&mut tree, &rel).await;
                self.finish(host, &share, conn, tree, result, "delete", path).await
            }
        })
    }
}

impl Default for SmbFs {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl VirtualFileSystem for SmbFs {
    async fn list_dir(&self, path: &RavenPath) -> RavenResult<Vec<FileEntry>> {
        let (host, share, rel) = Self::parts(path)?;
        if share.is_empty() {
            let mut shares = self.list_shares(host).await?;
            shares.sort_by_key(|s| s.to_lowercase());
            return Ok(shares.iter().map(|s| share_entry(host, s)).collect());
        }

        let (mut conn, mut tree) = self.open(host, &share, path).await?;
        let result = conn.client.list_directory(&mut tree, &rel).await;
        let entries = self
            .finish(host, &share, conn, tree, result, "list", path)
            .await?;
        Ok(entries
            .iter()
            .filter(|e| e.name != "." && e.name != "..")
            .map(|e| entry_from_listing(host, &share, &rel, e))
            .collect())
    }

    async fn stat(&self, path: &RavenPath) -> RavenResult<FileEntry> {
        let (host, share, rel) = Self::parts(path)?;
        if share.is_empty() {
            // The server itself: reachable means it is there.
            self.server(host).await?;
            return Ok(FileEntry::new(
                host.to_string(),
                path.clone(),
                EntryKind::Directory,
                EntryMetadata {
                    permissions: 0o755,
                    ..EntryMetadata::default()
                },
            ));
        }
        if rel.is_empty() {
            // Opening the share proves it exists; the root has no name of
            // its own to stat on every server.
            let (conn, tree) = self.open(host, &share, path).await?;
            self.finish(host, &share, conn, tree, Ok(()), "open", path).await?;
            return Ok(share_entry(host, &share));
        }
        let info = self.stat_info(path).await?;
        let name = rel.rsplit('/').next().unwrap_or(&rel).to_string();
        Ok(entry_from_info(path, &name, &info))
    }

    async fn read(&self, path: &RavenPath) -> RavenResult<Vec<u8>> {
        self.read_with_progress(path, None).await
    }

    async fn write(&self, path: &RavenPath, contents: &[u8]) -> RavenResult<()> {
        self.write_with_progress(path, contents, None).await
    }

    /// Opened with the FILE_CREATE disposition, so the server refuses when
    /// the name is already taken instead of replacing that file.
    async fn write_new(&self, path: &RavenPath, contents: &[u8]) -> RavenResult<()> {
        let (host, share, rel) = Self::parts_in_share(path, "create")?;
        let (conn, tree) = self.open(host, &share, path).await?;
        let mut created = false;
        let result = async {
            let mut writer = conn.client.create_file_writer_exclusive(&tree, &rel).await?;
            created = true;
            writer.write_chunk(contents).await?;
            writer.finish().await
        }
        .await;
        let result = self
            .finish(host, &share, conn, tree, result, "create", path)
            .await
            .map(|_| ());
        if result.is_err() && created {
            // Only a file this call created is removed, never an existing one.
            let _ = self.delete_inner(path).await;
        }
        result
    }

    async fn copy(
        &self,
        source: &RavenPath,
        destination: &RavenPath,
        progress: Option<Box<dyn Fn(u64, u64) + Send + Sync>>,
    ) -> RavenResult<()> {
        let (host, share, rel) = Self::parts_in_share(source, "copy")?;
        let (dst_host, dst_share, dst_rel) = Self::parts_in_share(destination, "copy to")?;
        if host_key(host) == host_key(dst_host)
            && share.eq_ignore_ascii_case(&dst_share)
            && is_same_or_inside(&rel, &dst_rel)
        {
            // Copying a folder into itself would never finish walking.
            return Err(RavenError::Vfs {
                message: format!("Cannot copy {} into itself ({})", source, destination),
            });
        }
        self.copy_inner(source, destination, progress.as_deref()).await
    }

    async fn rename(&self, source: &RavenPath, destination: &RavenPath) -> RavenResult<()> {
        let (host, share, rel) = Self::parts_in_share(source, "rename")?;
        let (dst_host, dst_share, dst_rel) = Self::parts_in_share(destination, "rename to")?;
        // Returning an error rather than copying lets a move fall back to
        // copy and delete, which works across shares and servers.
        if host_key(host) != host_key(dst_host) || !share.eq_ignore_ascii_case(&dst_share) {
            return Err(RavenError::Vfs {
                message: format!(
                    "SMB can only rename within one share: {} -> {}",
                    source, destination
                ),
            });
        }
        let (mut conn, mut tree) = self.open(host, &share, source).await?;
        let result = conn.client.rename(&mut tree, &rel, &dst_rel).await;
        self.finish(host, &share, conn, tree, result, "rename", source)
            .await
    }

    async fn delete(&self, path: &RavenPath) -> RavenResult<()> {
        self.delete_inner(path).await
    }

    /// Creates missing parents too and accepts a folder that is already
    /// there, the way the local backend does, so copying a folder onto an
    /// existing one merges into it.
    async fn create_dir(&self, path: &RavenPath) -> RavenResult<()> {
        let (host, share, rel) = Self::parts_in_share(path, "create")?;
        let mut built = String::new();
        for component in rel.split('/') {
            if !built.is_empty() {
                built.push('/');
            }
            built.push_str(component);
            let step = RavenPath::Smb {
                host: host.to_string(),
                share: share.clone(),
                path: format!("/{}", built),
            };
            let (mut conn, mut tree) = self.open(host, &share, &step).await?;
            let result = conn.client.create_directory(&mut tree, &built).await;
            match self
                .finish(host, &share, conn, tree, result, "create", &step)
                .await
            {
                Ok(()) => {}
                Err(RavenError::AlreadyExists { .. }) => {
                    if !self.stat_info(&step).await?.is_directory {
                        return Err(RavenError::NotADirectory {
                            path: raven_path_buf(&step),
                        });
                    }
                }
                Err(e) => return Err(e),
            }
        }
        Ok(())
    }

    async fn exists(&self, path: &RavenPath) -> RavenResult<bool> {
        match self.stat(path).await {
            Ok(_) => Ok(true),
            Err(RavenError::NotFound { .. }) => Ok(false),
            Err(e) => Err(e),
        }
    }

    fn supports(&self, path: &RavenPath) -> bool {
        matches!(path, RavenPath::Smb { .. })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn smb(host: &str, share: &str, path: &str) -> RavenPath {
        RavenPath::Smb {
            host: host.to_string(),
            share: share.to_string(),
            path: path.to_string(),
        }
    }

    #[test]
    fn parses_a_full_url() {
        let url = parse_smb_url("smb://WORK;alice:s%40cret@nas.local:1445/Media/Photos/2024/").unwrap();
        assert_eq!(url.host, "nas.local:1445");
        assert_eq!(url.share, "Media");
        assert_eq!(url.path, "/Photos/2024");
        assert_eq!(url.user.as_deref(), Some("alice"));
        assert_eq!(url.password.as_deref(), Some("s@cret"));
        assert_eq!(url.domain.as_deref(), Some("WORK"));
    }

    #[test]
    fn parses_server_only_and_bare_hosts() {
        let url = parse_smb_url("SMB://fileserver").unwrap();
        assert_eq!((url.host.as_str(), url.share.as_str(), url.path.as_str()), ("fileserver", "", "/"));
        assert_eq!(url.user, None);

        let bare = parse_smb_url("  192.168.1.10/public  ").unwrap();
        assert_eq!(bare.host, "192.168.1.10");
        assert_eq!(bare.share, "public");
        assert_eq!(bare.path, "/");
    }

    #[test]
    fn parses_unc_paths_and_escapes() {
        let url = parse_smb_url(r"\\server\Team Share\Q1 Reports\").unwrap();
        assert_eq!(url.host, "server");
        assert_eq!(url.share, "Team Share");
        assert_eq!(url.path, "/Q1 Reports");

        let escaped = parse_smb_url("smb://server/Team%20Share/a%2Fb").unwrap();
        assert_eq!(escaped.share, "Team Share");
        assert_eq!(escaped.path, "/a/b");
    }

    #[test]
    fn rejects_bad_urls() {
        assert!(parse_smb_url("").is_err());
        assert!(parse_smb_url("smb:///share").is_err());
        assert!(parse_smb_url("sftp://host/x").is_err());
        assert!(parse_smb_url("smb://host:0/share").is_err());
        assert!(parse_smb_url("smb://host:port/share").is_err());
    }

    #[test]
    fn percent_decode_keeps_malformed_escapes() {
        assert_eq!(percent_decode("100%"), "100%");
        assert_eq!(percent_decode("%zz"), "%zz");
        assert_eq!(percent_decode("a%20b"), "a b");
        assert_eq!(percent_decode("%C3%A9"), "é");
    }

    #[test]
    fn server_addresses_default_to_445() {
        assert_eq!(server_addr("nas"), "nas:445");
        assert_eq!(server_addr("nas:1445"), "nas:1445");
        assert_eq!(server_addr("fe80::1"), "[fe80::1]:445");
        assert_eq!(server_addr("[fe80::1]:1445"), "[fe80::1]:1445");
        assert_eq!(server_addr("[fe80::1]"), "[fe80::1]:445");
    }

    #[test]
    fn locations_split_into_share_and_wire_path() {
        assert_eq!(split_location("docs", "/"), ("docs".into(), "".into()));
        assert_eq!(split_location("docs", "/a/b/"), ("docs".into(), "a/b".into()));
        assert_eq!(split_location("docs", "a/./b"), ("docs".into(), "a/b".into()));
        // Joined onto the server root, the first component is the share.
        assert_eq!(split_location("", "/docs/a"), ("docs".into(), "a".into()));
        assert_eq!(split_location("", "/"), ("".into(), "".into()));
    }

    #[test]
    fn child_paths_are_absolute_in_the_share() {
        assert_eq!(child_path("h", "s", "", "a.txt"), smb("h", "s", "/a.txt"));
        assert_eq!(child_path("h", "s", "x/y", "a.txt"), smb("h", "s", "/x/y/a.txt"));
    }

    #[test]
    fn listing_rows_become_entries() {
        // 2024-01-01T00:00:00Z as a FILETIME.
        let jan_2024 = smb2::pack::FileTime(133_485_408_000_000_000);
        let file = DirectoryEntry {
            name: "report.pdf".into(),
            size: 4096,
            is_directory: false,
            created: jan_2024,
            modified: jan_2024,
        };
        let entry = entry_from_listing("nas", "docs", "2024", &file);
        assert_eq!(entry.name, "report.pdf");
        assert_eq!(entry.path, smb("nas", "docs", "/2024/report.pdf"));
        assert!(entry.is_file());
        assert_eq!(entry.metadata.size, 4096);
        assert_eq!(entry.metadata.permissions, 0o644);
        assert_eq!(
            entry.metadata.modified.map(|t| t.to_rfc3339()),
            Some("2024-01-01T00:00:00+00:00".to_string())
        );
        assert!(!entry.is_hidden());

        let dir = DirectoryEntry {
            name: ".config".into(),
            size: 123,
            is_directory: true,
            created: smb2::pack::FileTime::ZERO,
            modified: smb2::pack::FileTime::ZERO,
        };
        let entry = entry_from_listing("nas", "docs", "", &dir);
        assert!(entry.is_dir());
        assert!(entry.is_hidden());
        assert_eq!(entry.metadata.size, 0);
        assert_eq!(entry.metadata.modified, None);
        assert_eq!(entry.path, smb("nas", "docs", "/.config"));
    }

    #[test]
    fn stat_info_becomes_an_entry() {
        let t = smb2::pack::FileTime(133_485_408_000_000_000);
        let info = FileInfo {
            size: 10,
            is_directory: false,
            created: t,
            modified: t,
            accessed: t,
        };
        let path = smb("nas", "docs", "/a.txt");
        let entry = entry_from_info(&path, "a.txt", &info);
        assert_eq!(entry.path, path);
        assert!(entry.metadata.accessed.is_some());
        assert!(entry.metadata.created.is_some());
    }

    #[test]
    fn shares_browse_as_folders() {
        let entry = share_entry("nas", "Media");
        assert!(entry.is_dir());
        assert_eq!(entry.path, smb("nas", "Media", "/"));
    }

    #[test]
    fn errors_map_to_raven_errors() {
        let path = smb("nas", "docs", "/a.txt");
        assert!(matches!(
            map_error(&smb2::Error::Timeout, "read", &path),
            RavenError::Network { .. }
        ));
        assert!(connection_is_dead(&smb2::Error::Disconnected));
        assert!(!connection_is_dead(&smb2::Error::InvalidData { message: "x".into() }));
        let auth = map_error(&smb2::Error::Auth { message: "bad".into() }, "list", &path);
        assert!(auth.to_string().contains("check the user name"));
    }

    #[test]
    fn credentials_never_print_the_password() {
        let creds = SmbCredentials {
            user: "alice".into(),
            password: "hunter2".into(),
            domain: "WORK".into(),
        };
        let printed = format!("{:?}", creds);
        assert!(printed.contains("alice"));
        assert!(!printed.contains("hunter2"));
        assert!(SmbCredentials::guest().is_guest());
    }

    #[tokio::test]
    async fn unconnected_servers_say_how_to_connect() {
        let fs = SmbFs::new();
        let err = fs.list_dir(&smb("nas", "docs", "/")).await.unwrap_err();
        match err {
            RavenError::Network { message } => assert!(message.contains("Connect to Server")),
            other => panic!("expected a network error, got {:?}", other),
        }
        assert!(!fs.is_connected("nas"));
    }

    #[tokio::test]
    async fn share_roots_are_not_files() {
        let fs = SmbFs::new();
        assert!(matches!(
            fs.read(&smb("nas", "docs", "/")).await,
            Err(RavenError::Vfs { .. })
        ));
        assert!(matches!(
            fs.delete(&smb("nas", "", "/")).await,
            Err(RavenError::Vfs { .. })
        ));
    }

    #[tokio::test]
    async fn renames_across_shares_fall_back() {
        let fs = SmbFs::new();
        let err = fs
            .rename(&smb("nas", "a", "/x"), &smb("nas", "b", "/x"))
            .await
            .unwrap_err();
        assert!(matches!(err, RavenError::Vfs { .. }));
    }

    #[tokio::test]
    async fn supports_only_smb_paths() {
        let fs = SmbFs::new();
        assert!(fs.supports(&smb("server", "share", "/")));
        assert!(!fs.supports(&RavenPath::local("/tmp")));
        assert!(!fs.supports(&RavenPath::Sftp {
            host: "host".into(),
            port: 22,
            user: "user".into(),
            path: "/".into(),
        }));
    }

    #[tokio::test]
    async fn forgetting_drops_credentials() {
        let fs = SmbFs::new();
        fs.credentials
            .lock()
            .unwrap()
            .insert(host_key("NAS"), SmbCredentials::guest());
        assert!(fs.is_connected("nas"));
        fs.forget_server("Nas").await;
        assert!(!fs.is_connected("nas"));
    }

    #[test]
    fn nested_paths_are_detected_without_case() {
        assert!(is_same_or_inside("docs/A", "docs/A"));
        assert!(is_same_or_inside("docs/A", "docs/A/A"));
        assert!(is_same_or_inside("docs/a", "/DOCS/A/b/"));
        assert!(!is_same_or_inside("docs/A", "docs/AB"));
        assert!(!is_same_or_inside("docs/A", "docs"));
    }

    #[tokio::test]
    async fn copying_a_folder_into_itself_is_refused() {
        let fs = SmbFs::new();
        let err = fs
            .copy(&smb("nas", "docs", "/A"), &smb("NAS", "Docs", "/A/A"), None)
            .await
            .unwrap_err();
        assert!(err.to_string().contains("into itself"), "{}", err);
        // A sibling with a shared prefix is a normal copy, which here fails
        // only because no server is connected.
        let err = fs
            .copy(&smb("nas", "docs", "/A"), &smb("nas", "docs", "/AB"), None)
            .await
            .unwrap_err();
        assert!(!err.to_string().contains("into itself"), "{}", err);
    }
}
