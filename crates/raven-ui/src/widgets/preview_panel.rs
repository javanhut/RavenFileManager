use std::cell::RefCell;
use std::path::Path;
use std::rc::Rc;

use gtk4 as gtk;
use gtk::prelude::*;
use gtk::{gio, glib};

use raven_core::events::PreviewData;
use raven_core::path::RavenPath;

use crate::thumbnails::Slot;
use crate::widgets::file_list::format_size;

/// Edge of the image shown for pictures. Larger than the panel so it stays
/// sharp when the panel is widened or on HiDPI screens.
const IMAGE_SIZE: u32 = 768;

type OpenHandler = Rc<RefCell<Option<Box<dyn Fn(&RavenPath)>>>>;

/// Preview panel for the selected file: text, image, video frame or folder
/// summary, with the file's type, size and date underneath and a button to
/// open it.
pub struct PreviewPanel {
    pub widget: gtk::Box,
    stack: gtk::Stack,
    text_view: gtk::TextView,
    image: gtk::Picture,
    video_frame: gtk::Picture,
    video_caption: gtk::Label,
    dir_label: gtk::Label,
    unsupported_icon: gtk::Image,
    unsupported_label: gtk::Label,
    title_label: gtk::Label,
    info_label: gtk::Label,
    open_btn: gtk::Button,
    /// The file the panel is waiting for. Previews of anything else arrive
    /// late from an earlier selection and are ignored.
    requested: RefCell<Option<RavenPath>>,
    shown: Rc<RefCell<Option<RavenPath>>>,
    image_slot: Slot,
    on_open: OpenHandler,
}

impl PreviewPanel {
    pub fn new() -> Self {
        let widget = gtk::Box::new(gtk::Orientation::Vertical, 0);
        widget.set_width_request(300);
        widget.add_css_class("preview-panel");

        let title_label = gtk::Label::new(Some("Preview"));
        title_label.add_css_class("heading");
        title_label.set_margin_top(8);
        title_label.set_margin_bottom(8);
        title_label.set_margin_start(8);
        title_label.set_margin_end(8);
        title_label.set_halign(gtk::Align::Start);
        title_label.set_wrap(true);
        title_label.set_wrap_mode(gtk::pango::WrapMode::WordChar);
        title_label.set_xalign(0.0);
        widget.append(&title_label);

        widget.append(&gtk::Separator::new(gtk::Orientation::Horizontal));

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

        // Video: a still frame with a play badge; clicking it plays the file.
        let video_box = gtk::Box::new(gtk::Orientation::Vertical, 8);
        video_box.set_margin_start(8);
        video_box.set_margin_end(8);
        video_box.set_margin_top(8);
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
        video_box.append(&video_overlay);
        let video_caption = gtk::Label::new(None);
        video_caption.add_css_class("dim-label");
        video_box.append(&video_caption);
        stack.add_named(&video_box, Some("video"));

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

        widget.append(&stack);

        // Details and the open button
        let footer = gtk::Box::new(gtk::Orientation::Vertical, 6);
        footer.set_margin_start(8);
        footer.set_margin_end(8);
        footer.set_margin_top(8);
        footer.set_margin_bottom(8);
        let info_label = gtk::Label::new(None);
        info_label.set_xalign(0.0);
        info_label.set_wrap(true);
        info_label.set_selectable(true);
        info_label.add_css_class("caption");
        footer.append(&info_label);
        let open_btn = gtk::Button::with_label("Open");
        open_btn.add_css_class("suggested-action");
        open_btn.set_sensitive(false);
        footer.append(&open_btn);
        widget.append(&gtk::Separator::new(gtk::Orientation::Horizontal));
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

        Self {
            widget,
            stack,
            text_view,
            image,
            video_frame,
            video_caption,
            dir_label,
            unsupported_icon,
            unsupported_label,
            title_label,
            info_label,
            open_btn,
            requested: RefCell::new(None),
            shown,
            image_slot: Slot::default(),
            on_open,
        }
    }

    /// What the Open and Play buttons do with the shown file.
    pub fn connect_open(&self, handler: impl Fn(&RavenPath) + 'static) {
        *self.on_open.borrow_mut() = Some(Box::new(handler));
    }

    /// Note that a preview of `path` has been asked for. Until it arrives the
    /// panel shows the name and details and a spinner.
    pub fn request(&self, path: &RavenPath) {
        if self.requested.borrow().as_ref() == Some(path) {
            return;
        }
        *self.requested.borrow_mut() = Some(path.clone());
        self.show_header(path);
        self.stack.set_visible_child_name("loading");
    }

    /// Update the panel with a finished preview. Ignored unless it is for the
    /// file most recently requested.
    pub fn set_preview(&self, path: &RavenPath, data: &PreviewData) {
        if self.requested.borrow().as_ref() != Some(path) {
            return;
        }
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
                self.video_frame
                    .set_filename(thumbnail.as_deref().filter(|t| t.is_file()));
                self.video_caption
                    .set_text(&video_caption(*width, *height, *duration_secs));
                if thumbnail.as_deref().is_some_and(Path::is_file) {
                    self.stack.set_visible_child_name("video");
                } else {
                    self.show_unsupported(file, "No frame available");
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
        *self.requested.borrow_mut() = None;
        *self.shown.borrow_mut() = None;
        self.image_slot.clear();
        self.title_label.set_text("Preview");
        self.info_label.set_text("");
        self.open_btn.set_sensitive(false);
        self.stack.set_visible_child_name("empty");
    }

    fn show_header(&self, path: &RavenPath) {
        *self.shown.borrow_mut() = Some(path.clone());
        self.title_label.set_text(path.file_name().unwrap_or("Unknown"));
        let local = path.as_local_path();
        let is_dir = local.is_some_and(|p| p.is_dir());
        self.info_label
            .set_text(&local.map(|p| file_details(p)).unwrap_or_default());
        self.open_btn.set_sensitive(local.is_some());
        let kind = local.and_then(|p| raven_preview::thumbnail::kind_for(p));
        self.open_btn.set_label(match kind {
            _ if is_dir => "Open Folder",
            Some(raven_preview::thumbnail::ThumbnailKind::Video) => "Play",
            _ => "Open",
        });
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
        let secs = modified
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        if let Some(date) = glib::DateTime::from_unix_local(secs)
            .ok()
            .and_then(|d| d.format("%x %X").ok())
        {
            lines.push(format!("Modified: {}", date));
        }
    }
    lines.join("\n")
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

#[cfg(test)]
mod tests {
    use super::video_caption;

    #[test]
    fn caption_formats_size_and_duration() {
        assert_eq!(video_caption(1920, 1080, Some(61.4)), "1920×1080  ·  1:01");
        assert_eq!(video_caption(0, 0, Some(3725.0)), "1:02:05");
        assert_eq!(video_caption(640, 360, None), "640×360");
    }
}
