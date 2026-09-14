//! PDF previews: the first page rendered with hayro, plus page count and
//! title.
//!
//! hayro is a pure-Rust rasterizer, so this works without poppler, mupdf or
//! ghostscript installed. It runs in-process, which means a malformed file
//! could panic or take a long time inside it; both are contained here (panics
//! are caught, and a render that overruns its time limit is abandoned).

use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::Path;
use std::sync::{mpsc, Condvar, Mutex};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use hayro::hayro_interpret::InterpreterSettings;
use hayro::hayro_syntax::Pdf;
use hayro::vello_cpu::color::palette::css::WHITE;
use hayro::{render, RenderCache, RenderSettings};
use raven_core::error::{RavenError, RavenResult};
use raven_core::events::PreviewData;
use raven_core::path::RavenPath;

use crate::thumbnail::{self, DOCUMENT_EXTENSIONS};
use crate::{require_local, PreviewProvider};

/// Size of the first-page image shown in the preview panel. Matches the
/// panel's image size so it stays sharp when the panel is widened.
const PANEL_THUMBNAIL_SIZE: u32 = 768;

/// Files larger than this are not loaded into memory to be parsed for the
/// preview panel, which shows one file at a time.
const MAX_PARSE_BYTES: u64 = 512 * 1024 * 1024;

/// The list and grid views ask for a thumbnail of every PDF in the folder,
/// each a whole-file read, so they stop at a much smaller size and larger
/// files keep their icon there.
const MAX_THUMBNAIL_BYTES: u64 = 64 * 1024 * 1024;

/// A pathological file must not hold a thumbnail permit forever. The worker
/// thread cannot be killed, so on timeout it is left to finish on its own and
/// its result is thrown away.
const RENDER_TIMEOUT: Duration = Duration::from_secs(15);

/// Worker threads parsing or rendering PDFs at once. An abandoned worker
/// keeps its place (and its copy of the file) until it really exits, so a
/// folder of pathological files cannot pile up busy threads.
const MAX_WORKERS: usize = 2;

static WORKERS: Limiter = Limiter::new(MAX_WORKERS);

/// PDF previews: first page, page count and title.
pub struct PdfPreview;

impl PdfPreview {
    pub fn new() -> Self {
        Self
    }
}

impl Default for PdfPreview {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl PreviewProvider for PdfPreview {
    async fn generate(&self, path: &RavenPath) -> RavenResult<PreviewData> {
        let local = require_local(path)?.clone();
        let display = path.to_string();
        tokio::task::spawn_blocking(move || {
            // Parsed once for both the page image and the details when the
            // image has to be made; only read for the details when cached.
            let mut info = None;
            let thumbnail =
                thumbnail::generate_with(&local, PANEL_THUMBNAIL_SIZE, || true, |tmp| {
                    let (read, rendered) =
                        inspect_and_render(&local, tmp, PANEL_THUMBNAIL_SIZE);
                    info = read;
                    rendered
                });
            let info = info.unwrap_or_else(|| read_info(&local));
            PreviewData::Document {
                path: local,
                thumbnail,
                page_count: info.page_count,
                title: info.title,
            }
        })
        .await
        .map_err(|e| RavenError::Preview {
            message: format!("document preview task failed for {}: {}", display, e),
        })
    }

    fn supports(&self, extension: &str) -> bool {
        DOCUMENT_EXTENSIONS.contains(&extension.to_ascii_lowercase().as_str())
    }
}

#[derive(Debug, Default, PartialEq)]
pub struct DocumentInfo {
    pub page_count: Option<u32>,
    pub title: Option<String>,
}

fn load(path: &Path, max_bytes: u64) -> Option<Pdf> {
    let size = std::fs::metadata(path).ok()?.len();
    if size > max_bytes {
        return None;
    }
    let data = std::fs::read(path).ok()?;
    catch_unwind(move || Pdf::new(data).ok()).ok().flatten()
}

fn info_of(pdf: &Pdf) -> DocumentInfo {
    catch_unwind(AssertUnwindSafe(|| DocumentInfo {
        page_count: u32::try_from(pdf.pages().len()).ok(),
        title: pdf.metadata().title.as_deref().and_then(decode_text_string),
    }))
    .unwrap_or_default()
}

/// Page count and title of the PDF at `path`, whichever can be read.
/// Encrypted, broken or too slow to parse files give an empty `DocumentInfo`.
pub fn read_info(path: &Path) -> DocumentInfo {
    let path = path.to_path_buf();
    run_guarded(&WORKERS, RENDER_TIMEOUT, move || {
        load(&path, MAX_PARSE_BYTES).map(|pdf| info_of(&pdf)).unwrap_or_default()
    })
    .unwrap_or_default()
}

/// Decode a PDF text string: UTF-16BE or UTF-8 when marked with a byte order
/// mark, otherwise PDFDocEncoding, which agrees with Latin-1 for everything a
/// title realistically contains. Blank titles count as none.
fn decode_text_string(bytes: &[u8]) -> Option<String> {
    let text = if let Some(rest) = bytes.strip_prefix(&[0xFE, 0xFF]) {
        let units: Vec<u16> = rest
            .chunks_exact(2)
            .map(|pair| u16::from_be_bytes([pair[0], pair[1]]))
            .collect();
        String::from_utf16_lossy(&units)
    } else if let Some(rest) = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]) {
        String::from_utf8_lossy(rest).into_owned()
    } else {
        bytes.iter().map(|&b| b as char).collect()
    };
    let text = text.trim().replace(|c: char| c.is_control(), " ");
    (!text.is_empty()).then_some(text)
}

/// The panel's work: details and the first page from a single parse. The
/// details are kept even when rendering fails or overruns the time limit.
fn inspect_and_render(src: &Path, dst: &Path, size: u32) -> (Option<DocumentInfo>, bool) {
    enum Step {
        Info(DocumentInfo),
        Rendered(bool),
    }
    let (src, dst) = (src.to_path_buf(), dst.to_path_buf());
    let Some(rx) = spawn_guarded(&WORKERS, RENDER_TIMEOUT, move |tx| {
        let Some(pdf) = load(&src, MAX_PARSE_BYTES) else {
            let _ = tx.send(Step::Info(DocumentInfo::default()));
            return;
        };
        let _ = tx.send(Step::Info(info_of(&pdf)));
        let _ = tx.send(Step::Rendered(render_to_png(&pdf, &dst, size)));
    }) else {
        return (None, false);
    };
    let deadline = Instant::now() + RENDER_TIMEOUT;
    let (mut info, mut rendered) = (None, false);
    while let Ok(step) = rx.recv_timeout(deadline.saturating_duration_since(Instant::now())) {
        match step {
            Step::Info(read) => info = Some(read),
            Step::Rendered(ok) => {
                rendered = ok;
                break;
            }
        }
    }
    (info, rendered)
}

/// Render the first page of `src` as a PNG at `dst`, no larger than `size`
/// pixels on its longer side. True on success.
pub(crate) fn render_first_page(src: &Path, dst: &Path, size: u32) -> bool {
    let max_bytes = if size >= PANEL_THUMBNAIL_SIZE {
        MAX_PARSE_BYTES
    } else {
        MAX_THUMBNAIL_BYTES
    };
    let (src, dst) = (src.to_path_buf(), dst.to_path_buf());
    run_guarded(&WORKERS, RENDER_TIMEOUT, move || {
        load(&src, max_bytes).is_some_and(|pdf| render_to_png(&pdf, &dst, size))
    })
    .unwrap_or(false)
}

fn render_to_png(pdf: &Pdf, dst: &Path, size: u32) -> bool {
    catch_unwind(AssertUnwindSafe(|| {
        let Some(page) = pdf.pages().first() else {
            return false;
        };
        let (width, height) = page.render_dimensions();
        let longer = width.max(height);
        if !longer.is_finite() || longer < 1.0 {
            return false;
        }
        let scale = size.min(u16::MAX as u32) as f32 / longer;
        let settings = RenderSettings {
            x_scale: scale,
            y_scale: scale,
            bg_color: WHITE,
            ..Default::default()
        };
        let pixmap = render(page, &RenderCache::new(), &InterpreterSettings::default(), &settings);
        if pixmap.width() == 0 || pixmap.height() == 0 {
            return false;
        }
        match pixmap.into_png() {
            Ok(png) => std::fs::write(dst, png).is_ok(),
            Err(_) => false,
        }
    }))
    .unwrap_or(false)
}

/// A counting semaphore whose places are held by worker threads.
struct Limiter {
    used: Mutex<usize>,
    freed: Condvar,
    max: usize,
}

impl Limiter {
    const fn new(max: usize) -> Self {
        Self {
            used: Mutex::new(0),
            freed: Condvar::new(),
            max,
        }
    }

    /// Take a place, waiting at most `wait` for one to free up.
    fn acquire(&'static self, wait: Duration) -> Option<Place> {
        let deadline = Instant::now() + wait;
        let mut used = self.used.lock().unwrap_or_else(|e| e.into_inner());
        while *used >= self.max {
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return None;
            }
            used = self
                .freed
                .wait_timeout(used, left)
                .unwrap_or_else(|e| e.into_inner())
                .0;
        }
        *used += 1;
        Some(Place(self))
    }
}

struct Place(&'static Limiter);

impl Drop for Place {
    fn drop(&mut self) {
        let mut used = self.0.used.lock().unwrap_or_else(|e| e.into_inner());
        *used -= 1;
        self.0.freed.notify_one();
    }
}

/// Start `job` on a worker thread holding a place in `limiter` for as long as
/// the thread runs, panics caught. `None` when no place frees up within
/// `timeout` or the thread cannot be started.
fn spawn_guarded<M: Send + 'static>(
    limiter: &'static Limiter,
    timeout: Duration,
    job: impl FnOnce(&mpsc::Sender<M>) + Send + 'static,
) -> Option<mpsc::Receiver<M>> {
    let place = limiter.acquire(timeout)?;
    let (tx, rx) = mpsc::channel();
    std::thread::Builder::new()
        .name("pdf-worker".into())
        .spawn(move || {
            let _place = place;
            let _ = catch_unwind(AssertUnwindSafe(|| job(&tx)));
        })
        .ok()?;
    Some(rx)
}

/// Run `job` on a guarded worker and wait up to `timeout` for its result.
fn run_guarded<T: Send + 'static>(
    limiter: &'static Limiter,
    timeout: Duration,
    job: impl FnOnce() -> T + Send + 'static,
) -> Option<T> {
    let start = Instant::now();
    let rx = spawn_guarded(limiter, timeout, move |tx| {
        let _ = tx.send(job());
    })?;
    rx.recv_timeout(timeout.saturating_sub(start.elapsed())).ok()
}

/// A small valid PDF with `pages` pages of 200×150pt, for tests.
#[cfg(test)]
pub(crate) fn sample_pdf(pages: usize, title: Option<&str>) -> Vec<u8> {
    let first_page = 5;
    let kids: Vec<String> = (0..pages)
        .map(|i| format!("{} 0 R", first_page + 2 * i))
        .collect();
    let mut objects = vec![
        "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
        format!("<< /Type /Pages /Kids [{}] /Count {} >>", kids.join(" "), pages),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_string(),
        format!("<< /Title ({}) >>", title.unwrap_or("")),
    ];
    for i in 0..pages {
        let content = format!(
            "0 0 1 rg 20 20 160 60 re f BT /F1 24 Tf 30 100 Td (Page {}) Tj ET",
            i + 1
        );
        objects.push(format!(
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 200 150] \
             /Resources << /Font << /F1 3 0 R >> >> /Contents {} 0 R >>",
            first_page + 2 * i + 1
        ));
        objects.push(format!(
            "<< /Length {} >>\nstream\n{}\nendstream",
            content.len(),
            content
        ));
    }
    let mut out = String::from("%PDF-1.4\n");
    let mut offsets = Vec::new();
    for (n, body) in objects.iter().enumerate() {
        offsets.push(out.len());
        out += &format!("{} 0 obj\n{}\nendobj\n", n + 1, body);
    }
    let xref = out.len();
    out += &format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1);
    for off in offsets {
        out += &format!("{:010} 00000 n \n", off);
    }
    out += &format!(
        "trailer\n<< /Size {} /Root 1 0 R{} >>\nstartxref\n{}\n%%EOF\n",
        objects.len() + 1,
        if title.is_some() { " /Info 4 0 R" } else { "" },
        xref
    );
    out.into_bytes()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_page_count_and_title() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("doc.pdf");
        std::fs::write(&file, sample_pdf(3, Some("Quarterly Report"))).unwrap();
        assert_eq!(
            read_info(&file),
            DocumentInfo {
                page_count: Some(3),
                title: Some("Quarterly Report".to_string()),
            }
        );
    }

    #[test]
    fn untitled_and_broken_files() {
        let dir = tempfile::tempdir().unwrap();
        let untitled = dir.path().join("untitled.pdf");
        std::fs::write(&untitled, sample_pdf(1, None)).unwrap();
        assert_eq!(read_info(&untitled).page_count, Some(1));
        assert_eq!(read_info(&untitled).title, None);

        let broken = dir.path().join("broken.pdf");
        std::fs::write(&broken, b"%PDF-1.4\nnot really a pdf").unwrap();
        let info = read_info(&broken);
        assert_eq!(info.title, None);
        assert!(!render_first_page(&broken, &dir.path().join("x.png"), 64));
    }

    #[test]
    fn renders_first_page_within_size() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("doc.pdf");
        std::fs::write(&file, sample_pdf(2, None)).unwrap();
        let png = dir.path().join("page.png");
        assert!(render_first_page(&file, &png, 100));
        let img = image::open(&png).unwrap().to_rgb8();
        assert_eq!(img.dimensions(), (100, 75));
        // The blue rectangle made it onto the page, over a white background.
        assert_eq!(img.get_pixel(50, 55).0, [0, 0, 255]);
        assert_eq!(img.get_pixel(2, 2).0, [255, 255, 255]);
    }

    #[test]
    fn abandoned_workers_keep_their_place_until_they_exit() {
        static LIMIT: Limiter = Limiter::new(1);
        let short = Duration::from_millis(50);
        // Overruns its time limit and is abandoned, still running.
        let slow = run_guarded(&LIMIT, short, || {
            std::thread::sleep(Duration::from_millis(400));
            1
        });
        assert_eq!(slow, None);
        // Its place is still taken, so nothing else starts meanwhile.
        assert_eq!(run_guarded(&LIMIT, short, || 2), None);
        std::thread::sleep(Duration::from_millis(600));
        assert_eq!(run_guarded(&LIMIT, short, || 3), Some(3));
        // A panicking job gives its place back too.
        assert_eq!(run_guarded(&LIMIT, short, || -> i32 { panic!("bad page") }), None);
        assert_eq!(run_guarded(&LIMIT, Duration::from_secs(2), || 4), Some(4));
    }

    #[test]
    fn panel_inspection_reads_details_and_renders_once() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("doc.pdf");
        std::fs::write(&file, sample_pdf(2, Some("Once"))).unwrap();
        let png = dir.path().join("page.png");
        let (info, rendered) = inspect_and_render(&file, &png, 80);
        assert!(rendered);
        assert_eq!(
            info,
            Some(DocumentInfo {
                page_count: Some(2),
                title: Some("Once".to_string()),
            })
        );
        assert_eq!(image::image_dimensions(&png).unwrap(), (80, 60));
    }

    #[test]
    fn text_strings_decode() {
        assert_eq!(decode_text_string(b"Plain"), Some("Plain".to_string()));
        assert_eq!(
            decode_text_string(&[0xFE, 0xFF, 0x00, 0x48, 0x00, 0xE9]),
            Some("Hé".to_string())
        );
        assert_eq!(decode_text_string(b"caf\xe9"), Some("café".to_string()));
        assert_eq!(decode_text_string(b"   "), None);
    }

    #[test]
    fn supports_pdf_only() {
        let preview = PdfPreview::new();
        assert!(preview.supports("PDF"));
        assert!(!preview.supports("docx"));
    }
}
