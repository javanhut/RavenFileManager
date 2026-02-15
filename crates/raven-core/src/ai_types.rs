use crate::entry::FileEntry;

/// A group of duplicate files sharing the same content hash.
#[derive(Debug, Clone)]
pub struct DuplicateGroup {
    /// The blake3 hash of the file content.
    pub hash: String,
    /// The size of each file in bytes.
    pub size: u64,
    /// The duplicate file entries.
    pub entries: Vec<FileEntry>,
}

/// Progress information during a duplicate scan.
#[derive(Debug, Clone)]
pub struct DuplicateScanProgress {
    pub files_scanned: u64,
    pub total_files: u64,
    pub bytes_hashed: u64,
    pub duplicates_found: u64,
    pub phase: ScanPhase,
}

/// The current phase of a duplicate scan.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScanPhase {
    Walking,
    Hashing,
    Complete,
}

/// A suggestion for organizing files in a directory.
#[derive(Debug, Clone)]
pub struct OrganizeSuggestion {
    pub description: String,
    pub source_files: Vec<FileEntry>,
    pub destination: String,
    pub category: String,
}
