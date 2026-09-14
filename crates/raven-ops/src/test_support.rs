//! An in-memory filesystem that stands in for a remote backend (SMB, SFTP)
//! in tests, with hooks to simulate the failures real servers produce.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Mutex;

use async_trait::async_trait;

use raven_core::entry::{EntryKind, EntryMetadata, FileEntry};
use raven_core::error::{RavenError, RavenResult};
use raven_core::path::RavenPath;
use raven_core::vfs::VirtualFileSystem;

/// A path on the fake remote share.
pub fn remote(path: &str) -> RavenPath {
    RavenPath::Smb {
        host: "nas".into(),
        share: "share".into(),
        path: path.into(),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Node {
    Dir,
    File(Vec<u8>),
}

type Nodes = HashMap<RavenPath, Node>;

#[derive(Default)]
pub struct MemFs {
    nodes: Mutex<Nodes>,
    /// Every rename into another folder fails, as it does across SMB
    /// shares or backends. Renames within one folder still work.
    pub rename_fails: AtomicBool,
    /// Writing a file with this name fails partway through a copy.
    pub fail_writes_named: Mutex<Option<String>>,
    /// Created just before the next rename runs, as if another client wrote
    /// it between the conflict check and the move.
    pub appears_on_rename: Mutex<Option<(RavenPath, Vec<u8>)>>,
}

fn buf(path: &RavenPath) -> PathBuf {
    PathBuf::from(path.to_string())
}

fn not_found(path: &RavenPath) -> RavenError {
    RavenError::NotFound { path: buf(path) }
}

fn is_within(path: &RavenPath, root: &RavenPath) -> bool {
    let mut current = Some(path.clone());
    while let Some(p) = current {
        if &p == root {
            return true;
        }
        current = p.parent();
    }
    false
}

impl MemFs {
    pub fn new() -> Self {
        let fs = Self::default();
        fs.nodes.lock().unwrap().insert(remote("/"), Node::Dir);
        fs
    }

    /// Create a folder (and its parents).
    pub fn dir(&self, path: &str) -> RavenPath {
        let p = remote(path);
        Self::make_dirs(&mut self.nodes.lock().unwrap(), &p).unwrap();
        p
    }

    /// Create a file (and its parent folders).
    pub fn file(&self, path: &str, data: &[u8]) -> RavenPath {
        let p = remote(path);
        let mut nodes = self.nodes.lock().unwrap();
        if let Some(parent) = p.parent() {
            Self::make_dirs(&mut nodes, &parent).unwrap();
        }
        nodes.insert(p.clone(), Node::File(data.to_vec()));
        p
    }

    pub fn node(&self, path: &RavenPath) -> Option<Node> {
        self.nodes.lock().unwrap().get(path).cloned()
    }

    /// The contents of a file, or `None` when there is no file there.
    pub fn contents(&self, path: &RavenPath) -> Option<Vec<u8>> {
        match self.node(path) {
            Some(Node::File(data)) => Some(data),
            _ => None,
        }
    }

    fn make_dirs(nodes: &mut Nodes, path: &RavenPath) -> RavenResult<()> {
        match nodes.get(path) {
            Some(Node::Dir) => return Ok(()),
            Some(Node::File(_)) => return Err(RavenError::NotADirectory { path: buf(path) }),
            None => {}
        }
        if let Some(parent) = path.parent() {
            Self::make_dirs(nodes, &parent)?;
        }
        nodes.insert(path.clone(), Node::Dir);
        Ok(())
    }

    fn children(nodes: &Nodes, dir: &RavenPath) -> Vec<RavenPath> {
        let mut children: Vec<RavenPath> = nodes
            .keys()
            .filter(|p| *p != dir && p.parent().as_ref() == Some(dir))
            .cloned()
            .collect();
        children.sort_by_key(|p| p.to_string());
        children
    }

    fn parent_is_dir(nodes: &Nodes, path: &RavenPath) -> bool {
        matches!(path.parent().and_then(|p| nodes.get(&p).cloned()), Some(Node::Dir))
    }

    fn write_file(&self, nodes: &mut Nodes, path: &RavenPath, data: Vec<u8>) -> RavenResult<()> {
        if let Some(name) = self.fail_writes_named.lock().unwrap().as_deref() {
            if path.file_name() == Some(name) {
                return Err(RavenError::Network {
                    message: format!("simulated failure writing {}", path),
                });
            }
        }
        if !Self::parent_is_dir(nodes, path) {
            return Err(not_found(path));
        }
        if let Some(Node::Dir) = nodes.get(path) {
            return Err(RavenError::Vfs {
                message: format!("{} is a folder", path),
            });
        }
        nodes.insert(path.clone(), Node::File(data));
        Ok(())
    }

    /// Copy like the real backends do: folders merge into existing folders,
    /// files replace existing files.
    fn copy_node(&self, nodes: &mut Nodes, src: &RavenPath, dst: &RavenPath) -> RavenResult<()> {
        match nodes.get(src).cloned() {
            None => Err(not_found(src)),
            Some(Node::File(data)) => self.write_file(nodes, dst, data),
            Some(Node::Dir) => {
                let children = Self::children(nodes, src);
                match nodes.get(dst) {
                    Some(Node::File(_)) => {
                        return Err(RavenError::NotADirectory { path: buf(dst) })
                    }
                    Some(Node::Dir) => {}
                    None => {
                        if !Self::parent_is_dir(nodes, dst) {
                            return Err(not_found(dst));
                        }
                        nodes.insert(dst.clone(), Node::Dir);
                    }
                }
                for child in children {
                    let name = child.file_name().unwrap_or_default().to_string();
                    self.copy_node(nodes, &child, &dst.join(&name))?;
                }
                Ok(())
            }
        }
    }

    fn entry(path: &RavenPath, node: &Node) -> FileEntry {
        let (kind, size) = match node {
            Node::Dir => (EntryKind::Directory, 0),
            Node::File(data) => (EntryKind::File, data.len() as u64),
        };
        FileEntry::new(
            path.file_name().unwrap_or("/").to_string(),
            path.clone(),
            kind,
            EntryMetadata {
                size,
                ..EntryMetadata::default()
            },
        )
    }
}

#[async_trait]
impl VirtualFileSystem for MemFs {
    async fn list_dir(&self, path: &RavenPath) -> RavenResult<Vec<FileEntry>> {
        let nodes = self.nodes.lock().unwrap();
        match nodes.get(path) {
            Some(Node::Dir) => Ok(Self::children(&nodes, path)
                .iter()
                .map(|c| Self::entry(c, &nodes[c]))
                .collect()),
            Some(Node::File(_)) => Err(RavenError::NotADirectory { path: buf(path) }),
            None => Err(not_found(path)),
        }
    }

    async fn stat(&self, path: &RavenPath) -> RavenResult<FileEntry> {
        let nodes = self.nodes.lock().unwrap();
        nodes
            .get(path)
            .map(|n| Self::entry(path, n))
            .ok_or_else(|| not_found(path))
    }

    async fn read(&self, path: &RavenPath) -> RavenResult<Vec<u8>> {
        match self.node(path) {
            Some(Node::File(data)) => Ok(data),
            Some(Node::Dir) => Err(RavenError::Vfs {
                message: format!("{} is a folder", path),
            }),
            None => Err(not_found(path)),
        }
    }

    async fn write(&self, path: &RavenPath, contents: &[u8]) -> RavenResult<()> {
        let mut nodes = self.nodes.lock().unwrap();
        self.write_file(&mut nodes, path, contents.to_vec())
    }

    async fn copy(
        &self,
        source: &RavenPath,
        destination: &RavenPath,
        _progress: Option<Box<dyn Fn(u64, u64) + Send + Sync>>,
    ) -> RavenResult<()> {
        let mut nodes = self.nodes.lock().unwrap();
        self.copy_node(&mut nodes, source, destination)
    }

    /// Never replaces an existing destination, like SFTP and SMB.
    async fn rename(&self, source: &RavenPath, destination: &RavenPath) -> RavenResult<()> {
        let racer = self.appears_on_rename.lock().unwrap().take();
        let mut nodes = self.nodes.lock().unwrap();
        if let Some((path, data)) = racer {
            nodes.insert(path, Node::File(data));
        }
        if self.rename_fails.load(Ordering::SeqCst) && source.parent() != destination.parent() {
            return Err(RavenError::Vfs {
                message: format!("simulated: cannot rename {} -> {}", source, destination),
            });
        }
        if !nodes.contains_key(source) {
            return Err(not_found(source));
        }
        if nodes.contains_key(destination) {
            return Err(RavenError::AlreadyExists {
                path: buf(destination),
            });
        }
        if !Self::parent_is_dir(&nodes, destination) {
            return Err(not_found(destination));
        }
        let moved: Vec<RavenPath> = nodes
            .keys()
            .filter(|p| is_within(p, source))
            .cloned()
            .collect();
        for old in moved {
            let node = nodes.remove(&old).unwrap();
            let mut names = Vec::new();
            let mut p = old.clone();
            while &p != source {
                names.push(p.file_name().unwrap_or_default().to_string());
                p = p.parent().unwrap();
            }
            let new = names
                .iter()
                .rev()
                .fold(destination.clone(), |acc, name| acc.join(name));
            nodes.insert(new, node);
        }
        Ok(())
    }

    async fn delete(&self, path: &RavenPath) -> RavenResult<()> {
        let mut nodes = self.nodes.lock().unwrap();
        if !nodes.contains_key(path) {
            return Err(not_found(path));
        }
        nodes.retain(|p, _| !is_within(p, path));
        Ok(())
    }

    async fn create_dir(&self, path: &RavenPath) -> RavenResult<()> {
        Self::make_dirs(&mut self.nodes.lock().unwrap(), path)
    }

    async fn exists(&self, path: &RavenPath) -> RavenResult<bool> {
        Ok(self.nodes.lock().unwrap().contains_key(path))
    }

    fn supports(&self, _path: &RavenPath) -> bool {
        true
    }
}
