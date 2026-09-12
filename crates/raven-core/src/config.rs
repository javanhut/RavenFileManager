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
    /// Plugins to load at startup. Empty means every discovered plugin that
    /// is not in `disabled_plugins`.
    pub enabled_plugins: Vec<String>,
    /// Plugins never loaded at startup, whatever `enabled_plugins` says.
    #[serde(default)]
    pub disabled_plugins: Vec<String>,
}

impl Default for PluginConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            plugin_dirs: Vec::new(),
            enabled_plugins: Vec::new(),
            disabled_plugins: Vec::new(),
        }
    }
}

impl PluginConfig {
    /// Whether the plugin with `id` should be loaded at startup.
    pub fn is_plugin_enabled(&self, id: &str) -> bool {
        if self.disabled_plugins.iter().any(|d| d == id) {
            return false;
        }
        self.enabled_plugins.is_empty() || self.enabled_plugins.iter().any(|e| e == id)
    }

    /// Record the user's choice for one plugin.
    pub fn set_plugin_enabled(&mut self, id: &str, enabled: bool) {
        self.disabled_plugins.retain(|d| d != id);
        if enabled {
            if !self.enabled_plugins.is_empty() && !self.enabled_plugins.iter().any(|e| e == id) {
                self.enabled_plugins.push(id.to_string());
            }
        } else {
            self.enabled_plugins.retain(|e| e != id);
            self.disabled_plugins.push(id.to_string());
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DbusConfig {
    pub enabled: bool,
    pub bus_name: String,
    /// Claim `org.freedesktop.FileManager1` so desktop "show in folder"
    /// actions reach this window. Only one process can own the name; turn this
    /// off to leave it to another file manager.
    #[serde(default = "default_true")]
    pub file_manager1: bool,
}

impl Default for DbusConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            bus_name: "com.ravenfilemanager.Raven".to_string(),
            file_manager1: true,
        }
    }
}

fn default_true() -> bool {
    true
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

fn binding(action: &str, key: &str, modifiers: &[&str]) -> Keybinding {
    Keybinding {
        action: action.to_string(),
        key: key.to_string(),
        modifiers: modifiers.iter().map(|m| m.to_string()).collect(),
    }
}

/// Every action the window can dispatch, with its default shortcut. The
/// settings page lists these; unknown actions in a config file are kept but
/// do nothing.
fn default_keybindings() -> Vec<Keybinding> {
    vec![
        binding("navigate_back", "Left", &["Alt"]),
        binding("navigate_forward", "Right", &["Alt"]),
        binding("navigate_up", "Up", &["Alt"]),
        binding("navigate_home", "Home", &["Alt"]),
        binding("refresh", "r", &["Ctrl"]),
        binding("edit_path", "l", &["Ctrl"]),
        binding("toggle_hidden", "h", &["Ctrl"]),
        binding("toggle_search", "f", &["Ctrl"]),
        binding("toggle_preview", "space", &[]),
        binding("new_tab", "t", &["Ctrl"]),
        binding("close_tab", "w", &["Ctrl"]),
        binding("next_tab", "Page_Down", &["Ctrl"]),
        binding("prev_tab", "Page_Up", &["Ctrl"]),
        binding("toggle_dual_pane", "F3", &[]),
        binding("switch_pane", "F6", &[]),
        binding("copy", "c", &["Ctrl"]),
        binding("cut", "x", &["Ctrl"]),
        binding("paste", "v", &["Ctrl"]),
        binding("trash", "Delete", &[]),
        binding("delete_permanently", "Delete", &["Shift"]),
        binding("rename", "F2", &[]),
        binding("new_folder", "n", &["Ctrl", "Shift"]),
        binding("new_file", "n", &["Ctrl", "Alt"]),
        binding("select_all", "a", &["Ctrl"]),
        binding("undo", "z", &["Ctrl"]),
        binding("properties", "i", &["Ctrl"]),
        binding("open_settings", "comma", &["Ctrl"]),
    ]
}

/// A modifier name as written in a config, reduced to one spelling.
fn canonical_modifier(name: &str) -> Option<&'static str> {
    match name.trim().to_ascii_lowercase().as_str() {
        "ctrl" | "control" | "primary" => Some("Ctrl"),
        "alt" | "meta" | "option" => Some("Alt"),
        "shift" => Some("Shift"),
        "super" | "win" | "logo" | "cmd" => Some("Super"),
        _ => None,
    }
}

/// A key name as GDK reports it or a config spells it, reduced to one
/// spelling: case-insensitive, with a few aliases.
fn canonical_key(name: &str) -> String {
    let lower = name.trim().to_ascii_lowercase();
    match lower.as_str() {
        "esc" => "escape".to_string(),
        "return" | "enter" | "kp_enter" => "return".to_string(),
        "del" => "delete".to_string(),
        "pgup" | "page_up" | "pageup" | "prior" => "page_up".to_string(),
        "pgdn" | "page_down" | "pagedown" | "next" => "page_down".to_string(),
        "+" => "plus".to_string(),
        "-" => "minus".to_string(),
        "," => "comma".to_string(),
        "." => "period".to_string(),
        "/" => "slash".to_string(),
        " " => "space".to_string(),
        _ => lower,
    }
}

impl Keybinding {
    /// Whether a key press with these held modifiers is this binding.
    pub fn matches(&self, key_name: &str, held: &[&str]) -> bool {
        if canonical_key(&self.key) != canonical_key(key_name) {
            return false;
        }
        let mut wanted: Vec<&str> = self.modifiers.iter().filter_map(|m| canonical_modifier(m)).collect();
        let mut have: Vec<&str> = held.iter().filter_map(|m| canonical_modifier(m)).collect();
        wanted.sort_unstable();
        wanted.dedup();
        have.sort_unstable();
        have.dedup();
        wanted == have
    }
}

impl KeybindingsConfig {
    /// The action bound to a key press, if any. The first matching binding
    /// wins, so a user's binding placed earlier shadows a default.
    pub fn action_for(&self, key_name: &str, held: &[&str]) -> Option<&str> {
        self.bindings
            .iter()
            .find(|b| b.matches(key_name, held))
            .map(|b| b.action.as_str())
    }

    /// Add the default shortcut for every action the saved config does not
    /// mention, so a config written by an older version still reaches the
    /// newer actions.
    pub fn with_missing_defaults(mut self) -> Self {
        for default in default_keybindings() {
            if !self.bindings.iter().any(|b| b.action == default.action) {
                self.bindings.push(default);
            }
        }
        self
    }
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
        let mut config: Self = if path.exists() {
            let content = std::fs::read_to_string(&path).unwrap_or_default();
            toml::from_str(&content).unwrap_or_default()
        } else {
            Self::default()
        };
        config.keybindings = std::mem::take(&mut config.keybindings.bindings)
            .into_iter()
            .fold(KeybindingsConfig { bindings: Vec::new() }, |mut acc, b| {
                acc.bindings.push(b);
                acc
            })
            .with_missing_defaults();
        config
    }

    pub fn save(&self) -> Result<(), Box<dyn std::error::Error>> {
        let dir = Self::config_dir();
        std::fs::create_dir_all(&dir)?;
        let content = toml::to_string_pretty(self)?;
        std::fs::write(Self::config_path(), content)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keybinding_matching_ignores_case_and_modifier_order() {
        let b = binding("new_folder", "N", &["Shift", "Ctrl"]);
        assert!(b.matches("n", &["Ctrl", "Shift"]));
        assert!(b.matches("N", &["shift", "control"]));
        assert!(!b.matches("n", &["Ctrl"]));
        assert!(!b.matches("m", &["Ctrl", "Shift"]));
        let plain = binding("rename", "F2", &[]);
        assert!(plain.matches("F2", &[]));
        assert!(!plain.matches("F2", &["Ctrl"]));
    }

    #[test]
    fn key_aliases_resolve() {
        let b = binding("next_tab", "PgDn", &["Ctrl"]);
        assert!(b.matches("Page_Down", &["Ctrl"]));
        let c = binding("open_settings", ",", &["Ctrl"]);
        assert!(c.matches("comma", &["Ctrl"]));
    }

    #[test]
    fn action_lookup_prefers_the_first_binding() {
        let cfg = KeybindingsConfig {
            bindings: vec![
                binding("custom", "r", &["Ctrl"]),
                binding("refresh", "r", &["Ctrl"]),
            ],
        };
        assert_eq!(cfg.action_for("r", &["Ctrl"]), Some("custom"));
        assert_eq!(cfg.action_for("q", &["Ctrl"]), None);
    }

    #[test]
    fn missing_defaults_are_filled_in_without_overriding() {
        let cfg = KeybindingsConfig {
            bindings: vec![binding("refresh", "F5", &[])],
        }
        .with_missing_defaults();
        let refresh: Vec<_> = cfg.bindings.iter().filter(|b| b.action == "refresh").collect();
        assert_eq!(refresh.len(), 1);
        assert_eq!(refresh[0].key, "F5");
        assert!(cfg.bindings.iter().any(|b| b.action == "toggle_dual_pane"));
    }
}
