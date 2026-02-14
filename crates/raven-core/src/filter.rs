use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct FilterSpec {
    pub query: String,
    pub show_hidden: bool,
    pub file_types: Vec<FileTypeFilter>,
    pub min_size: Option<u64>,
    pub max_size: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum FileTypeFilter {
    Files,
    Directories,
    Symlinks,
    Images,
    Documents,
    Videos,
    Audio,
    Archives,
    Custom(String),
}

impl FilterSpec {
    pub fn empty() -> Self {
        Self::default()
    }

    pub fn with_query(query: impl Into<String>) -> Self {
        Self {
            query: query.into(),
            ..Self::default()
        }
    }

    pub fn is_empty(&self) -> bool {
        self.query.is_empty() && self.file_types.is_empty() && self.min_size.is_none() && self.max_size.is_none()
    }
}
