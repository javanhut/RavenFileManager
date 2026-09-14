use std::cell::{Cell, RefCell};
use std::path::{Path, PathBuf};
use std::rc::{Rc, Weak};

use gtk4 as gtk;
use gtk::prelude::*;
use gtk::{gio, glib};

use raven_core::events::PreviewData;
use raven_core::path::RavenPath;

use crate::thumbnails::Slot;
use crate::widgets::file_list::{format_local_time, format_size};

/// Edge of the image shown for pictures. Larger than the panel so it stays
/// sharp when the panel is widened or on HiDPI screens.
const IMAGE_SIZE: u32 = 768;

/// How long a watched file may be missing before the panel lets it go. Long
/// enough for a save that renames the original aside and writes a new one.
const SAVE_GRACE: std::time::Duration = std::time::Duration::from_millis(400);

type OpenHandler = Rc<RefCell<Option<Box<dyn Fn(&RavenPath)>>>>;

/// The stream playing in the panel, and the handler watching it for errors,
/// which has to be disconnected before the stream is dropped.
struct InlineVideo {
    media: gtk::MediaFile,
    error_handler: glib::SignalHandlerId,
}

/// Preview panel for the selected file: text, image, video, PDF first page or
/// folder summary, with the file's type, size and date underneath and a
/// button to open it.
pub struct PreviewPanel {
    pub widget: gtk::Box,
    stack: gtk::Stack,
    text_view: gtk::TextView,
    image: gtk::Picture,
    /// "inline" (a player) or "frame" (a still with a play badge that opens
    /// the file externally, for GTK builds without a media backend).
    video_stack: gtk::Stack,
    video_player: gtk::Video,
    video_frame: gtk::Picture,
    video_caption: gtk::Label,
    /// The video shown in the panel, kept while the panel is hidden so its
    /// stream can be reopened when the panel comes back.
    video_file: RefCell<Option<PathBuf>>,
    inline_video: RefCell<Option<InlineVideo>>,
    document_page: gtk::Picture,
    document_caption: gtk::Label,
    dir_label: gtk::Label,
    unsupported_icon: gtk::Image,
    unsupported_label: gtk::Label,
    title_label: gtk::Label,
    /// Type, size and date as rows, rebuilt for each file.
    props: gtk::Box,
    open_btn: gtk::Button,
    /// The file the panel is waiting for. Previews of anything else arrive
    /// late from an earlier selection and are ignored.
    requested: RefCell<Option<RavenPath>>,
    shown: Rc<RefCell<Option<RavenPath>>>,
    /// Watches the shown file so the panel empties when it is deleted or
    /// moved away, whoever did it.
    monitor: RefCell<Option<gio::FileMonitor>>,
    image_slot: Slot,
    on_open: OpenHandler,
}

impl PreviewPanel {
    pub fn new() -> Rc<Self> {
        let widget = gtk::Box::new(gtk::Orientation::Vertical, 0);
        // A minimum only: the window lets the panel be dragged wider.
        widget.set_width_request(220);
        widget.add_css_class("preview-panel");

        let title_label = gtk::Label::new(Some("Preview"));
        title_label.add_css_class("preview-title");
        title_label.set_margin_top(14);
        title_label.set_margin_bottom(10);
        title_label.set_margin_start(14);
        title_label.set_margin_end(14);
        title_label.set_halign(gtk::Align::Start);
        title_label.set_wrap(true);
        title_label.set_wrap_mode(gtk::pango::WrapMode::WordChar);
        title_label.set_xalign(0.0);
        widget.append(&title_label);

        let stack = gtk::Stack::new();
        stack.set_vexpand(true);

        // Text
        let text_view = gtk::TextView::new();
        text_view.set_editable(false);
        text_view.set_cursor_visible(false);
        text_view.set_wrap_mode(gtk::WrapMode::WordChar);
        text_view.set_monospace(true);
        text_view.set_margin_start(8);
        text_view.set_margin_end(8);
        text_view.set_margin_top(8);
        let text_scroll = gtk::ScrolledWindow::builder()
            .hscrollbar_policy(gtk::PolicyType::Automatic)
            .vscrollbar_policy(gtk::PolicyType::Automatic)
            .child(&text_view)
            .build();
        stack.add_named(&text_scroll, Some("text"));

        // Image
        let image = gtk::Picture::new();
        image.set_can_shrink(true);
        image.set_content_fit(gtk::ContentFit::Contain);
        image.set_margin_start(8);
        image.set_margin_end(8);
        image.set_margin_top(8);
        image.set_margin_bottom(8);
        stack.add_named(&image, Some("image"));

        // Video: played in the panel when GTK can, otherwise a still frame
        // with a play badge that opens the file in the default player.
        let video_box = gtk::Box::new(gtk::Orientation::Vertical, 8);
        video_box.set_margin_start(8);
        video_box.set_margin_end(8);
        video_box.set_margin_top(8);
        let video_stack = gtk::Stack::new();
        let video_player = gtk::Video::new();
        // Nothing starts making sound just because it was selected.
        video_player.set_autoplay(false);
        video_player.set_loop(false);
        video_player.set_size_request(-1, 160);
        video_stack.add_named(&video_player, Some("inline"));
        let video_overlay = gtk::Overlay::new();
        let video_frame = gtk::Picture::new();
        video_frame.set_can_shrink(true);
        video_frame.set_content_fit(gtk::ContentFit::Contain);
        video_frame.set_size_request(-1, 160);
        video_overlay.set_child(Some(&video_frame));
        let play_badge = gtk::Button::from_icon_name("media-playback-start-symbolic");
        play_badge.add_css_class("osd");
        play_badge.add_css_class("circular");
        play_badge.set_halign(gtk::Align::Center);
        play_badge.set_valign(gtk::Align::Center);
        play_badge.set_tooltip_text(Some("Play"));
        video_overlay.add_overlay(&play_badge);
        video_stack.add_named(&video_overlay, Some("frame"));
        video_box.append(&video_stack);
        let video_caption = gtk::Label::new(None);
        video_caption.add_css_class("dim-label");
        video_box.append(&video_caption);
        stack.add_named(&video_box, Some("video"));

        // Document: the first page and what the file says about itself.
        let document_box = gtk::Box::new(gtk::Orientation::Vertical, 8);
        document_box.set_margin_start(8);
        document_box.set_margin_end(8);
        document_box.set_margin_top(8);
        document_box.set_margin_bottom(8);
        let document_page = gtk::Picture::new();
        document_page.set_can_shrink(true);
        document_page.set_content_fit(gtk::ContentFit::Contain);
        document_page.set_vexpand(true);
        document_page.add_css_class("document-page");
        document_page.set_overflow(gtk::Overflow::Hidden);
        document_box.append(&document_page);
        let document_caption = gtk::Label::new(None);
        document_caption.add_css_class("dim-label");
        document_caption.set_wrap(true);
        document_caption.set_wrap_mode(gtk::pango::WrapMode::WordChar);
        document_caption.set_justify(gtk::Justification::Center);
        document_box.append(&document_caption);
        stack.add_named(&document_box, Some("document"));

        // Directory
        let dir_label = gtk::Label::new(None);
        dir_label.set_wrap(true);
        dir_label.set_margin_start(16);
        dir_label.set_margin_end(16);
        dir_label.set_margin_top(16);
        dir_label.set_valign(gtk::Align::Start);
        stack.add_named(&dir_label, Some("directory"));

        // Unsupported: the type's icon at a size that still tells files apart.
        let unsupported_box = gtk::Box::new(gtk::Orientation::Vertical, 12);
        unsupported_box.set_valign(gtk::Align::Center);
        let unsupported_icon = gtk::Image::new();
        unsupported_icon.set_pixel_size(96);
        unsupported_box.append(&unsupported_icon);
        let unsupported_label = gtk::Label::new(Some("No preview available"));
        unsupported_label.add_css_class("dim-label");
        unsupported_label.set_justify(gtk::Justification::Center);
        unsupported_box.append(&unsupported_label);
        stack.add_named(&unsupported_box, Some("unsupported"));

        // Waiting for the backend
        let loading = gtk::Spinner::new();
        loading.set_spinning(true);
        loading.set_halign(gtk::Align::Center);
        loading.set_valign(gtk::Align::Center);
        stack.add_named(&loading, Some("loading"));

        let empty_label = gtk::Label::new(Some("Select a file to preview"));
        empty_label.add_css_class("dim-label");
        empty_label.set_valign(gtk::Align::Center);
        stack.add_named(&empty_label, Some("empty"));
        stack.set_visible_child_name("empty");

        // The preview sits on a card of its own; the card clips what it holds
        // to its rounded corners.
        let card = gtk::Box::new(gtk::Orientation::Vertical, 0);
        card.add_css_class("preview-card");
        card.set_overflow(gtk::Overflow::Hidden);
        card.set_margin_start(12);
        card.set_margin_end(12);
        card.set_vexpand(true);
        card.append(&stack);
        widget.append(&card);

        // Details as label/value rows, then the primary action.
        let footer = gtk::Box::new(gtk::Orientation::Vertical, 10);
        footer.set_margin_start(12);
        footer.set_margin_end(12);
        footer.set_margin_top(12);
        footer.set_margin_bottom(12);
        let props = gtk::Box::new(gtk::Orientation::Vertical, 0);
        props.add_css_class("props");
        props.set_visible(false);
        footer.append(&props);
        let open_btn = gtk::Button::with_label("Open");
        open_btn.add_css_class("suggested-action");
        open_btn.set_sensitive(false);
        footer.append(&open_btn);
        widget.append(&footer);

        let shown: Rc<RefCell<Option<RavenPath>>> = Rc::new(RefCell::new(None));
        let on_open: OpenHandler = Rc::new(RefCell::new(None));
        let open_shown = {
            let shown = shown.clone();
            let on_open = on_open.clone();
            move || {
                if let (Some(path), Some(handler)) = (shown.borrow().as_ref(), on_open.borrow().as_ref()) {
                    handler(path);
                }
            }
        };
        {
            let open_shown = open_shown.clone();
            open_btn.connect_clicked(move |_| open_shown());
        }
        play_badge.connect_clicked(move |_| open_shown());

        let panel = Rc::new(Self {
            widget,
            stack,
            text_view,
            image,
            video_stack,
            video_player,
            video_frame,
            video_caption,
            video_file: RefCell::new(None),
            inline_video: RefCell::new(None),
            document_page,
            document_caption,
            dir_label,
            unsupported_icon,
            unsupported_label,
            title_label,
            props,
            open_btn,
            requested: RefCell::new(None),
            shown,
            monitor: RefCell::new(None),
            image_slot: Slot::default(),
            on_open,
        });

        // A hidden panel -- toggled off, or its window closed -- holds no
        // open stream. The video comes back, paused, when the panel does.
        {
            let weak = Rc::downgrade(&panel);
            panel.widget.connect_unmap(move |_| {
                if let Some(panel) = weak.upgrade() {
                    panel.release_video();
                }
            });
        }
        {
            let weak = Rc::downgrade(&panel);
            panel.widget.connect_map(move |_| {
                let weak = weak.clone();
                // Deferred: showing the panel re-previews the selection right
                // after mapping it, and a selection that changed while it was
                // hidden forgets the video (stop_video) instead of this
                // opening a stream only to tear it down again.
                glib::idle_add_local_once(move || {
                    let Some(panel) = weak.upgrade() else { return };
                    if !panel.widget.is_mapped() {
                        return;
                    }
                    let file = panel.video_file.borrow().clone();
                    if let Some(file) = file {
                        if panel.inline_video.borrow().is_none() {
                            panel.show_video_stream(&file);
                        }
                    }
                });
            });
        }

        panel
    }

    /// What the Open and Play buttons do with the shown file.
    pub fn connect_open(&self, handler: impl Fn(&RavenPath) + 'static) {
        *self.on_open.borrow_mut() = Some(Box::new(handler));
    }

    /// The file the panel shows or is waiting for, if any.
    pub fn shown_path(&self) -> Option<RavenPath> {
        self.shown.borrow().clone()
    }

    /// Whether a preview of `path` is shown or already on its way.
    pub fn is_showing(&self, path: &RavenPath) -> bool {
        self.requested.borrow().as_ref() == Some(path)
    }

    /// Note that a preview of `path` has been asked for. Until it arrives the
    /// panel shows the name and details and a spinner.
    pub fn request(self: &Rc<Self>, path: &RavenPath) {
        if self.is_showing(path) {
            return;
        }
        self.stop_video();
        *self.requested.borrow_mut() = Some(path.clone());
        self.show_header(path);
        self.stack.set_visible_child_name("loading");
    }

    /// Update the panel with a finished preview. Ignored unless it is for the
    /// file most recently requested.
    pub fn set_preview(self: &Rc<Self>, path: &RavenPath, data: &PreviewData) {
        if self.requested.borrow().as_ref() != Some(path) {
            return;
        }
        self.stop_video();
        self.show_header(path);
        self.image_slot.clear();

        match data {
            PreviewData::Text { content, language: _ } => {
                self.text_view.buffer().set_text(content);
                self.stack.set_visible_child_name("text");
            }
            PreviewData::Image { path: file, .. } => {
                self.show_image(file);
            }
            PreviewData::Video {
                path: file,
                thumbnail,
                width,
                height,
                duration_secs,
            } => {
                let frame = thumbnail.as_deref().filter(|t| t.is_file());
                self.video_frame.set_filename(frame);
                self.video_caption
                    .set_text(&video_caption(*width, *height, *duration_secs));
                if media_backend_available() {
                    *self.video_file.borrow_mut() = Some(file.clone());
                    self.stack.set_visible_child_name("video");
                    if self.widget.is_mapped() {
                        self.show_video_stream(file);
                    }
                } else if frame.is_some() {
                    self.video_stack.set_visible_child_name("frame");
                    self.stack.set_visible_child_name("video");
                } else {
                    self.show_unsupported(file, "No frame available");
                }
            }
            PreviewData::Document {
                path: file,
                thumbnail,
                page_count,
                title,
            } => {
                let caption = document_caption(*page_count, title.as_deref());
                match thumbnail.as_deref().filter(|t| t.is_file()) {
                    Some(page) => {
                        self.document_page.set_filename(Some(page));
                        self.document_caption.set_text(&caption);
                        self.stack.set_visible_child_name("document");
                    }
                    None if caption.is_empty() => {
                        self.show_unsupported(file, "No preview available")
                    }
                    None => self.show_unsupported(file, &caption),
                }
            }
            PreviewData::Directory { item_count, total_size } => {
                self.dir_label.set_markup(&format!(
                    "<b>Folder</b>\n\nItems: {}\nTotal size: {}",
                    item_count,
                    format_size(*total_size)
                ));
                self.stack.set_visible_child_name("directory");
            }
            PreviewData::Unsupported { .. } => match path.as_local_path() {
                Some(file) => self.show_unsupported(file, "No preview available"),
                None => self.stack.set_visible_child_name("empty"),
            },
        }
    }

    pub fn clear(&self) {
        self.stop_video();
        *self.requested.borrow_mut() = None;
        *self.shown.borrow_mut() = None;
        self.watch(None);
        self.image_slot.clear();
        self.title_label.set_text("Preview");
        self.show_details("");
        self.open_btn.set_sensitive(false);
        self.stack.set_visible_child_name("empty");
    }

    fn show_header(self: &Rc<Self>, path: &RavenPath) {
        if self.shown.borrow().as_ref() != Some(path) {
            *self.shown.borrow_mut() = Some(path.clone());
            self.watch(path.as_local_path().map(|p| (p.as_path(), Rc::downgrade(self))));
        }
        self.title_label.set_text(path.file_name().unwrap_or("Unknown"));
        let local = path.as_local_path();
        let is_dir = local.is_some_and(|p| p.is_dir());
        self.show_details(&local.map(|p| file_details(p)).unwrap_or_default());
        self.open_btn.set_sensitive(local.is_some());
        let kind = local.and_then(|p| raven_preview::thumbnail::kind_for(p));
        self.open_btn.set_label(match kind {
            _ if is_dir => "Open Folder",
            Some(raven_preview::thumbnail::ThumbnailKind::Video) if media_backend_available() => {
                "Open in Player"
            }
            Some(raven_preview::thumbnail::ThumbnailKind::Video) => "Play",
            _ => "Open",
        });
    }

    /// Show `details` ("Key: value" lines) as property rows; empty hides them.
    fn show_details(&self, details: &str) {
        while let Some(child) = self.props.first_child() {
            self.props.remove(&child);
        }
        for (key, value) in detail_rows(details) {
            let row = gtk::Box::new(gtk::Orientation::Horizontal, 12);
            row.add_css_class("prop-row");
            let key_label = gtk::Label::new(Some(key));
            key_label.add_css_class("prop-key");
            key_label.set_xalign(0.0);
            row.append(&key_label);
            let value_label = gtk::Label::new(Some(value));
            value_label.add_css_class("prop-value");
            value_label.set_xalign(1.0);
            value_label.set_hexpand(true);
            value_label.set_wrap(true);
            value_label.set_wrap_mode(gtk::pango::WrapMode::WordChar);
            value_label.set_justify(gtk::Justification::Right);
            value_label.set_selectable(true);
            row.append(&value_label);
            self.props.append(&row);
        }
        self.props.set_visible(self.props.first_child().is_some());
    }

    /// Watch `target` for being deleted or moved away, replacing any earlier
    /// watch; `None` just stops watching.
    fn watch(&self, target: Option<(&Path, Weak<Self>)>) {
        if let Some(old) = self.monitor.borrow_mut().take() {
            old.cancel();
        }
        let Some((file, weak)) = target else {
            return;
        };
        let Ok(monitor) = gio::File::for_path(file)
            .monitor_file(gio::FileMonitorFlags::WATCH_MOVES, gio::Cancellable::NONE)
        else {
            return;
        };
        let watched = file.to_path_buf();
        monitor.connect_changed(move |_, changed, _other, event| {
            let gone = matches!(
                event,
                gio::FileMonitorEvent::Deleted
                    | gio::FileMonitorEvent::MovedOut
                    | gio::FileMonitorEvent::Renamed
            );
            // A rename *onto* the watched name (an editor saving) names the
            // temporary file here, not the watched one, and is not a loss.
            if !gone || changed.path().as_deref() != Some(watched.as_path()) {
                return;
            }
            if let Some(panel) = weak.upgrade() {
                let watched = watched.clone();
                // Deferred, and not just to leave the monitor that clear()
                // cancels: some programs save by renaming the original aside
                // and then writing a new file under its name, so the name
                // only counts as gone if it is still missing a moment later.
                glib::timeout_add_local_once(SAVE_GRACE, move || {
                    let still_shown = panel
                        .shown_path()
                        .and_then(|p| p.as_local_path().cloned())
                        .is_some_and(|p| p == watched && !p.exists());
                    if still_shown {
                        panel.clear();
                    }
                });
            }
        });
        *self.monitor.borrow_mut() = Some(monitor);
    }

    /// Open `file` in the inline player, paused. Falls back to the still
    /// frame if GTK cannot play it after all.
    fn show_video_stream(self: &Rc<Self>, file: &Path) {
        self.release_video();
        let media = gtk::MediaFile::for_filename(file);
        if media.error().is_some() || is_no_media_file(&media) {
            self.video_stack.set_visible_child_name("frame");
            return;
        }
        media.set_loop(false);
        let error_handler = {
            let weak = Rc::downgrade(self);
            media.connect_notify_local(Some("error"), move |media, _| {
                let Some(error) = media.error() else { return };
                tracing::debug!("Inline video playback unavailable: {}", error);
                if let Some(panel) = weak.upgrade() {
                    let failed = media.downgrade();
                    // Deferred: releasing the stream from its own notify
                    // handler would disconnect the handler mid-emission.
                    glib::idle_add_local_once(move || {
                        // Another video may have taken the player meanwhile;
                        // only the stream that failed falls back.
                        let still_current = match (failed.upgrade(), panel.inline_video.borrow().as_ref()) {
                            (Some(failed), Some(current)) => failed == current.media,
                            _ => false,
                        };
                        if still_current {
                            panel.release_video();
                            panel.video_stack.set_visible_child_name("frame");
                        }
                    });
                }
            })
        };
        self.video_player.set_media_stream(Some(&media));
        self.video_stack.set_visible_child_name("inline");
        *self.inline_video.borrow_mut() = Some(InlineVideo {
            media,
            error_handler,
        });
    }

    /// Stop and close the inline stream, keeping the panel on the same video.
    fn release_video(&self) {
        if let Some(InlineVideo {
            media,
            error_handler,
        }) = self.inline_video.borrow_mut().take()
        {
            media.disconnect(error_handler);
            media.set_playing(false);
            media.clear();
        }
        self.video_player
            .set_media_stream(None::<&gtk::MediaStream>);
    }

    /// Release the stream and forget the video, when showing something else.
    fn stop_video(&self) {
        self.release_video();
        *self.video_file.borrow_mut() = None;
    }

    fn show_image(&self, file: &Path) {
        self.stack.set_visible_child_name("image");
        // GTK renders vector images itself at any size.
        let applied = {
            let weak = self.image.downgrade();
            self.image_slot.request(file, IMAGE_SIZE, move |thumb| {
                if let Some(image) = weak.upgrade() {
                    image.set_filename(Some(thumb));
                }
            })
        };
        if !applied {
            self.image.set_filename(Some(file));
        }
    }

    fn show_unsupported(&self, file: &Path, message: &str) {
        let (gicon, description) = content_type_of(file);
        self.unsupported_icon.set_from_gicon(&gicon);
        self.unsupported_label
            .set_text(&format!("{}\n{}", message, description));
        self.stack.set_visible_child_name("unsupported");
    }
}

/// Whether this GTK can play media at all. Without a media module (no
/// GStreamer backend installed) every `GtkMediaFile` is a `GtkNoMediaFile`
/// that only ever reports an error, so the panel keeps to still frames.
fn media_backend_available() -> bool {
    thread_local! {
        static AVAILABLE: Cell<Option<bool>> = const { Cell::new(None) };
    }
    AVAILABLE.with(|cell| {
        *cell.get().get_or_insert_with(|| {
            let available = !is_no_media_file(&gtk::MediaFile::new());
            if !available {
                tracing::info!("GTK has no media backend; videos preview as still frames");
            }
            available
        })
    })
}

fn is_no_media_file(media: &gtk::MediaFile) -> bool {
    media.type_().name() == "GtkNoMediaFile"
}

/// True when a preview of `shown` still belongs in a pane showing `folder`:
/// both local and `shown` directly inside `folder`.
pub fn belongs_to_folder(shown: &RavenPath, folder: &RavenPath) -> bool {
    match (shown.as_local_path(), folder.as_local_path()) {
        (Some(shown), Some(folder)) => shown.parent() == Some(folder.as_path()),
        _ => false,
    }
}

fn content_type_of(file: &Path) -> (gio::Icon, String) {
    let (content_type, _uncertain) = gio::content_type_guess(Some(file), None);
    (
        gio::content_type_get_icon(&content_type),
        gio::content_type_get_description(&content_type).to_string(),
    )
}

/// Type, size and modification date, one per line.
fn file_details(file: &Path) -> String {
    let Ok(meta) = std::fs::metadata(file) else {
        return String::new();
    };
    let mut lines = Vec::new();
    if meta.is_dir() {
        lines.push("Type: Folder".to_string());
    } else {
        lines.push(format!("Type: {}", content_type_of(file).1));
        lines.push(format!("Size: {}", format_size(meta.len())));
    }
    if let Ok(modified) = meta.modified() {
        // The same local-time pattern as the list and Properties.
        let modified: chrono::DateTime<chrono::Utc> = modified.into();
        lines.push(format!("Modified: {}", format_local_time(&modified)));
    }
    lines.join("\n")
}

/// Split [`file_details`]' lines into (label, value) pairs. A line without a
/// label keeps its text as the value.
fn detail_rows(details: &str) -> Vec<(&str, &str)> {
    details
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| line.split_once(": ").unwrap_or(("", line)))
        .collect()
}

fn video_caption(width: u32, height: u32, duration_secs: Option<f64>) -> String {
    let mut parts = Vec::new();
    if width > 0 && height > 0 {
        parts.push(format!("{}×{}", width, height));
    }
    if let Some(secs) = duration_secs.filter(|s| s.is_finite() && *s >= 0.0) {
        let secs = secs.round() as u64;
        parts.push(if secs >= 3600 {
            format!("{}:{:02}:{:02}", secs / 3600, secs / 60 % 60, secs % 60)
        } else {
            format!("{}:{:02}", secs / 60, secs % 60)
        });
    }
    parts.join("  ·  ")
}

/// The document's title over its page count, whichever are known.
fn document_caption(page_count: Option<u32>, title: Option<&str>) -> String {
    let mut lines = Vec::new();
    if let Some(title) = title.map(str::trim).filter(|t| !t.is_empty()) {
        lines.push(title.to_string());
    }
    match page_count {
        Some(1) => lines.push("1 page".to_string()),
        Some(n) => lines.push(format!("{} pages", n)),
        None => {}
    }
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::{belongs_to_folder, detail_rows, document_caption, video_caption};
    use raven_core::path::RavenPath;

    #[test]
    fn details_split_into_label_and_value() {
        assert_eq!(
            detail_rows("Type: PNG image\nSize: 1.2 MB\n\nModified: 01/02/26 10:00:00"),
            vec![("Type", "PNG image"), ("Size", "1.2 MB"), ("Modified", "01/02/26 10:00:00")]
        );
        assert_eq!(detail_rows("loose text"), vec![("", "loose text")]);
        assert!(detail_rows("").is_empty());
    }

    #[test]
    fn caption_formats_size_and_duration() {
        assert_eq!(video_caption(1920, 1080, Some(61.4)), "1920×1080  ·  1:01");
        assert_eq!(video_caption(0, 0, Some(3725.0)), "1:02:05");
        assert_eq!(video_caption(640, 360, None), "640×360");
    }

    #[test]
    fn document_caption_shows_what_is_known() {
        assert_eq!(document_caption(Some(12), Some("Report")), "Report\n12 pages");
        assert_eq!(document_caption(Some(1), None), "1 page");
        assert_eq!(document_caption(None, Some("  ")), "");
    }

    #[test]
    fn preview_belongs_only_to_its_own_folder() {
        let folder = RavenPath::local("/home/u/docs");
        assert!(belongs_to_folder(&RavenPath::local("/home/u/docs/a.pdf"), &folder));
        assert!(!belongs_to_folder(&RavenPath::local("/home/u/docs/sub/a.pdf"), &folder));
        assert!(!belongs_to_folder(&RavenPath::local("/home/u/a.pdf"), &folder));
    }
}
