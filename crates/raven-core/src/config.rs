use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::sort::{SortColumn, SortDirection};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    #[serde(default)]
    pub general: GeneralConfig,
    #[serde(default)]
    pub appearance: AppearanceConfig,
    #[serde(default)]
    pub navigation: NavigationConfig,
    #[serde(default)]
    pub operations: OperationsConfig,
    #[serde(default)]
    pub preview: PreviewConfig,
    #[serde(default)]
    pub search: SearchConfig,
    #[serde(default)]
    pub bookmarks: Vec<Bookmark>,
    #[serde(default)]
    pub automation: AutomationConfig,
    #[serde(default)]
    pub plugins: PluginConfig,
    #[serde(default)]
    pub dbus: DbusConfig,
    #[serde(default)]
    pub keybindings: KeybindingsConfig,
    #[serde(default)]
    pub file_associations: Vec<FileAssociation>,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            general: GeneralConfig::default(),
            appearance: AppearanceConfig::default(),
            navigation: NavigationConfig::default(),
            operations: OperationsConfig::default(),
            preview: PreviewConfig::default(),
            search: SearchConfig::default(),
            automation: AutomationConfig::default(),
            plugins: PluginConfig::default(),
            dbus: DbusConfig::default(),
            keybindings: KeybindingsConfig::default(),
            file_associations: Vec::new(),
            bookmarks: vec![
                Bookmark {
                    name: "Home".to_string(),
                    path: dirs_string("HOME"),
                    icon: Some("user-home-symbolic".to_string()),
                },
                Bookmark {
                    name: "Documents".to_string(),
                    path: dir_join("Documents"),
                    icon: Some("folder-documents-symbolic".to_string()),
                },
                Bookmark {
                    name: "Downloads".to_string(),
                    path: dir_join("Downloads"),
                    icon: Some("folder-download-symbolic".to_string()),
                },
                Bookmark {
                    name: "Music".to_string(),
                    path: dir_join("Music"),
                    icon: Some("folder-music-symbolic".to_string()),
                },
                Bookmark {
                    name: "Pictures".to_string(),
                    path: dir_join("Pictures"),
                    icon: Some("folder-pictures-symbolic".to_string()),
                },
                Bookmark {
                    name: "Videos".to_string(),
                    path: dir_join("Videos"),
                    icon: Some("folder-videos-symbolic".to_string()),
                },
            ],
        }
    }
}

fn dirs_string(var: &str) -> String {
    std::env::var(var).unwrap_or_else(|_| "/".to_string())
}

fn dir_join(sub: &str) -> String {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/".to_string());
    format!("{}/{}", home, sub)
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GeneralConfig {
    pub show_hidden_files: bool,
    pub confirm_delete: bool,
    pub confirm_trash: bool,
    pub single_click_open: bool,
    pub default_terminal: String,
}

impl Default for GeneralConfig {
    fn default() -> Self {
        Self {
            show_hidden_files: false,
            confirm_delete: true,
            confirm_trash: false,
            single_click_open: false,
            default_terminal: "xdg-terminal-emulator".to_string(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Theme {
    AdwaitaDark,
    AdwaitaLight,
    CatppuccinMocha,
    CatppuccinLatte,
    Nord,
    Dracula,
    Frost,
    RosePine,
}

impl Default for Theme {
    fn default() -> Self {
        Self::CatppuccinMocha
    }
}

impl Theme {
    pub const ALL: &'static [Theme] = &[
        Theme::AdwaitaDark,
        Theme::AdwaitaLight,
        Theme::CatppuccinMocha,
        Theme::CatppuccinLatte,
        Theme::Nord,
        Theme::Dracula,
        Theme::Frost,
        Theme::RosePine,
    ];

    pub fn display_name(&self) -> &'static str {
        match self {
            Theme::AdwaitaDark => "Adwaita Dark",
            Theme::AdwaitaLight => "Adwaita Light",
            Theme::CatppuccinMocha => "Catppuccin Mocha",
            Theme::CatppuccinLatte => "Catppuccin Latte",
            Theme::Nord => "Nord",
            Theme::Dracula => "Dracula",
            Theme::Frost => "Frost",
            Theme::RosePine => "Rosé Pine",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ViewMode {
    List,
    Icons,
    Previews,
}

impl Default for ViewMode {
    fn default() -> Self {
        Self::List
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppearanceConfig {
    pub icon_size: u32,
    pub font_size: u32,
    pub show_path_bar: bool,
    pub show_status_bar: bool,
    pub show_sidebar: bool,
    pub sidebar_width: i32,
    pub preview_panel_width: i32,
    #[serde(default)]
    pub theme: Theme,
    #[serde(default)]
    pub view_mode: ViewMode,
}

impl Default for AppearanceConfig {
    fn default() -> Self {
        Self {
            icon_size: 24,
            font_size: 13,
            show_path_bar: true,
            show_status_bar: true,
            show_sidebar: true,
            sidebar_width: 200,
            preview_panel_width: 300,
            theme: Theme::default(),
            view_mode: ViewMode::default(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NavigationConfig {
    pub default_sort_column: SortColumn,
    pub default_sort_direction: SortDirection,
    pub directories_first: bool,
    pub default_path: Option<String>,
}

impl Default for NavigationConfig {
    fn default() -> Self {
        Self {
            default_sort_column: SortColumn::Name,
            default_sort_direction: SortDirection::Ascending,
            directories_first: true,
            default_path: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OperationsConfig {
    pub max_concurrent_operations: usize,
    pub copy_buffer_size: usize,
    pub default_conflict_strategy: String,
}

impl Default for OperationsConfig {
    fn default() -> Self {
        Self {
            max_concurrent_operations: 4,
            copy_buffer_size: 64 * 1024,
            default_conflict_strategy: "ask".to_string(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PreviewConfig {
    pub max_text_size: u64,
    pub max_image_size: u64,
    pub cache_size_mb: u64,
    pub enabled: bool,
}

impl Default for PreviewConfig {
    fn default() -> Self {
        Self {
            max_text_size: 1024 * 1024,
            max_image_size: 50 * 1024 * 1024,
            cache_size_mb: 256,
            enabled: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchConfig {
    pub max_results: usize,
    pub respect_gitignore: bool,
    pub follow_symlinks: bool,
}

impl Default for SearchConfig {
    fn default() -> Self {
        Self {
            max_results: 10000,
            respect_gitignore: true,
            follow_symlinks: false,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Bookmark {
    pub name: String,
    pub path: String,
    pub icon: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AutomationConfig {
    pub enabled: bool,
    pub rules_dir: Option<String>,
}

impl Default for AutomationConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            rules_dir: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginConfig {
    pub enabled: bool,
    pub plugin_dirs: Vec<String>,
    pub enabled_plugins: Vec<String>,
}

impl Default for PluginConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            plugin_dirs: Vec::new(),
            enabled_plugins: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DbusConfig {
    pub enabled: bool,
    pub bus_name: String,
}

impl Default for DbusConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            bus_name: "com.ravenfilemanager.Raven".to_string(),
        }
    }
}

/// A single keybinding mapping an action to a key combination.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Keybinding {
    pub action: String,
    pub key: String,
    pub modifiers: Vec<String>,
}

/// Custom keybinding configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct KeybindingsConfig {
    #[serde(default = "default_keybindings")]
    pub bindings: Vec<Keybinding>,
}

impl Default for KeybindingsConfig {
    fn default() -> Self {
        Self {
            bindings: default_keybindings(),
        }
    }
}

fn default_keybindings() -> Vec<Keybinding> {
    vec![
        Keybinding { action: "navigate_back".into(), key: "Left".into(), modifiers: vec!["Alt".into()] },
        Keybinding { action: "navigate_forward".into(), key: "Right".into(), modifiers: vec!["Alt".into()] },
        Keybinding { action: "navigate_up".into(), key: "Up".into(), modifiers: vec!["Alt".into()] },
        Keybinding { action: "new_tab".into(), key: "t".into(), modifiers: vec!["Ctrl".into()] },
        Keybinding { action: "close_tab".into(), key: "w".into(), modifiers: vec!["Ctrl".into()] },
        Keybinding { action: "toggle_hidden".into(), key: "h".into(), modifiers: vec!["Ctrl".into()] },
        Keybinding { action: "toggle_search".into(), key: "f".into(), modifiers: vec!["Ctrl".into()] },
        Keybinding { action: "edit_path".into(), key: "l".into(), modifiers: vec!["Ctrl".into()] },
        Keybinding { action: "refresh".into(), key: "r".into(), modifiers: vec!["Ctrl".into()] },
        Keybinding { action: "undo".into(), key: "z".into(), modifiers: vec!["Ctrl".into()] },
        Keybinding { action: "toggle_preview".into(), key: "space".into(), modifiers: vec![] },
        Keybinding { action: "properties".into(), key: "i".into(), modifiers: vec!["Ctrl".into()] },
        Keybinding { action: "rename".into(), key: "F2".into(), modifiers: vec![] },
        Keybinding { action: "trash".into(), key: "Delete".into(), modifiers: vec![] },
        Keybinding { action: "copy".into(), key: "c".into(), modifiers: vec!["Ctrl".into()] },
        Keybinding { action: "cut".into(), key: "x".into(), modifiers: vec!["Ctrl".into()] },
        Keybinding { action: "paste".into(), key: "v".into(), modifiers: vec!["Ctrl".into()] },
        Keybinding { action: "select_all".into(), key: "a".into(), modifiers: vec!["Ctrl".into()] },
        Keybinding { action: "open_settings".into(), key: "comma".into(), modifiers: vec!["Ctrl".into()] },
    ]
}

/// A file association mapping MIME types or extensions to a specific application.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileAssociation {
    /// MIME type pattern (e.g., "text/plain", "image/*", "application/pdf")
    pub mime_pattern: String,
    /// File extensions this applies to (e.g., ["rs", "py", "js"])
    #[serde(default)]
    pub extensions: Vec<String>,
    /// Application command to use (e.g., "code", "gimp %f", "vlc %f")
    pub application: String,
    /// Human-readable name for the application
    pub label: String,
}

impl AppConfig {
    pub fn config_dir() -> PathBuf {
        let xdg = std::env::var("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|_| {
                let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".to_string());
                PathBuf::from(home).join(".config")
            });
        xdg.join("raven")
    }

    pub fn config_path() -> PathBuf {
        Self::config_dir().join("config.toml")
    }

    pub fn load() -> Self {
        let path = Self::config_path();
        if path.exists() {
            let content = std::fs::read_to_string(&path).unwrap_or_default();
            toml::from_str(&content).unwrap_or_default()
        } else {
            Self::default()
        }
    }

    pub fn save(&self) -> Result<(), Box<dyn std::error::Error>> {
        let dir = Self::config_dir();
        std::fs::create_dir_all(&dir)?;
        let content = toml::to_string_pretty(self)?;
        std::fs::write(Self::config_path(), content)?;
        Ok(())
    }
}
