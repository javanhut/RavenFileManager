use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SortColumn {
    Name,
    Size,
    Modified,
    Kind,
    Permissions,
    Owner,
    Extension,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SortDirection {
    Ascending,
    Descending,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct SortSpec {
    pub column: SortColumn,
    pub direction: SortDirection,
    pub directories_first: bool,
}

impl Default for SortSpec {
    fn default() -> Self {
        Self {
            column: SortColumn::Name,
            direction: SortDirection::Ascending,
            directories_first: true,
        }
    }
}
