use std::path::PathBuf;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::path::RavenPath;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum EntryKind {
    File,
    Directory,
    Symlink,
    BlockDevice,
    CharDevice,
    Fifo,
    Socket,
    Unknown,
}

impl EntryKind {
    pub fn is_dir(&self) -> bool {
        *self == Self::Directory
    }

    pub fn is_file(&self) -> bool {
        *self == Self::File
    }

    pub fn is_symlink(&self) -> bool {
        *self == Self::Symlink
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EntryMetadata {
    pub size: u64,
    pub modified: Option<DateTime<Utc>>,
    pub accessed: Option<DateTime<Utc>>,
    pub created: Option<DateTime<Utc>>,
    pub permissions: u32,
    pub owner_uid: u32,
    pub group_gid: u32,
    pub mime_type: Option<String>,
    pub symlink_target: Option<PathBuf>,
    pub is_hidden: bool,
    pub is_executable: bool,
}

impl Default for EntryMetadata {
    fn default() -> Self {
        Self {
            size: 0,
            modified: None,
            accessed: None,
            created: None,
            permissions: 0o644,
            owner_uid: 0,
            group_gid: 0,
            mime_type: None,
            symlink_target: None,
            is_hidden: false,
            is_executable: false,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileEntry {
    pub name: String,
    pub path: RavenPath,
    pub kind: EntryKind,
    pub metadata: EntryMetadata,
}

impl FileEntry {
    pub fn new(name: String, path: RavenPath, kind: EntryKind, metadata: EntryMetadata) -> Self {
        Self {
            name,
            path,
            kind,
            metadata,
        }
    }

    pub fn is_dir(&self) -> bool {
        self.kind.is_dir()
    }

    pub fn is_file(&self) -> bool {
        self.kind.is_file()
    }

    pub fn is_hidden(&self) -> bool {
        self.metadata.is_hidden
    }

    pub fn extension(&self) -> Option<&str> {
        self.name.rsplit_once('.').map(|(_, ext)| ext)
    }
}
