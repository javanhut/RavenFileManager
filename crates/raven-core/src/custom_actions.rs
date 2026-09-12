//! Custom context-menu actions: external commands run on the selection.
//!
//! Actions come from `actions.toml` in the config directory when the user has
//! one, and otherwise from the copy built into the binary. Each action names
//! a command and arguments with placeholders, and says what selection it
//! applies to.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::config::AppConfig;
use crate::entry::FileEntry;

/// The actions shipped with the file manager.
pub const BUILTIN_ACTIONS_TOML: &str = include_str!("../../../config/actions.toml");

/// Extensions treated as archives for `show_for = "archives"`.
const ARCHIVE_EXTENSIONS: [&str; 11] = [
    "zip", "tar", "gz", "tgz", "xz", "txz", "bz2", "tbz2", "7z", "rar", "zst",
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ShowFor {
    #[default]
    All,
    Files,
    Directories,
    Archives,
}

impl ShowFor {
    pub const ALL: [ShowFor; 4] = [
        ShowFor::All,
        ShowFor::Files,
        ShowFor::Directories,
        ShowFor::Archives,
    ];

    pub fn label(self) -> &'static str {
        match self {
            ShowFor::All => "Anything",
            ShowFor::Files => "Files only",
            ShowFor::Directories => "Folders only",
            ShowFor::Archives => "Archives only",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CustomAction {
    pub name: String,
    pub command: String,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub icon: Option<String>,
    #[serde(default)]
    pub show_for: ShowFor,
    /// Offer the action for a multi-item selection too.
    #[serde(default)]
    pub multiple: bool,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct ActionsFile {
    #[serde(default)]
    actions: Vec<CustomAction>,
}

/// What an action is run against: the selected local paths, and the
/// directory the command starts in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActionTarget {
    pub paths: Vec<PathBuf>,
    pub directory: PathBuf,
}

impl ActionTarget {
    /// Build the target for `selected` inside `current_dir`. A single selected
    /// folder becomes the working directory, so "open in terminal" on a folder
    /// opens that folder rather than the one containing it.
    ///
    /// `None` when any selected item is not on the local filesystem.
    pub fn from_selection(selected: &[FileEntry], current_dir: &Path) -> Option<Self> {
        let mut paths = Vec::with_capacity(selected.len());
        for entry in selected {
            paths.push(entry.path.as_local_path()?.clone());
        }
        let directory = match selected {
            [only] if only.is_dir() => paths[0].clone(),
            _ => current_dir.to_path_buf(),
        };
        Some(Self { paths, directory })
    }
}

impl CustomAction {
    /// Whether the action is offered for `selected`. An empty selection means
    /// the directory being shown.
    pub fn applies_to(&self, selected: &[FileEntry]) -> bool {
        if selected.is_empty() {
            return matches!(self.show_for, ShowFor::All | ShowFor::Directories);
        }
        if selected.len() > 1 && !self.multiple {
            return false;
        }
        if selected.iter().any(|e| !e.path.is_local()) {
            return false;
        }
        match self.show_for {
            ShowFor::All => true,
            ShowFor::Files => selected.iter().all(|e| !e.is_dir()),
            ShowFor::Directories => selected.iter().all(|e| e.is_dir()),
            ShowFor::Archives => selected.iter().all(|e| {
                !e.is_dir()
                    && e.extension()
                        .map(|ext| ARCHIVE_EXTENSIONS.contains(&ext.to_ascii_lowercase().as_str()))
                        .unwrap_or(false)
            }),
        }
    }

    /// The arguments with placeholders filled in for `target`.
    ///
    /// `{path}` is the first selected path (the directory when nothing is
    /// selected), `{directory}` the working directory, `{name}` the first
    /// path's file name, and `{paths}` every selected path. An argument that
    /// is exactly `{paths}` expands to one argument per path; embedded in a
    /// longer argument the paths are joined with spaces.
    pub fn expand_args(&self, target: &ActionTarget) -> Vec<String> {
        let first = target
            .paths
            .first()
            .cloned()
            .unwrap_or_else(|| target.directory.clone());
        let first_str = first.to_string_lossy().to_string();
        let dir_str = target.directory.to_string_lossy().to_string();
        let name = first
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_default();
        let all: Vec<String> = if target.paths.is_empty() {
            vec![dir_str.clone()]
        } else {
            target
                .paths
                .iter()
                .map(|p| p.to_string_lossy().to_string())
                .collect()
        };

        let mut out = Vec::with_capacity(self.args.len());
        for arg in &self.args {
            if arg == "{paths}" {
                out.extend(all.iter().cloned());
                continue;
            }
            out.push(
                arg.replace("{paths}", &all.join(" "))
                    .replace("{path}", &first_str)
                    .replace("{directory}", &dir_str)
                    .replace("{name}", &name),
            );
        }
        out
    }
}

/// Where the user's own actions file lives.
pub fn user_actions_path() -> PathBuf {
    AppConfig::config_dir().join("actions.toml")
}

/// Parse an actions file.
pub fn parse(toml_text: &str) -> Result<Vec<CustomAction>, toml::de::Error> {
    toml::from_str::<ActionsFile>(toml_text).map(|f| f.actions)
}

/// The built-in actions.
pub fn builtin() -> Vec<CustomAction> {
    parse(BUILTIN_ACTIONS_TOML).unwrap_or_default()
}

/// The user's actions if they have a readable file, else the built-in set.
pub fn load() -> Vec<CustomAction> {
    let path = user_actions_path();
    match std::fs::read_to_string(&path) {
        Ok(text) => match parse(&text) {
            Ok(actions) => actions,
            Err(e) => {
                tracing::warn!("{} is not valid; using built-in actions: {}", path.display(), e);
                builtin()
            }
        },
        Err(_) => builtin(),
    }
}

/// Write the user's actions file.
pub fn save(actions: &[CustomAction]) -> Result<(), Box<dyn std::error::Error>> {
    let file = ActionsFile {
        actions: actions.to_vec(),
    };
    let dir = AppConfig::config_dir();
    std::fs::create_dir_all(&dir)?;
    let text = toml::to_string_pretty(&file)?;
    std::fs::write(user_actions_path(), text)?;
    Ok(())
}

/// Split a command line the way a shell would for the simple cases: on
/// whitespace, with double or single quotes grouping, and backslash escaping
/// the next character outside single quotes.
pub fn split_args(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();
    let mut in_word = false;
    let mut quote: Option<char> = None;
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        match quote {
            Some('\'') => {
                if c == '\'' {
                    quote = None;
                } else {
                    current.push(c);
                }
            }
            Some(_) => match c {
                '"' => quote = None,
                '\\' => {
                    if let Some(next) = chars.next() {
                        current.push(next);
                    }
                }
                _ => current.push(c),
            },
            None => match c {
                '"' | '\'' => {
                    quote = Some(c);
                    in_word = true;
                }
                '\\' => {
                    if let Some(next) = chars.next() {
                        current.push(next);
                        in_word = true;
                    }
                }
                c if c.is_whitespace() => {
                    if in_word {
                        out.push(std::mem::take(&mut current));
                        in_word = false;
                    }
                }
                _ => {
                    current.push(c);
                    in_word = true;
                }
            },
        }
    }
    if in_word {
        out.push(current);
    }
    out
}

/// The inverse of [`split_args`]: quote what needs quoting.
pub fn join_args(args: &[String]) -> String {
    args.iter()
        .map(|a| {
            if a.is_empty() || a.chars().any(|c| c.is_whitespace() || c == '"' || c == '\'') {
                format!("\"{}\"", a.replace('\\', "\\\\").replace('"', "\\\""))
            } else {
                a.clone()
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entry::{EntryKind, EntryMetadata};
    use crate::path::RavenPath;

    fn entry(path: &str, dir: bool) -> FileEntry {
        let kind = if dir { EntryKind::Directory } else { EntryKind::File };
        let name = Path::new(path)
            .file_name()
            .unwrap()
            .to_string_lossy()
            .to_string();
        FileEntry::new(name, RavenPath::local(path), kind, EntryMetadata::default())
    }

    #[test]
    fn builtin_actions_parse() {
        let actions = builtin();
        assert!(actions.iter().any(|a| a.name == "Open in Terminal"));
        let terminal = actions.iter().find(|a| a.name == "Open in Terminal").unwrap();
        assert_eq!(terminal.show_for, ShowFor::Directories);
        let compress = actions.iter().find(|a| a.name == "Compress...").unwrap();
        assert!(compress.multiple);
    }

    #[test]
    fn applies_to_respects_kind_and_count() {
        let files_only = CustomAction {
            name: "x".into(),
            command: "x".into(),
            args: vec![],
            icon: None,
            show_for: ShowFor::Files,
            multiple: false,
        };
        assert!(files_only.applies_to(&[entry("/a/f.txt", false)]));
        assert!(!files_only.applies_to(&[entry("/a/d", true)]));
        assert!(!files_only.applies_to(&[entry("/a/f.txt", false), entry("/a/g.txt", false)]));
        assert!(!files_only.applies_to(&[]));

        let archives = CustomAction {
            show_for: ShowFor::Archives,
            ..files_only.clone()
        };
        assert!(archives.applies_to(&[entry("/a/f.TAR", false)]));
        assert!(!archives.applies_to(&[entry("/a/f.txt", false)]));

        let dirs = CustomAction {
            show_for: ShowFor::Directories,
            ..files_only
        };
        // Nothing selected means the directory being shown.
        assert!(dirs.applies_to(&[]));
    }

    #[test]
    fn placeholders_expand() {
        let action = CustomAction {
            name: "x".into(),
            command: "x".into(),
            args: vec![
                "--cwd".into(),
                "{directory}".into(),
                "{paths}".into(),
                "first={path}".into(),
                "name={name}".into(),
            ],
            icon: None,
            show_for: ShowFor::All,
            multiple: true,
        };
        let target = ActionTarget::from_selection(
            &[entry("/a/f.txt", false), entry("/a/g h.txt", false)],
            Path::new("/a"),
        )
        .unwrap();
        assert_eq!(
            action.expand_args(&target),
            vec![
                "--cwd",
                "/a",
                "/a/f.txt",
                "/a/g h.txt",
                "first=/a/f.txt",
                "name=f.txt"
            ]
        );
    }

    #[test]
    fn a_single_folder_is_the_working_directory() {
        let target =
            ActionTarget::from_selection(&[entry("/a/sub", true)], Path::new("/a")).unwrap();
        assert_eq!(target.directory, PathBuf::from("/a/sub"));
        let none = ActionTarget::from_selection(&[], Path::new("/a")).unwrap();
        assert_eq!(none.directory, PathBuf::from("/a"));
        assert!(none.paths.is_empty());
    }

    #[test]
    fn split_and_join_round_trip() {
        let args = split_args(r#"--add "a b" 'c d' e\ f {path}"#);
        assert_eq!(args, vec!["--add", "a b", "c d", "e f", "{path}"]);
        let joined = join_args(&args);
        assert_eq!(split_args(&joined), args);
        assert!(split_args("   ").is_empty());
    }
}
