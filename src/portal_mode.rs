//! `ravenfilemanager --portal`: the xdg-desktop-portal FileChooser backend.
//!
//! D-Bus activation starts the binary this way when xdg-desktop-portal
//! forwards an open or save dialog to `org.freedesktop.impl.portal.desktop.raven`.
//! No file manager window and none of the file manager backend is started:
//! a Tokio thread owns the bus name and forwards each call to the GTK thread,
//! which shows one picker per call. The process exits after a minute with no
//! dialog open; the next request activates it again.

use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Duration;

use gio::prelude::*;
use libadwaita as adw;

use raven_core::config::AppConfig;
use raven_dbus::portal::ChooserRequest;
use raven_ui::widgets::file_chooser_dialog::{DialogRequest, FileChooserDialog};

const IDLE_EXIT: Duration = Duration::from_secs(60);

pub fn run() -> glib::ExitCode {
    let (request_tx, request_rx) = tokio::sync::mpsc::unbounded_channel::<ChooserRequest>();
    let (ready_tx, ready_rx) = std::sync::mpsc::channel::<bool>();

    std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .build()
            .expect("Failed to create Tokio runtime");
        rt.block_on(async move {
            match raven_dbus::portal::serve(request_tx).await {
                Ok(_connection) => {
                    tracing::info!("Serving {}", raven_dbus::portal::PORTAL_BUS_NAME);
                    let _ = ready_tx.send(true);
                    std::future::pending::<()>().await;
                }
                Err(e) => {
                    tracing::error!(
                        "Could not serve {}: {}",
                        raven_dbus::portal::PORTAL_BUS_NAME,
                        e
                    );
                    let _ = ready_tx.send(false);
                }
            }
        });
    });

    if !ready_rx.recv().unwrap_or(false) {
        return glib::ExitCode::FAILURE;
    }

    let app = adw::Application::builder()
        .application_id("com.ravenfilemanager.Raven.Portal")
        .flags(gio::ApplicationFlags::NON_UNIQUE)
        .build();

    let request_rx = RefCell::new(Some(request_rx));
    app.connect_activate(move |app| {
        let Some(mut request_rx) = request_rx.borrow_mut().take() else {
            return;
        };

        let appearance = AppConfig::load().appearance;
        // The same look as the file manager, following the desktop's.
        raven_ui::themes::init(appearance.theme);

        let hold = app.hold();
        let open_dialogs = Rc::new(Cell::new(0u32));
        let generation = Rc::new(Cell::new(0u64));
        schedule_idle_exit(app, &open_dialogs, &generation);

        let app = app.clone();
        glib::spawn_future_local(async move {
            let _hold = hold;
            while let Some(request) = request_rx.recv().await {
                let ChooserRequest {
                    mode,
                    app_id,
                    title,
                    options,
                    reply,
                    closed,
                    ..
                } = request;

                open_dialogs.set(open_dialogs.get() + 1);
                let on_done = {
                    let app = app.clone();
                    let open_dialogs = open_dialogs.clone();
                    let generation = generation.clone();
                    move |selection| {
                        let _ = reply.send(selection);
                        open_dialogs.set(open_dialogs.get().saturating_sub(1));
                        schedule_idle_exit(&app, &open_dialogs, &generation);
                    }
                };
                let dialog = FileChooserDialog::present(
                    &app,
                    DialogRequest {
                        mode,
                        app_id,
                        title,
                        options,
                    },
                    on_done,
                );
                // `closed` resolves Ok on Request.Close and Err once the call
                // has been answered and its handle removed.
                glib::spawn_future_local(async move {
                    if closed.await.is_ok() {
                        dialog.cancel();
                    }
                });
            }
        });
    });

    app.run_with_args(&["ravenfilemanager"])
}

/// Quit after [`IDLE_EXIT`] unless a dialog opens in the meantime.
fn schedule_idle_exit(app: &adw::Application, open_dialogs: &Rc<Cell<u32>>, generation: &Rc<Cell<u64>>) {
    if open_dialogs.get() > 0 {
        return;
    }
    let mine = generation.get().wrapping_add(1);
    generation.set(mine);
    let app = app.clone();
    let open_dialogs = open_dialogs.clone();
    let generation = generation.clone();
    glib::timeout_add_local_once(IDLE_EXIT, move || {
        if open_dialogs.get() == 0 && generation.get() == mine {
            tracing::info!("File chooser portal idle; exiting");
            app.quit();
        }
    });
}
