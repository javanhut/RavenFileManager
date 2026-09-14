//! Shared handling for files dropped onto Raven.
//!
//! Drops arrive either from Raven's own listings or from other applications.
//! Other applications offer a `GdkFileList` (text/uri-list on the wire, or the
//! portal's file transfer for sandboxed apps); Raven's own drags also carry a
//! plain string of `file://` lines, and tab reordering uses a string too. Every
//! file drop target therefore accepts both types, preferring the file list.
//!
//! Whether a drop copies or moves follows the usual file manager convention:
//! Ctrl copies, Shift moves, and otherwise files are moved within one
//! filesystem (a cheap rename) and copied across filesystems (where a "move"
//! would silently delete the originals from, say, a USB stick).

use std::cell::{Cell, RefCell};
use std::os::unix::fs::MetadataExt;
use std::path::PathBuf;
use std::rc::Rc;

use gtk4 as gtk;
use gtk::gdk;
use gtk::gio;
use gtk::glib;
use gtk::prelude::*;

use raven_core::commands::AppCommand;
use raven_core::path::RavenPath;

/// What a file drop does with its sources.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DropOp {
    Copy,
    Move,
}

impl DropOp {
    fn drag_action(self) -> gdk::DragAction {
        match self {
            DropOp::Copy => gdk::DragAction::COPY,
            DropOp::Move => gdk::DragAction::MOVE,
        }
    }
}

/// Decide between copy and move.
///
/// `same_fs` is `None` when it could not be determined (sources not loaded
/// yet, or a stat failed); copying is the choice that cannot lose data then.
/// `offered` is what the drag source allows: some sources only offer one
/// action, and on Wayland the compositor narrows the offer to match the held
/// modifier, so the wanted action falls back to whichever one is offered.
pub fn choose_action(
    ctrl: bool,
    shift: bool,
    same_fs: Option<bool>,
    offered: gdk::DragAction,
) -> Option<DropOp> {
    let wanted = if ctrl {
        DropOp::Copy
    } else if shift {
        DropOp::Move
    } else if same_fs == Some(true) {
        DropOp::Move
    } else {
        DropOp::Copy
    };
    let other = match wanted {
        DropOp::Copy => DropOp::Move,
        DropOp::Move => DropOp::Copy,
    };
    [wanted, other]
        .into_iter()
        .find(|op| offered.contains(op.drag_action()))
}

/// Whether every source lives on the same filesystem as `destination`.
///
/// Sources are stat'ed without following symlinks, since moving a link moves
/// the link itself. A remote destination is never the same filesystem.
pub fn same_filesystem(sources: &[PathBuf], destination: &RavenPath) -> Option<bool> {
    same_filesystem_as(source_devices(sources).as_deref(), destination)
}

/// The distinct devices the sources live on, or `None` when there are no
/// sources or one cannot be stat'ed. This is the per-drag expensive half of
/// [`same_filesystem`]; the per-destination half is a single stat.
fn source_devices(sources: &[PathBuf]) -> Option<Vec<u64>> {
    if sources.is_empty() {
        return None;
    }
    let mut devs = Vec::new();
    for src in sources {
        let dev = std::fs::symlink_metadata(src).ok()?.dev();
        if !devs.contains(&dev) {
            devs.push(dev);
        }
    }
    Some(devs)
}

fn same_filesystem_as(source_devs: Option<&[u64]>, destination: &RavenPath) -> Option<bool> {
    let Some(dest) = destination.as_local_path() else {
        return Some(false);
    };
    let source_devs = source_devs?;
    let dest_dev = std::fs::metadata(dest).ok()?.dev();
    Some(source_devs.iter().all(|dev| *dev == dest_dev))
}

/// The sources that would actually go anywhere: files already living in
/// `destination` are dropped from the transfer rather than moved onto
/// themselves (or copied next to themselves), while the rest still go.
pub fn sources_to_transfer(paths: &[PathBuf], destination: &RavenPath) -> Vec<RavenPath> {
    paths
        .iter()
        .cloned()
        .map(RavenPath::local)
        .filter(|src| src.parent().as_ref() != Some(destination))
        .collect()
}

/// Local paths from a text/uri-list or newline-separated `file://` URIs.
///
/// URIs are percent-decoded through GIO, so names with spaces or accents
/// survive. Comment lines and non-local URIs (http, sftp, ...) are skipped.
pub fn parse_uri_list(text: &str) -> Vec<PathBuf> {
    text.lines()
        .map(|line| line.trim())
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .filter(|line| line.get(..5).is_some_and(|s| s.eq_ignore_ascii_case("file:")))
        .filter_map(|line| {
            gio::File::for_uri(line).path().or_else(|| {
                // Older Raven builds wrote unescaped `file:///path` lines, which
                // GIO rejects when the name contains a stray `%`.
                line.strip_prefix("file://")
                    .filter(|p| p.starts_with('/'))
                    .map(PathBuf::from)
            })
        })
        .collect()
}

/// Local paths carried by a dropped value: a `GdkFileList` or a URI string.
pub fn paths_from_value(value: &glib::Value) -> Vec<PathBuf> {
    if let Ok(list) = value.get::<gdk::FileList>() {
        return list.files().iter().filter_map(|f| f.path()).collect();
    }
    if let Ok(text) = value.get::<String>() {
        return parse_uri_list(&text);
    }
    Vec::new()
}

/// Modifier keys of the event the target is currently handling.
fn modifiers(target: &gtk::DropTarget) -> (bool, bool) {
    let state = target.current_event_state();
    (
        state.contains(gdk::ModifierType::CONTROL_MASK),
        state.contains(gdk::ModifierType::SHIFT_MASK),
    )
}

/// Facts about the drag in progress, shared by every drop target it crosses.
///
/// Keyed by the `GdkDrop`, which lives for as long as the drag hovers one
/// window, so crossing tabs and bookmark rows neither re-stats the sources
/// nor forgets which actions the source offered.
struct DragInfo {
    drop: glib::WeakRef<gdk::Drop>,
    /// Every action set seen on the drop. On Wayland GDK overwrites the
    /// drop's actions with the single action the compositor picked in reply
    /// to our last preference, so reading them fresh would lock a drag onto
    /// whatever we preferred before the sources had loaded. The first value
    /// seen is the source's full offer, and the union keeps it.
    offered: gdk::DragAction,
    /// `source_devices` of the dropped paths, once computed.
    source_devs: Option<Option<Vec<u64>>>,
}

thread_local! {
    static CURRENT_DRAG: RefCell<Option<DragInfo>> = const { RefCell::new(None) };
}

/// Run `f` on the info for `drop`, starting afresh when it is a new drag.
fn with_drag_info<R>(drop: &gdk::Drop, f: impl FnOnce(&mut DragInfo) -> R) -> R {
    CURRENT_DRAG.with(|cell| {
        let mut current = cell.borrow_mut();
        let same = current
            .as_ref()
            .and_then(|info| info.drop.upgrade())
            .is_some_and(|d| &d == drop);
        if !same {
            *current = Some(DragInfo {
                drop: drop.downgrade(),
                offered: gdk::DragAction::empty(),
                source_devs: None,
            });
        }
        f(current.as_mut().expect("drag info was just set"))
    })
}

/// The actions the drag source allows.
fn offered_actions(target: &gtk::DropTarget) -> gdk::DragAction {
    let Some(drop) = target.current_drop() else {
        return gdk::DragAction::COPY | gdk::DragAction::MOVE;
    };
    // Drags from Raven itself carry their source, whose offer is exact.
    if let Some(drag) = drop.drag() {
        return drag.actions();
    }
    let now = drop.actions();
    with_drag_info(&drop, |info| {
        info.offered |= now;
        info.offered
    })
}

/// Whether the dropped paths share `destination`'s filesystem, stat'ing the
/// sources only once per drag.
fn drag_same_filesystem(
    target: &gtk::DropTarget,
    paths: &[PathBuf],
    destination: &RavenPath,
) -> Option<bool> {
    let Some(drop) = target.current_drop() else {
        return same_filesystem(paths, destination);
    };
    let devs = with_drag_info(&drop, |info| {
        info.source_devs
            .get_or_insert_with(|| source_devices(paths))
            .clone()
    });
    same_filesystem_as(devs.as_deref(), destination)
}

/// What a file drop target knows about the value hovering it.
#[derive(Clone, Copy, Default)]
struct Hover {
    same_fs: Option<bool>,
    /// Every dropped file already lives in the destination, so a drop would
    /// do nothing.
    nothing_to_do: bool,
}

fn action_for(target: &gtk::DropTarget, hover: Hover) -> gdk::DragAction {
    if hover.nothing_to_do {
        return gdk::DragAction::empty();
    }
    let (ctrl, shift) = modifiers(target);
    choose_action(ctrl, shift, hover.same_fs, offered_actions(target))
        .map(DropOp::drag_action)
        .unwrap_or_else(gdk::DragAction::empty)
}

/// A drop target for files that shows copy or move on the cursor.
///
/// The value is preloaded so the sources can be compared against
/// `destination` while hovering; the sources are stat'ed once per drag
/// (shared across all targets) rather than on each motion event. When
/// `highlight` is given, that CSS class is added to the target's widget while
/// a drag hovers it. Callers connect `drop` themselves and normally call
/// [`perform_drop`].
pub fn file_drop_target(
    highlight: Option<&'static str>,
    destination: impl Fn() -> Option<RavenPath> + 'static,
) -> gtk::DropTarget {
    let target = gtk::DropTarget::new(
        glib::Type::INVALID,
        gdk::DragAction::COPY | gdk::DragAction::MOVE,
    );
    target.set_types(&[gdk::FileList::static_type(), glib::Type::STRING]);
    target.set_preload(true);

    let hover: Rc<Cell<Hover>> = Rc::new(Cell::new(Hover::default()));
    let destination = Rc::new(destination);

    let refresh: Rc<dyn Fn(&gtk::DropTarget)> = {
        let hover = hover.clone();
        let destination = destination.clone();
        Rc::new(move |target: &gtk::DropTarget| {
            let (Some(value), Some(dest)) = (target.value(), destination()) else {
                hover.set(Hover::default());
                return;
            };
            let paths = paths_from_value(&value);
            // Tab reorder strings carry no paths and must stay droppable.
            let nothing_to_do = !paths.is_empty() && sources_to_transfer(&paths, &dest).is_empty();
            hover.set(Hover {
                same_fs: drag_same_filesystem(target, &paths, &dest),
                nothing_to_do,
            });
        })
    };

    {
        let refresh = refresh.clone();
        target.connect_value_notify(move |target| refresh(target));
    }
    {
        let hover = hover.clone();
        target.connect_enter(move |target, _, _| {
            refresh(target);
            if let (Some(class), Some(widget)) = (highlight, target.widget()) {
                widget.add_css_class(class);
            }
            action_for(target, hover.get())
        });
    }
    target.connect_motion(move |target, _, _| action_for(target, hover.get()));
    target.connect_leave(move |target| {
        if let (Some(class), Some(widget)) = (highlight, target.widget()) {
            widget.remove_css_class(class);
        }
    });
    target
}

/// Copy or move the dropped files into `destination`. Returns whether a
/// command was sent.
///
/// Files dropped onto the directory they already live in are left alone
/// rather than moved onto themselves; the rest of the selection still goes.
pub fn perform_drop(
    target: &gtk::DropTarget,
    value: &glib::Value,
    destination: &RavenPath,
    cmd_tx: &tokio::sync::mpsc::UnboundedSender<AppCommand>,
) -> bool {
    let paths = paths_from_value(value);
    let sources = sources_to_transfer(&paths, destination);
    if sources.is_empty() {
        return false;
    }
    let same_fs = drag_same_filesystem(target, &paths, destination);
    let (ctrl, shift) = modifiers(target);
    let destination = destination.clone();
    let command = match choose_action(ctrl, shift, same_fs, offered_actions(target)) {
        Some(DropOp::Copy) => AppCommand::CopyFiles { sources, destination },
        Some(DropOp::Move) => AppCommand::MoveFiles { sources, destination },
        None => return false,
    };
    let _ = cmd_tx.send(command);
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    const BOTH: gdk::DragAction = gdk::DragAction::COPY.union(gdk::DragAction::MOVE);

    #[test]
    fn uri_lists_are_percent_decoded() {
        assert_eq!(
            parse_uri_list("file:///tmp/a%20b/caf%C3%A9.txt"),
            vec![PathBuf::from("/tmp/a b/café.txt")]
        );
    }

    #[test]
    fn uri_lists_handle_crlf_comments_and_blank_lines() {
        let text = "# dragged from somewhere\r\nfile:///a/b.txt\r\n\r\nfile:///a/c.txt\r\n";
        assert_eq!(
            parse_uri_list(text),
            vec![PathBuf::from("/a/b.txt"), PathBuf::from("/a/c.txt")]
        );
    }

    #[test]
    fn non_local_uris_and_plain_text_are_skipped() {
        let text = "https://example.com/x.png\nsftp://host/file\nhello\n/plain/path\nfile:///ok";
        assert_eq!(parse_uri_list(text), vec![PathBuf::from("/ok")]);
    }

    #[test]
    fn unescaped_legacy_lines_still_parse() {
        assert_eq!(
            parse_uri_list("file:///tmp/100% done.txt\r\n"),
            vec![PathBuf::from("/tmp/100% done.txt")]
        );
    }

    #[test]
    fn values_of_either_type_yield_paths() {
        let list = gdk::FileList::from_array(&[gio::File::for_path("/tmp/x y")]);
        assert_eq!(paths_from_value(&list.to_value()), vec![PathBuf::from("/tmp/x y")]);
        let text = "file:///tmp/x%20y".to_value();
        assert_eq!(paths_from_value(&text), vec![PathBuf::from("/tmp/x y")]);
        assert!(paths_from_value(&5i32.to_value()).is_empty());
    }

    #[test]
    fn modifiers_override_the_filesystem_default() {
        assert_eq!(choose_action(true, false, Some(true), BOTH), Some(DropOp::Copy));
        assert_eq!(choose_action(false, true, Some(false), BOTH), Some(DropOp::Move));
        // Ctrl wins when both are held.
        assert_eq!(choose_action(true, true, Some(true), BOTH), Some(DropOp::Copy));
    }

    #[test]
    fn default_depends_on_filesystem() {
        assert_eq!(choose_action(false, false, Some(true), BOTH), Some(DropOp::Move));
        assert_eq!(choose_action(false, false, Some(false), BOTH), Some(DropOp::Copy));
        assert_eq!(choose_action(false, false, None, BOTH), Some(DropOp::Copy));
    }

    #[test]
    fn falls_back_to_what_the_source_offers() {
        let copy = gdk::DragAction::COPY;
        let mv = gdk::DragAction::MOVE;
        assert_eq!(choose_action(false, false, Some(true), copy), Some(DropOp::Copy));
        assert_eq!(choose_action(true, false, None, mv), Some(DropOp::Move));
        assert_eq!(choose_action(false, false, None, gdk::DragAction::LINK), None);
    }

    #[test]
    fn filesystem_comparison() {
        let dir = std::env::temp_dir().join(format!("raven-dnd-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("f");
        std::fs::write(&file, b"x").unwrap();
        let dest = RavenPath::local(dir.clone());

        assert_eq!(same_filesystem(&[file.clone()], &dest), Some(true));
        // procfs is always its own filesystem.
        assert_eq!(same_filesystem(&[PathBuf::from("/proc/self")], &dest), Some(false));
        assert_eq!(same_filesystem(&[dir.join("missing")], &dest), None);
        assert_eq!(same_filesystem(&[], &dest), None);
        let remote = RavenPath::Sftp {
            host: "h".into(),
            port: 22,
            user: "u".into(),
            path: "/".into(),
        };
        assert_eq!(same_filesystem(&[file.clone()], &remote), Some(false));

        // A selection spanning two filesystems is not "same filesystem", and
        // devices are deduplicated so the per-destination check stays cheap.
        let devs = source_devices(&[file.clone(), file.clone(), PathBuf::from("/proc/self")])
            .unwrap();
        assert_eq!(devs.len(), 2);
        assert_eq!(same_filesystem_as(Some(&devs), &dest), Some(false));
        assert_eq!(same_filesystem_as(Some(&devs[..1]), &dest), Some(true));
        assert_eq!(same_filesystem_as(None, &dest), None);

        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn only_files_already_in_the_destination_are_skipped() {
        let dest = RavenPath::local("/a");
        let paths = [PathBuf::from("/a/x.txt"), PathBuf::from("/b/y.txt")];
        assert_eq!(
            sources_to_transfer(&paths, &dest),
            vec![RavenPath::local("/b/y.txt")]
        );
        assert!(sources_to_transfer(&paths[..1], &dest).is_empty());
        // A folder dropped onto itself is not "already in" itself.
        assert_eq!(
            sources_to_transfer(&[PathBuf::from("/a")], &dest),
            vec![RavenPath::local("/a")]
        );
    }
}
