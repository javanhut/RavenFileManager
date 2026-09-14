//! Development aid: with `RAVEN_FM_SNAPSHOT=<dir>`, walk the window through
//! its main states, save each as a PNG in that directory, and quit.
//!
//! Lets the look be checked from a shell -- on a headless Broadway display,
//! say -- without anyone clicking through it. Nothing here runs unless the
//! variable is set. Optional: `RAVEN_FM_SNAPSHOT_MEDIA=<folder>` names the
//! folder with images and text files the preview states use (default:
//! `~/Pictures`, else the home folder).
//!
//! The steps press the same buttons a person would, so the view mode and the
//! preview panel's state are put back in the config before exiting.

use std::any::Any;
use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::Duration;

use gtk4 as gtk;
use libadwaita as adw;
use libadwaita::prelude::*;

use raven_core::commands::AppCommand;
use raven_core::path::RavenPath;
use raven_dbus::portal::{ChooserMode, ChooserOptions};

use super::{get_primary_selected_entry, RavenWindow};
use crate::widgets::app_chooser_dialog::show_app_chooser;
use crate::widgets::connect_dialog::ConnectDialog;
use crate::widgets::file_chooser_dialog::{DialogRequest, FileChooserDialog};
use crate::widgets::file_list::{FileEntryObject, FileListView};
use crate::widgets::properties_dialog::PropertiesDialog;
use crate::widgets::settings_dialog::SettingsDialog;

/// What a step's picture is of.
enum Shot {
    /// The main window.
    Window,
    /// The main window with the active pane's context menu drawn over it.
    WithMenu,
    /// The newest window other than the main one, closed afterwards unless
    /// the next step still needs it.
    Dialog { keep: bool },
}

struct Step {
    name: &'static str,
    /// How long the step's action gets to take effect: a listing to load,
    /// thumbnails and previews to arrive.
    settle: Duration,
    shot: Shot,
    act: Box<dyn Fn(&Snapshots)>,
}

struct Snapshots {
    window: Rc<RavenWindow>,
    app: adw::Application,
    dir: PathBuf,
    home: PathBuf,
    media: PathBuf,
    settings: RefCell<Option<SettingsDialog>>,
    /// The main window as drawn just before a menu was opened over it.
    backdrop: RefCell<Option<gtk::gdk::Texture>>,
    /// Dialogs whose owners must outlive the step that opened them.
    keep: RefCell<Vec<Box<dyn Any>>>,
}

impl RavenWindow {
    /// Start the walk-through; see the module documentation.
    pub fn start_snapshots(self: &Rc<Self>, app: &adw::Application, dir: PathBuf) {
        let home = std::env::var_os("HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("/"));
        let media = std::env::var_os("RAVEN_FM_SNAPSHOT_MEDIA")
            .map(PathBuf::from)
            .filter(|p| p.is_dir())
            .or_else(|| Some(home.join("Pictures")).filter(|p| p.is_dir()))
            .unwrap_or_else(|| home.clone());
        if let Err(e) = std::fs::create_dir_all(&dir) {
            tracing::error!("Snapshot directory {}: {}", dir.display(), e);
            std::process::exit(1);
        }
        tracing::info!("Snapshot mode: writing to {}", dir.display());
        // Transitions only advance as frames are shown, which a headless
        // display may never do; without them each state lands at once.
        if let Some(settings) = gtk::Settings::default() {
            settings.set_gtk_enable_animations(false);
        }

        let original = self.state.borrow().config.appearance.clone();
        let snapshots = Rc::new(Snapshots {
            window: self.clone(),
            app: app.clone(),
            dir,
            home,
            media,
            settings: RefCell::new(None),
            backdrop: RefCell::new(None),
            keep: RefCell::new(Vec::new()),
        });
        let steps = Rc::new(steps());
        let finish = Rc::new(move |snapshots: &Snapshots| {
            let mut s = snapshots.window.state.borrow_mut();
            s.config.appearance = original.clone();
            let _ = s.config.save();
            drop(s);
            tracing::info!("Snapshot mode: done");
            std::process::exit(0);
        });
        // The window needs a moment to map and lay out before the first step.
        glib::timeout_add_local_once(Duration::from_millis(1500), move || {
            run(snapshots, steps, 0, finish);
        });
    }
}

fn run(
    snapshots: Rc<Snapshots>,
    steps: Rc<Vec<Step>>,
    index: usize,
    finish: Rc<dyn Fn(&Snapshots)>,
) {
    let Some(step) = steps.get(index) else {
        finish(&snapshots);
        return;
    };
    (step.act)(&snapshots);
    let settle = step.settle;
    glib::timeout_add_local_once(settle, move || {
        snapshots.capture(index, &steps[index]);
        run(snapshots, steps, index + 1, finish);
    });
}

fn steps() -> Vec<Step> {
    let step = |name, settle_ms, shot, act: Box<dyn Fn(&Snapshots)>| Step {
        name,
        settle: Duration::from_millis(settle_ms),
        shot,
        act,
    };
    vec![
        step("list-home", 2500, Shot::Window, Box::new(|s| {
            s.view_mode(0);
            s.go(&s.home.clone());
        })),
        step("icons-home", 1500, Shot::Window, Box::new(|s| s.view_mode(1))),
        step("previews-media", 3500, Shot::Window, Box::new(|s| {
            s.go(&s.media.clone());
            s.view_mode(2);
        })),
        step("preview-image", 3000, Shot::Window, Box::new(|s| {
            s.window.preview_btn.set_active(true);
            if !select_first(&s.list(), is_image) {
                tracing::warn!("No image in {} to preview", s.media.display());
            }
        })),
        step("preview-text", 2000, Shot::Window, Box::new(|s| {
            if !select_first(&s.list(), is_text) {
                tracing::warn!("No text file in {} to preview", s.media.display());
            }
        })),
        step("context-menu", 1200, Shot::WithMenu, Box::new(|s| {
            // The window is drawn now, before the menu opens: rendered while
            // a popup holds the grab, it comes out empty.
            let main: gtk::Window = s.window.window.clone().upcast();
            *s.backdrop.borrow_mut() = render(&main, None);
            let view = s.window.views.active();
            let popover = &view.context_menu.popover;
            let rect = gtk::gdk::Rectangle::new(220, 140, 1, 1);
            popover.set_pointing_to(Some(&rect));
            popover.popup();
        })),
        step("dual-pane", 2500, Shot::Window, Box::new(|s| {
            s.window.preview_btn.set_active(false);
            s.view_mode(0);
            s.window.views.toggle_dual();
        })),
        step("settings-general", 1500, Shot::Dialog { keep: true }, Box::new(|s| {
            if s.window.views.is_dual() {
                s.window.views.toggle_dual();
            }
            let dialog = SettingsDialog::new(
                &s.window.window,
                s.window.state.clone(),
                s.window.command_tx.clone(),
            );
            dialog.present();
            *s.settings.borrow_mut() = Some(dialog);
        })),
        step("settings-appearance", 1200, Shot::Dialog { keep: false }, Box::new(|s| {
            if let Some(dialog) = s.settings.borrow().as_ref() {
                dialog.show_page("appearance");
            }
        })),
        step("properties", 1500, Shot::Dialog { keep: false }, Box::new(|s| {
            let entry = get_primary_selected_entry(&s.list()).or_else(|| {
                s.list()
                    .selection
                    .item(0)
                    .and_downcast::<FileEntryObject>()
                    .and_then(|o| o.entry())
            });
            if let Some(entry) = entry {
                let dialog = PropertiesDialog::new(&s.window.window, &entry, &s.window.command_tx);
                dialog.borrow().present();
                s.keep.borrow_mut().push(Box::new(dialog));
            }
        })),
        step("connect-server", 1200, Shot::Dialog { keep: false }, Box::new(|s| {
            let dialog = ConnectDialog::new(&s.window.window, s.window.command_tx.clone());
            dialog.present();
            s.keep.borrow_mut().push(Box::new(dialog));
        })),
        step("app-chooser", 1500, Shot::Dialog { keep: false }, Box::new(|s| {
            show_app_chooser(&s.window.window, "image/png", |_, _| {});
        })),
        step("portal-open", 2000, Shot::Dialog { keep: false }, Box::new(|s| {
            s.chooser(ChooserMode::Open, ChooserOptions::default());
        })),
        step("portal-save", 2000, Shot::Dialog { keep: false }, Box::new(|s| {
            let options = ChooserOptions {
                current_name: Some("Notes.txt".to_string()),
                ..ChooserOptions::default()
            };
            s.chooser(ChooserMode::Save, options);
        })),
    ]
}

impl Snapshots {
    fn list(&self) -> Rc<FileListView> {
        self.window.views.active().file_list.clone()
    }

    /// Press the list (0), icon (1) or preview (2) view button.
    fn view_mode(&self, index: usize) {
        self.window.view_buttons[index].set_active(true);
    }

    fn go(&self, path: &Path) {
        let path = RavenPath::local(path.to_path_buf());
        let pane_id = self.window.views.active_pane_id();
        if let Some(pane) = self.window.state.borrow_mut().pane_by_id_mut(pane_id) {
            pane.navigate_to(path.clone());
        }
        let _ = self
            .window
            .command_tx
            .send(AppCommand::Navigate { path, pane_id });
    }

    /// The portal's picker, built directly rather than over D-Bus.
    fn chooser(&self, mode: ChooserMode, options: ChooserOptions) {
        let dialog = FileChooserDialog::present(
            &self.app,
            DialogRequest {
                mode,
                app_id: "org.example.Editor".to_string(),
                title: String::new(),
                options,
            },
            |_| {},
        );
        self.keep.borrow_mut().push(Box::new(dialog));
    }

    fn capture(&self, index: usize, step: &Step) {
        let file = self.dir.join(format!("{:02}-{}.png", index + 1, step.name));
        let main: gtk::Window = self.window.window.clone().upcast();
        match step.shot {
            Shot::Window => save(render(&main, None), &file),
            Shot::WithMenu => {
                let popover = self.window.views.active().context_menu.popover.clone();
                let backdrop = self.backdrop.borrow_mut().take();
                save(render(&main, Some((backdrop, &popover))), &file);
                popover.popdown();
            }
            Shot::Dialog { keep } => match newest_dialog(&main) {
                Some(dialog) => {
                    save(render(&dialog, None), &file);
                    if !keep {
                        dialog.close();
                    }
                }
                None => tracing::warn!("{}: no dialog to capture", step.name),
            },
        }
        tracing::info!("Snapshot {}", file.display());
    }
}

/// The most recently created visible window other than `main`.
fn newest_dialog(main: &gtk::Window) -> Option<gtk::Window> {
    let toplevels = gtk::Window::toplevels();
    (0..toplevels.n_items())
        .rev()
        .filter_map(|i| toplevels.item(i).and_downcast::<gtk::Window>())
        .find(|w| w != main && w.is_visible())
}

fn save(texture: Option<gtk::gdk::Texture>, file: &Path) {
    let Some(texture) = texture else {
        tracing::warn!("Nothing to render for {}", file.display());
        return;
    };
    if let Err(e) = texture.save_to_png(file) {
        tracing::warn!("Could not write {}: {}", file.display(), e);
    }
}

/// Render `window`. With a menu, the backdrop (the window drawn before the
/// menu opened, or the window now if there is none) goes underneath and the
/// menu is drawn over it where it sits on screen.
fn render(
    window: &gtk::Window,
    menu: Option<(Option<gtk::gdk::Texture>, &gtk::PopoverMenu)>,
) -> Option<gtk::gdk::Texture> {
    let (width, height) = (window.width() as f32, window.height() as f32);
    let bounds = gtk::graphene::Rect::new(0.0, 0.0, width, height);
    let snapshot = gtk::Snapshot::new();
    match menu.as_ref().and_then(|(backdrop, _)| backdrop.as_ref()) {
        Some(backdrop) => snapshot.append_texture(backdrop, &bounds),
        None => gtk::WidgetPaintable::new(Some(window)).snapshot(
            &snapshot,
            width as f64,
            height as f64,
        ),
    }
    if let Some((popover, (x, y))) = menu
        .as_ref()
        .and_then(|(_, p)| popover_origin(window, p).map(|origin| (*p, origin)))
    {
        snapshot.save();
        snapshot.translate(&gtk::graphene::Point::new(x as f32, y as f32));
        gtk::WidgetPaintable::new(Some(popover)).snapshot(
            &snapshot,
            popover.width() as f64,
            popover.height() as f64,
        );
        snapshot.restore();
    }
    // The window's own bounds, so a popover's shadow reaching past an edge
    // neither grows nor shifts the picture.
    let node = snapshot.to_node()?;
    Some(window.renderer()?.render_texture(node, Some(&bounds)))
}

/// Where a popover's content is, in its window's coordinates: its popup
/// surface's offset from the window's surface, less the shadow margins each
/// surface draws around its widget.
fn popover_origin(window: &gtk::Window, popover: &gtk::PopoverMenu) -> Option<(f64, f64)> {
    if !popover.is_mapped() {
        return None;
    }
    let popup = popover.surface()?.downcast::<gtk::gdk::Popup>().ok()?;
    let (px, py) = popover.surface_transform();
    let (wx, wy) = window.surface_transform();
    Some((
        popup.position_x() as f64 + px - wx,
        popup.position_y() as f64 + py - wy,
    ))
}

/// Select the first file in `list` whose extension passes `wanted`, and
/// scroll it into view.
fn select_first(list: &FileListView, wanted: fn(&str) -> bool) -> bool {
    for i in 0..list.selection.n_items() {
        let Some(entry) = list
            .selection
            .item(i)
            .and_downcast::<FileEntryObject>()
            .and_then(|o| o.entry())
        else {
            continue;
        };
        let matches = !entry.is_dir()
            && entry
                .extension()
                .is_some_and(|ext| wanted(&ext.to_ascii_lowercase()));
        if matches {
            list.select_paths(&[entry.path.clone()]);
            return true;
        }
    }
    false
}

fn is_image(ext: &str) -> bool {
    matches!(ext, "png" | "jpg" | "jpeg" | "webp" | "gif" | "bmp")
}

fn is_text(ext: &str) -> bool {
    matches!(
        ext,
        "txt" | "md" | "rs" | "toml" | "json" | "sh" | "conf" | "log" | "csv" | "py" | "ini"
    )
}
