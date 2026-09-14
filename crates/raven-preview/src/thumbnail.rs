//! Small cached thumbnails for images, videos and PDFs.
//!
//! Used by the list and grid views, which need many of them quickly, and by
//! the preview panel. Thumbnails are PNGs under
//! `$XDG_CACHE_HOME/raven/thumbnails/`, keyed by path, size and modification
//! time so an edited file gets a fresh one. Everything here blocks: callers
//! run it off the UI thread.

use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Condvar, Mutex};
use std::time::{Duration, Instant, UNIX_EPOCH};

/// Raster formats the `image` crate decodes. SVG is left to GTK, which
/// renders it at any size without a thumbnail.
pub const IMAGE_EXTENSIONS: &[&str] = &[
    "png", "jpg", "jpeg", "gif", "webp", "bmp", "ico", "tiff", "tif",
];

pub const VIDEO_EXTENSIONS: &[&str] = &[
    "mp4", "m4v", "mkv", "webm", "mov", "avi", "wmv", "flv", "mpg", "mpeg", "ogv", "3gp",
    "ts", "mts", "m2ts",
];

/// Paged documents whose first page is rendered in-process (see `pdf`).
pub const DOCUMENT_EXTENSIONS: &[&str] = &["pdf"];

/// Thumbnails generated at once. Video frames cost an ffmpeg process each,
/// and a folder of hundreds should not start hundreds of them.
const MAX_CONCURRENT: usize = 3;

/// A stuck ffmpeg (a truncated download, a network mount) must not hold a
/// permit forever.
const FFMPEG_TIMEOUT: Duration = Duration::from_secs(15);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThumbnailKind {
    Image,
    Video,
    Document,
}

/// What kind of thumbnail `path` can have, judged by extension.
pub fn kind_for(path: &Path) -> Option<ThumbnailKind> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    if IMAGE_EXTENSIONS.contains(&ext.as_str()) {
        Some(ThumbnailKind::Image)
    } else if VIDEO_EXTENSIONS.contains(&ext.as_str()) {
        Some(ThumbnailKind::Video)
    } else if DOCUMENT_EXTENSIONS.contains(&ext.as_str()) {
        Some(ThumbnailKind::Document)
    } else {
        None
    }
}

pub fn cache_dir() -> PathBuf {
    let base = std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache")))
        .unwrap_or_else(std::env::temp_dir);
    base.join("raven").join("thumbnails")
}

/// Where the thumbnail of `path` at `size` lives, whether or not it exists.
/// `None` when the file itself cannot be read.
fn cached_path(path: &Path, size: u32) -> Option<PathBuf> {
    let modified = std::fs::metadata(path).ok()?.modified().ok()?;
    let mtime = modified.duration_since(UNIX_EPOCH).ok()?.as_nanos();
    let mut hasher = blake3::Hasher::new();
    hasher.update(path.as_os_str().as_encoded_bytes());
    hasher.update(&mtime.to_le_bytes());
    hasher.update(&size.to_le_bytes());
    Some(cache_dir().join(format!("{}.png", hasher.finalize().to_hex())))
}

/// An already generated thumbnail, without generating one. Cheap enough to
/// call on the UI thread.
pub fn lookup(path: &Path, size: u32) -> Option<PathBuf> {
    cached_path(path, size).filter(|p| p.is_file())
}

/// Return the thumbnail of `path` no larger than `size` pixels on its longer
/// side, generating it if needed.
///
/// `still_wanted` is asked once a generation slot is free, so work queued for
/// a row that has since scrolled away is dropped instead of done.
pub fn generate(path: &Path, size: u32, still_wanted: impl Fn() -> bool) -> Option<PathBuf> {
    let kind = kind_for(path)?;
    generate_with(path, size, still_wanted, |tmp| match kind {
        ThumbnailKind::Image => image_thumbnail(path, tmp, size),
        ThumbnailKind::Video => video_thumbnail(path, tmp, size),
        ThumbnailKind::Document => crate::pdf::render_first_page(path, tmp, size),
    })
}

/// `generate` with the rendering supplied by the caller: `make` writes the
/// thumbnail to the path it is given and says whether it did. It is only
/// called when the thumbnail is not cached, under a generation slot, so a
/// caller that learns more from the file while rendering (the PDF preview
/// reads the page count from the same parse) does not have to open it twice.
pub(crate) fn generate_with(
    path: &Path,
    size: u32,
    still_wanted: impl Fn() -> bool,
    make: impl FnOnce(&Path) -> bool,
) -> Option<PathBuf> {
    let target = cached_path(path, size)?;
    if target.is_file() {
        return Some(target);
    }

    let _permit = Permit::acquire();
    if !still_wanted() {
        return None;
    }
    // Another thread may have made it while this one waited.
    if target.is_file() {
        return Some(target);
    }

    std::fs::create_dir_all(target.parent()?).ok()?;
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let tmp = target.with_extension(format!(
        "{}-{}.tmp",
        std::process::id(),
        COUNTER.fetch_add(1, Ordering::Relaxed)
    ));

    let made = make(&tmp);
    // Written aside and renamed, so a reader never sees half a PNG.
    if made && std::fs::rename(&tmp, &target).is_ok() {
        Some(target)
    } else {
        let _ = std::fs::remove_file(&tmp);
        None
    }
}

fn image_thumbnail(src: &Path, dst: &Path, size: u32) -> bool {
    let Ok(reader) = image::ImageReader::open(src).and_then(|r| r.with_guessed_format()) else {
        return false;
    };
    match reader.decode() {
        Ok(img) => img
            .thumbnail(size, size)
            .save_with_format(dst, image::ImageFormat::Png)
            .is_ok(),
        Err(_) => false,
    }
}

fn video_thumbnail(src: &Path, dst: &Path, size: u32) -> bool {
    let scale = format!(
        "thumbnail=24,scale={size}:{size}:force_original_aspect_ratio=decrease"
    );
    // A few seconds in skips black lead-in frames; clips shorter than that
    // have no frame there, so retry from the start.
    for seek in ["3", "0"] {
        let mut cmd = Command::new("ffmpeg");
        cmd.args(["-nostdin", "-v", "error", "-y", "-ss", seek, "-i"])
            .arg(src)
            .args(["-frames:v", "1", "-vf", &scale, "-f", "image2", "-c:v", "png"])
            .arg(dst);
        if run_with_timeout(cmd, FFMPEG_TIMEOUT)
            && std::fs::metadata(dst).map(|m| m.len() > 0).unwrap_or(false)
        {
            return true;
        }
    }
    false
}

/// Run `cmd` with no stdio, killing it after `timeout`. True on success.
pub(crate) fn run_with_timeout(mut cmd: Command, timeout: Duration) -> bool {
    let Ok(mut child) = cmd
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
    else {
        return false;
    };
    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return status.success(),
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(25)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return false;
            }
        }
    }
}

static SLOTS: (Mutex<usize>, Condvar) = (Mutex::new(0), Condvar::new());

struct Permit;

impl Permit {
    fn acquire() -> Self {
        let (lock, cvar) = &SLOTS;
        let mut used = lock.lock().unwrap_or_else(|e| e.into_inner());
        while *used >= MAX_CONCURRENT {
            used = cvar.wait(used).unwrap_or_else(|e| e.into_inner());
        }
        *used += 1;
        Permit
    }
}

impl Drop for Permit {
    fn drop(&mut self) {
        let (lock, cvar) = &SLOTS;
        let mut used = lock.lock().unwrap_or_else(|e| e.into_inner());
        *used -= 1;
        cvar.notify_one();
    }
}

/// Held by tests that generate thumbnails. One of them points
/// `XDG_CACHE_HOME` at a temporary directory that is deleted when it ends, and
/// generating into that directory at the same time fails.
#[cfg(test)]
pub(crate) static CACHE_ENV_LOCK: Mutex<()> = Mutex::new(());

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kind_is_judged_by_extension() {
        assert_eq!(kind_for(Path::new("/a/b.PNG")), Some(ThumbnailKind::Image));
        assert_eq!(kind_for(Path::new("/a/b.mkv")), Some(ThumbnailKind::Video));
        assert_eq!(kind_for(Path::new("/a/b.Pdf")), Some(ThumbnailKind::Document));
        assert_eq!(kind_for(Path::new("/a/b.svg")), None);
        assert_eq!(kind_for(Path::new("/a/b")), None);
    }

    #[test]
    fn image_thumbnail_is_generated_and_reused() {
        let _env = CACHE_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().unwrap();
        std::env::set_var("XDG_CACHE_HOME", dir.path().join("cache"));
        let src = dir.path().join("big.png");
        image::RgbImage::new(400, 200).save(&src).unwrap();

        assert!(lookup(&src, 64).is_none());
        let thumb = generate(&src, 64, || true).expect("thumbnail");
        let (w, h) = image::image_dimensions(&thumb).unwrap();
        assert_eq!((w, h), (64, 32));
        assert_eq!(lookup(&src, 64), Some(thumb));
    }

    #[test]
    fn pdf_thumbnail_is_the_first_page() {
        let _env = CACHE_ENV_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("doc.pdf");
        std::fs::write(&src, crate::pdf::sample_pdf(2, Some("t"))).unwrap();
        // A size no other test uses, so the shared cache cannot already hold it.
        let thumb = generate(&src, 48, || true).expect("pdf thumbnail");
        assert_eq!(image::image_dimensions(&thumb).unwrap(), (48, 36));
    }

    #[test]
    fn unwanted_work_is_skipped() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("x.png");
        image::RgbImage::new(10, 10).save(&src).unwrap();
        // A different size than any other test, so no cached file satisfies it.
        assert!(generate(&src, 7, || false).is_none());
    }
}
