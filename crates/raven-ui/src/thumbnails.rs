//! Thumbnails for recycled list and grid items.
//!
//! A list item is rebound to another file as the view scrolls, so work started
//! for one file must not land on the item after it shows a different one. Each
//! item keeps a [`Slot`] naming the file it currently wants a thumbnail for;
//! results for anything else are dropped, and queued work for a file no item
//! wants any more is skipped before it starts.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use gtk::glib;
use gtk4 as gtk;

use raven_preview::thumbnail;

/// Thumbnail edge for list rows (32px icons, doubled for HiDPI).
pub const LIST_SIZE: u32 = 64;
/// Thumbnail edge for the icon grid.
pub const ICON_SIZE: u32 = 128;
/// Thumbnail edge for the preview grid.
pub const PREVIEW_SIZE: u32 = 256;

#[derive(Clone, Default)]
pub struct Slot(Arc<Mutex<Option<PathBuf>>>);

impl Slot {
    fn wants(&self, path: &Path) -> bool {
        self.0.lock().map(|p| p.as_deref() == Some(path)).unwrap_or(false)
    }

    fn set(&self, path: Option<PathBuf>) {
        if let Ok(mut p) = self.0.lock() {
            *p = path;
        }
    }

    /// Forget the current file, so pending work for it is dropped.
    pub fn clear(&self) {
        self.set(None);
    }

    /// Ask for a thumbnail of `path`, calling `apply` with the thumbnail file
    /// once there is one. Returns false, and never calls `apply`, when the
    /// file is not a kind that has thumbnails.
    ///
    /// An existing thumbnail is applied before this returns; otherwise one is
    /// generated on a worker thread and applied later, if the slot still
    /// wants `path` by then.
    pub fn request(&self, path: &Path, size: u32, apply: impl FnOnce(&Path) + 'static) -> bool {
        if thumbnail::kind_for(path).is_none() {
            self.clear();
            return false;
        }
        self.set(Some(path.to_path_buf()));
        if let Some(existing) = thumbnail::lookup(path, size) {
            apply(&existing);
            return true;
        }

        let slot = self.clone();
        let src = path.to_path_buf();
        glib::spawn_future_local(async move {
            let worker_slot = slot.clone();
            let worker_src = src.clone();
            let made = gtk::gio::spawn_blocking(move || {
                thumbnail::generate(&worker_src, size, || worker_slot.wants(&worker_src))
            })
            .await
            .ok()
            .flatten();
            if let Some(thumb) = made {
                if slot.wants(&src) {
                    apply(&thumb);
                }
            }
        });
        true
    }
}
