use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

use gtk4 as gtk;
use gtk::prelude::*;
use libadwaita as adw;

use gio::prelude::*;
use libadwaita::prelude::*;

use raven_core::commands::AppCommand;
use raven_core::config::AppConfig;
use raven_core::events::AppEvent;
use raven_core::path::RavenPath;

use crate::state::AppState;
use crate::themes;
use crate::window::RavenWindow;

pub const APP_ID: &str = "com.ravenfilemanager.Raven";

/// What the two entry points -- activate and open -- share: the window, or the
/// means to create it, plus the channels it talks over.
struct Session {
    state: AppState,
    command_tx: tokio::sync::mpsc::UnboundedSender<AppCommand>,
    window: RefCell<Option<Rc<RavenWindow>>>,
    event_rx: RefCell<Option<tokio::sync::mpsc::UnboundedReceiver<AppEvent>>>,
    theme_loaded: RefCell<bool>,
}

impl Session {
    /// The window, created on first use and returned as-is after that.
    ///
    /// GApplication routes a second launch of the app into the process that is
    /// already running, so opening a folder from the desktop and revealing a
    /// file over D-Bus both arrive here and reuse the open window instead of
    /// starting another copy of the file manager.
    fn window(&self, app: &adw::Application) -> Rc<RavenWindow> {
        if let Some(window) = self.window.borrow().as_ref() {
            return window.clone();
        }

        // CSS needs a display, which exists only once the app is running.
        if !*self.theme_loaded.borrow() {
            *self.theme_loaded.borrow_mut() = true;
            let theme = self.state.borrow().config.appearance.theme;
            themes::load_base_css();
            themes::apply_theme(theme);
        }

        let window = Rc::new(RavenWindow::new(
            app,
            self.state.clone(),
            self.command_tx.clone(),
        ));
        window.present();
        *self.window.borrow_mut() = Some(window.clone());

        // Started here rather than at the call site so it is running before
        // either entry point sends the command whose events it has to receive.
        if let Some(rx) = self.event_rx.borrow_mut().take() {
            let window = window.clone();
            glib::spawn_future_local(async move {
                let mut rx = rx;
                while let Some(event) = rx.recv().await {
                    window.handle_event(event);
                }
            });
        }

        window
    }

    fn active_pane_id(&self) -> u32 {
        self.state.borrow().active_tab().active_pane().id
    }

    /// Show the directory the pane starts on. This is a plain launch, with no
    /// location named by whoever started us.
    fn open_initial_directory(&self) {
        let (path, pane_id) = {
            let state = self.state.borrow();
            let pane = state.active_tab().active_pane();
            (pane.current_path.clone(), pane.id)
        };
        let _ = self.command_tx.send(AppCommand::Navigate { path, pane_id });
    }

    /// Show locations handed to us by the desktop -- `ravenfilemanager <path>`,
    /// or an application launching the `%U` desktop entry.
    fn open_locations(&self, files: &[gio::File]) {
        let paths: Vec<PathBuf> = files
            .iter()
            .filter_map(|file| match file.path() {
                Some(path) => Some(path),
                None => {
                    // A gio::File can name a gvfs location with no local path.
                    tracing::warn!("Ignoring location with no local path: {}", file.uri());
                    None
                }
            })
            .collect();

        let Some(first) = paths.first() else {
            // Nothing usable was named; fall back to a plain launch rather than
            // presenting an empty window.
            self.open_initial_directory();
            return;
        };

        let pane_id = self.active_pane_id();

        if first.is_dir() {
            let _ = self.command_tx.send(AppCommand::Navigate {
                path: RavenPath::local(first.clone()),
                pane_id,
            });
        } else {
            // A file was named, so what was meant is "show me this one" -- the
            // same request ShowItems makes over D-Bus, and the same handling.
            let _ = self.command_tx.send(AppCommand::RevealItems {
                paths: paths.iter().cloned().map(RavenPath::local).collect(),
                pane_id,
                show_properties: false,
            });
        }
    }
}

pub struct RavenApplication {
    app: adw::Application,
    command_tx: tokio::sync::mpsc::UnboundedSender<AppCommand>,
    event_rx: Option<tokio::sync::mpsc::UnboundedReceiver<AppEvent>>,
    state: AppState,
}

impl RavenApplication {
    pub fn new(
        command_tx: tokio::sync::mpsc::UnboundedSender<AppCommand>,
        event_rx: tokio::sync::mpsc::UnboundedReceiver<AppEvent>,
    ) -> Self {
        // HANDLES_OPEN because the desktop entry is `Exec=ravenfilemanager %U`
        // and a file manager is above all the thing other applications hand a
        // location to. Without the flag GApplication refuses the arguments
        // outright -- "This application can not open files" -- so every launch
        // that named a folder failed, which is most of the ways one gets here.
        let app = adw::Application::builder()
            .application_id(APP_ID)
            .flags(gio::ApplicationFlags::HANDLES_OPEN)
            .build();

        let config = AppConfig::load();
        let state = AppState::new(config);

        Self {
            app,
            command_tx,
            event_rx: Some(event_rx),
            state,
        }
    }

    pub fn run(mut self) -> glib::ExitCode {
        let session = Rc::new(Session {
            state: self.state.clone(),
            command_tx: self.command_tx.clone(),
            window: RefCell::new(None),
            event_rx: RefCell::new(self.event_rx.take()),
            theme_loaded: RefCell::new(false),
        });

        {
            let session = session.clone();
            self.app.connect_activate(move |app| {
                session.window(app);
                session.open_initial_directory();
            });
        }

        {
            let session = session.clone();
            self.app.connect_open(move |app, files, _hint| {
                session.window(app);
                session.open_locations(files);
            });
        }

        self.app.run()
    }
}
