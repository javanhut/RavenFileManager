use std::cell::RefCell;

use gtk4 as gtk;
use gtk::prelude::*;
use libadwaita as adw;

use raven_core::commands::AppCommand;
use raven_core::config::AppConfig;
use raven_core::events::AppEvent;

use crate::state::AppState;
use crate::window::RavenWindow;

pub const APP_ID: &str = "com.ravenfilemanager.Raven";

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
        let app = adw::Application::builder()
            .application_id(APP_ID)
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
        let command_tx = self.command_tx.clone();
        let state = self.state.clone();

        // Wrap event_rx in RefCell so the Fn closure can consume it once
        let event_rx = RefCell::new(self.event_rx.take());

        self.app.connect_activate(move |app| {
            let window = RavenWindow::new(app, state.clone(), command_tx.clone());
            window.present();

            // Kick off initial directory load
            let initial_path = {
                let s = state.borrow();
                s.active_tab().active_pane().current_path.clone()
            };
            let pane_id = {
                let s = state.borrow();
                s.active_tab().active_pane().id
            };
            let _ = command_tx.send(AppCommand::Navigate {
                path: initial_path,
                pane_id,
            });

            // Spawn event receiver on GLib main loop (only once, on first activation)
            if let Some(rx) = event_rx.borrow_mut().take() {
                glib::spawn_future_local(async move {
                    let mut rx = rx;
                    while let Some(event) = rx.recv().await {
                        window.handle_event(event);
                    }
                });
            }
        });

        self.app.run()
    }
}
