use std::path::Path;

use gio::prelude::*;
use gtk4 as gtk;
use gtk::prelude::*;

use raven_core::config::AppConfig;
use raven_core::path::RavenPath;

/// Open a file using configured file associations, falling back to the
/// desktop's default application. Failures are only logged; use
/// [`try_open_file`] where the caller can show the error.
pub fn open_file(path: &RavenPath, config: &AppConfig) {
    if let Err(e) = try_open_file(path, config) {
        tracing::warn!("{}", e);
    }
}

/// Open a file the way a double click does.
///
/// Raven's own file associations win, since the user configured them on
/// purpose. Otherwise the desktop default for the file's content type is used
/// through GIO (the same lookup the Open With menu shows as "default"), and
/// only if GIO cannot launch anything does the `open` crate get a try.
pub fn try_open_file(path: &RavenPath, config: &AppConfig) -> Result<(), String> {
    let Some(local) = path.as_local_path() else {
        return Err("Only local files can be opened".to_string());
    };

    if let Some(index) = association_for(config, local) {
        let assoc = &config.file_associations[index];
        spawn_application(&assoc.application, local)?;
        record_recent(local);
        return Ok(());
    }

    let uri = gio::File::for_path(local).uri();
    let context = default_launch_context();
    match gio::AppInfo::launch_default_for_uri(&uri, context.as_ref()) {
        Ok(()) => {
            record_recent(local);
            Ok(())
        }
        Err(gio_err) => {
            tracing::debug!("GIO could not open {}: {}", uri, gio_err);
            match open::that(local) {
                Ok(()) => {
                    record_recent(local);
                    Ok(())
                }
                Err(e) => Err(format!(
                    "Could not open {}: {}",
                    local.display(),
                    first_non_empty(&gio_err.to_string(), &e.to_string())
                )),
            }
        }
    }
}

/// Index of the first configured association for `local`: an extension match
/// anywhere in the list beats a MIME pattern match.
fn association_for(config: &AppConfig, local: &Path) -> Option<usize> {
    let extension = local
        .extension()
        .and_then(|e| e.to_str())
        .map(|s| s.to_lowercase());
    let ext = extension.as_deref()?;

    if let Some(i) = config
        .file_associations
        .iter()
        .position(|assoc| assoc.extensions.iter().any(|a| a.eq_ignore_ascii_case(ext)))
    {
        return Some(i);
    }

    let mime = mime_from_extension(ext);
    config
        .file_associations
        .iter()
        .position(|assoc| !assoc.mime_pattern.is_empty() && mime_matches(&assoc.mime_pattern, &mime))
}

fn first_non_empty<'a>(a: &'a str, b: &'a str) -> &'a str {
    if a.is_empty() {
        b
    } else {
        a
    }
}

/// Launch context for the default display, so launched applications get
/// startup notification and land on the right screen. `None` when GTK has no
/// display (tests, headless).
fn default_launch_context() -> Option<gio::AppLaunchContext> {
    gtk::gdk::Display::default().map(|d| d.app_launch_context().upcast())
}

/// Spawn an application command, replacing `%f` with the file path.
fn spawn_application(command: &str, file_path: &Path) -> Result<(), String> {
    let path_str = file_path.to_string_lossy();
    let full_cmd = if command.contains("%f") {
        command.replace("%f", &path_str)
    } else {
        format!("{} {}", command, shell_escape(&path_str))
    };

    tracing::info!("Opening with: {}", full_cmd);
    std::process::Command::new("sh")
        .arg("-c")
        .arg(&full_cmd)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map(|_| ())
        .map_err(|e| format!("Failed to run '{}': {}", full_cmd, e))
}

/// Simple shell escaping: wrap in single quotes, escaping existing single quotes.
fn shell_escape(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// Check if a MIME pattern matches a MIME type (supports `*` wildcards like `text/*`).
fn mime_matches(pattern: &str, mime: &str) -> bool {
    if pattern == "*" || pattern == "*/*" {
        return true;
    }
    if let Some(prefix) = pattern.strip_suffix("/*") {
        // e.g., "text/*" matches "text/plain"
        if let Some(mime_prefix) = mime.split('/').next() {
            return prefix == mime_prefix;
        }
    }
    pattern == mime
}

/// Map common file extensions to MIME types.
fn mime_from_extension(ext: &str) -> String {
    match ext {
        // Text
        "txt" => "text/plain",
        "md" => "text/markdown",
        "html" | "htm" => "text/html",
        "css" => "text/css",
        "csv" => "text/csv",
        "xml" => "text/xml",
        "json" => "application/json",
        "toml" => "application/toml",
        "yaml" | "yml" => "application/x-yaml",
        // Code
        "rs" | "py" | "js" | "ts" | "c" | "cpp" | "h" | "go" | "java" | "rb" | "sh" => {
            "text/x-script"
        }
        // Images
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "svg" => "image/svg+xml",
        "webp" => "image/webp",
        "bmp" => "image/bmp",
        // Audio
        "mp3" => "audio/mpeg",
        "flac" => "audio/flac",
        "ogg" => "audio/ogg",
        "wav" => "audio/wav",
        "m4a" => "audio/mp4",
        // Video
        "mp4" => "video/mp4",
        "mkv" => "video/x-matroska",
        "avi" => "video/x-msvideo",
        "webm" => "video/webm",
        "mov" => "video/quicktime",
        // Documents
        "pdf" => "application/pdf",
        "doc" | "docx" => "application/msword",
        "xls" | "xlsx" => "application/vnd.ms-excel",
        // Archives
        "zip" => "application/zip",
        "tar" => "application/x-tar",
        "gz" => "application/gzip",
        "xz" => "application/x-xz",
        "bz2" => "application/x-bzip2",
        "7z" => "application/x-7z-compressed",
        "rar" => "application/vnd.rar",
        _ => "application/octet-stream",
    }
    .to_string()
}

/// Get matching file associations for a given file entry (used for "Open With" submenu).
pub fn get_matching_associations(
    config: &AppConfig,
    extension: Option<&str>,
) -> Vec<(usize, String)> {
    let mut matches = Vec::new();
    let mime = extension.map(|e| mime_from_extension(&e.to_lowercase()));

    for (i, assoc) in config.file_associations.iter().enumerate() {
        let ext_match = extension
            .map(|ext| assoc.extensions.iter().any(|a| a.eq_ignore_ascii_case(ext)))
            .unwrap_or(false);

        let mime_match = mime
            .as_deref()
            .map(|m| !assoc.mime_pattern.is_empty() && mime_matches(&assoc.mime_pattern, m))
            .unwrap_or(false);

        if ext_match || mime_match {
            matches.push((i, assoc.label.clone()));
        }
    }
    matches
}

/// Open a file with a specific association index from config.
pub fn open_with_association(
    path: &RavenPath,
    config: &AppConfig,
    index: usize,
) -> Result<(), String> {
    let Some(local) = path.as_local_path() else {
        return Err("Only local files can be opened".to_string());
    };
    let Some(assoc) = config.file_associations.get(index) else {
        return Err(format!("File association {} no longer exists", index));
    };
    spawn_application(&assoc.application, local)?;
    record_recent(local);
    Ok(())
}

// ---------------------------------------------------------------------------
// Installed applications (Open With)
// ---------------------------------------------------------------------------

/// What an "Open With" menu item does, carried as the string target of the
/// `file.open-with` action. A string rather than an index because installed
/// applications are identified by desktop id, and the list can change between
/// building the menu and activating it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OpenWithTarget {
    /// An installed application, by desktop id (e.g. `org.gnome.eog.desktop`).
    App(String),
    /// A Raven config file association, by index.
    Association(usize),
    /// Ask with the application chooser dialog.
    Other,
}

impl OpenWithTarget {
    pub fn to_target(&self) -> String {
        match self {
            Self::App(id) => format!("app:{}", id),
            Self::Association(i) => format!("assoc:{}", i),
            Self::Other => "other".to_string(),
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        if s == "other" {
            return Some(Self::Other);
        }
        if let Some(id) = s.strip_prefix("app:") {
            return (!id.is_empty()).then(|| Self::App(id.to_string()));
        }
        s.strip_prefix("assoc:")
            .and_then(|i| i.parse().ok())
            .map(Self::Association)
    }
}

/// Content type of a local file as GIO sees it.
///
/// A confident guess from the name is used as is: this runs on the main
/// thread when a context menu opens, and querying `standard::content-type`
/// reads the start of the file, which can stall on a slow or hung mount. Only
/// names that say nothing (no or unknown extension) are sniffed; a file that
/// cannot be queried keeps the name guess.
pub fn content_type_for_path(path: &Path) -> String {
    let (guess, uncertain) = gio::content_type_guess(Some(path), None::<&[u8]>);
    if !uncertain {
        return guess.to_string();
    }
    let file = gio::File::for_path(path);
    let queried = file
        .query_info(
            "standard::content-type,standard::fast-content-type",
            gio::FileQueryInfoFlags::NONE,
            gio::Cancellable::NONE,
        )
        .ok()
        .and_then(|info| {
            info.content_type().or_else(|| {
                info.attribute_string("standard::fast-content-type")
            })
        });
    match queried {
        Some(ct) if !ct.is_empty() => ct.to_string(),
        _ => guess.to_string(),
    }
}

/// Applications registered for `content_type`, the desktop default first.
///
/// Hidden (`NoDisplay`) applications are dropped unless one is the default,
/// which the user evidently wants to see.
pub fn apps_for_content_type(content_type: &str) -> (Vec<gio::AppInfo>, Option<String>) {
    let default = gio::AppInfo::default_for_type(content_type, false);
    let default_id = default.as_ref().and_then(|a| a.id()).map(|s| s.to_string());

    let mut apps: Vec<gio::AppInfo> = default.into_iter().collect();
    apps.extend(gio::AppInfo::all_for_type(content_type));
    let ids: Vec<Option<String>> = apps.iter().map(|a| a.id().map(|s| s.to_string())).collect();
    let keep = default_first_unique(&ids, default_id.as_deref());

    let apps = keep
        .into_iter()
        .map(|i| apps[i].clone())
        .filter(|a| a.should_show() || a.id().as_deref() == default_id.as_deref())
        .collect();
    (apps, default_id)
}

/// Indices of `ids` in display order: the entry equal to `default` first, then
/// the rest in their original order, each id only once. Entries without an id
/// cannot be told apart and are all kept.
fn default_first_unique(ids: &[Option<String>], default: Option<&str>) -> Vec<usize> {
    let mut seen = std::collections::HashSet::new();
    let mut order = Vec::new();
    if let Some(d) = default {
        if let Some(i) = ids.iter().position(|id| id.as_deref() == Some(d)) {
            seen.insert(d.to_string());
            order.push(i);
        }
    }
    for (i, id) in ids.iter().enumerate() {
        match id {
            Some(id) => {
                if seen.insert(id.clone()) {
                    order.push(i);
                }
            }
            None => order.push(i),
        }
    }
    order
}

/// Find an installed application by desktop id.
pub fn app_by_id(id: &str) -> Option<gio::AppInfo> {
    gio::AppInfo::all()
        .into_iter()
        .find(|a| a.id().as_deref() == Some(id))
}

/// Program name (basename of the first word) of a shell command line.
fn command_program(command: &str) -> Option<&str> {
    let first = command.split_whitespace().next()?;
    let first = first.trim_matches(|c| c == '\'' || c == '"');
    let base = first.rsplit('/').next().unwrap_or(first);
    (!base.is_empty()).then_some(base)
}

/// Programs that only start something else (sandboxes, interpreters, `env`).
/// Sharing one of these says nothing about launching the same application:
/// every Flatpak app's executable is `flatpak`.
const LAUNCHER_PROGRAMS: &[&str] = &[
    "flatpak", "snap", "env", "sh", "bash", "dash", "zsh", "fish", "python", "python3",
    "perl", "java", "wine", "gtk-launch", "xdg-open", "gio", "sudo", "pkexec", "nice",
    "firejail", "bwrap",
];

/// Whether a config association is just another way of launching one of the
/// installed applications already listed, judged by program name or label.
/// Launcher programs ([`LAUNCHER_PROGRAMS`]) never count as a program match.
pub fn association_duplicates_app(
    assoc_label: &str,
    assoc_command: &str,
    app_names: &[String],
    app_programs: &[String],
) -> bool {
    let program_dup = command_program(assoc_command)
        .filter(|p| !LAUNCHER_PROGRAMS.contains(p))
        .is_some_and(|p| app_programs.iter().any(|a| a == p));
    let label_dup = app_names
        .iter()
        .any(|n| n.eq_ignore_ascii_case(assoc_label.trim()));
    program_dup || label_dup
}

/// Basename of an application's executable, for [`association_duplicates_app`].
pub fn app_program(app: &gio::AppInfo) -> String {
    app.executable()
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default()
}

/// Launch `app` with every local file in `paths` in one invocation (an
/// application that takes a single file is started once per file by GIO).
pub fn launch_app_with(
    app: &gio::AppInfo,
    paths: &[RavenPath],
    display: &gtk::gdk::Display,
) -> Result<(), String> {
    let locals: Vec<&std::path::PathBuf> = paths.iter().filter_map(|p| p.as_local_path()).collect();
    if locals.is_empty() {
        return Err("Only local files can be opened".to_string());
    }
    let files: Vec<gio::File> = locals.iter().map(gio::File::for_path).collect();
    let context = display.app_launch_context();
    app.launch(&files, Some(&context))
        .map_err(|e| format!("Could not start {}: {}", app.display_name(), e.message()))?;
    for local in locals {
        record_recent(local);
    }
    Ok(())
}

/// Add a file Raven opened to the shared recent-files list
/// (`recently-used.xbel`), where a Recent view and other applications find it.
pub fn record_recent(path: &Path) {
    // RecentManager needs GTK initialised; skip quietly when it is not (tests).
    if !gtk::is_initialized_main_thread() {
        return;
    }
    let uri = gio::File::for_path(path).uri();
    if !gtk::RecentManager::default().add_item(&uri) {
        tracing::debug!("Could not add {} to recent files", uri);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn target_round_trips() {
        for target in [
            OpenWithTarget::App("org.gnome.eog.desktop".into()),
            OpenWithTarget::Association(3),
            OpenWithTarget::Other,
        ] {
            assert_eq!(OpenWithTarget::parse(&target.to_target()), Some(target));
        }
    }

    #[test]
    fn target_rejects_garbage() {
        assert_eq!(OpenWithTarget::parse("app:"), None);
        assert_eq!(OpenWithTarget::parse("assoc:x"), None);
        assert_eq!(OpenWithTarget::parse("assoc:-1"), None);
        assert_eq!(OpenWithTarget::parse("7"), None);
    }

    #[test]
    fn default_goes_first_and_ids_are_unique() {
        let ids = vec![
            Some("a".to_string()),
            Some("b".to_string()),
            None,
            Some("a".to_string()),
            Some("c".to_string()),
            None,
        ];
        assert_eq!(default_first_unique(&ids, Some("c")), vec![4, 0, 1, 2, 5]);
        assert_eq!(default_first_unique(&ids, None), vec![0, 1, 2, 4, 5]);
        assert_eq!(default_first_unique(&ids, Some("zzz")), vec![0, 1, 2, 4, 5]);
    }

    #[test]
    fn content_type_from_file_and_from_name() {
        let dir = std::env::temp_dir().join(format!("raven-opener-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("notes.txt");
        std::fs::write(&file, "hello\n").unwrap();
        assert_eq!(content_type_for_path(&file), "text/plain");
        // A missing file falls back to guessing from the name.
        assert_eq!(content_type_for_path(&dir.join("missing.png")), "image/png");
        // A name that says nothing is sniffed from the contents.
        let bare = dir.join("qzxnoext");
        std::fs::write(&bare, "plain words\n").unwrap();
        assert_eq!(content_type_for_path(&bare), "text/plain");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn command_program_takes_basename() {
        assert_eq!(command_program("/usr/bin/gimp %f"), Some("gimp"));
        assert_eq!(command_program("  'mpv' --fs"), Some("mpv"));
        assert_eq!(command_program(""), None);
    }

    #[test]
    fn association_duplicates() {
        let names = vec!["Image Viewer".to_string()];
        let programs = vec!["eog".to_string()];
        assert!(association_duplicates_app("Viewer", "/usr/bin/eog", &names, &programs));
        assert!(association_duplicates_app("image viewer", "other", &names, &programs));
        assert!(!association_duplicates_app("GIMP", "gimp %f", &names, &programs));
        // A shared launcher is not the same application.
        let programs = vec!["flatpak".to_string(), "sh".to_string()];
        assert!(!association_duplicates_app("Gimp", "flatpak run org.gimp.GIMP %f", &names, &programs));
        assert!(!association_duplicates_app("Tool", "sh -c 'mytool %f'", &names, &programs));
    }

    #[test]
    fn association_prefers_extension_match() {
        use raven_core::config::FileAssociation;
        let mut config = AppConfig::default();
        config.file_associations = vec![
            FileAssociation {
                label: "Any image".into(),
                mime_pattern: "image/*".into(),
                extensions: vec![],
                application: "viewer".into(),
            },
            FileAssociation {
                label: "PNG".into(),
                mime_pattern: String::new(),
                extensions: vec!["PNG".into()],
                application: "pngtool".into(),
            },
        ];
        assert_eq!(association_for(&config, Path::new("/x/a.png")), Some(1));
        assert_eq!(association_for(&config, Path::new("/x/a.jpg")), Some(0));
        assert_eq!(association_for(&config, Path::new("/x/a.txt")), None);
        assert_eq!(association_for(&config, Path::new("/x/noext")), None);
    }
}
