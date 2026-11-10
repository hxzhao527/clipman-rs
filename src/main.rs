use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result};
use clipman_rs::clipboard::{CapturedEvent, ClipboardService};
use clipman_rs::i18n::Translator;
use clipman_rs::storage::{StorageService, default_database_path};
use clipman_rs::tray::{ClipmanTray, TrayCommand, TraySnapshot, spawn_tray};
use clipman_rs::ui::{AppUi, AppUiInit};
use gtk::prelude::*;
use gtk::{Application, gio, glib};
use tokio::runtime::{Builder, Runtime};
use tracing::{error, info};
use tracing_subscriber::EnvFilter;

const APPLICATION_ID: &str = "io.github.clipman_rs.Clipman";

fn main() -> glib::ExitCode {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    let app = Application::builder()
        .application_id(APPLICATION_ID)
        .flags(gio::ApplicationFlags::empty())
        .build();

    let state_slot = Rc::new(RefCell::new(None::<ApplicationState>));
    let first_activation = Rc::new(Cell::new(true));
    {
        let state_slot = Rc::clone(&state_slot);
        app.connect_startup(move |app| match start_application(app) {
            Ok(state) => {
                *state_slot.borrow_mut() = Some(state);
            }
            Err(error) => {
                error!(%error, "failed to start clipman-rs");
                app.quit();
            }
        });
    }

    let activation_flag = Rc::clone(&first_activation);
    let activation_state = Rc::clone(&state_slot);
    app.connect_activate(move |_| {
        if activation_flag.replace(false) {
            return;
        }
        if let Some(state) = activation_state.borrow().as_ref() {
            state.ui.show();
        }
    });

    app.run()
}

struct ApplicationState {
    ui: AppUi,
    _runtime: Runtime,
    _tray_handle: ksni::Handle<ClipmanTray>,
    _clipboard: ClipboardService,
    _storage: StorageService,
    _hold: gio::ApplicationHoldGuard,
}

fn start_application(app: &Application) -> Result<ApplicationState> {
    let runtime = Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .context("failed to create Tokio runtime")?;
    let database_path = default_database_path()?;
    let (storage, initial_snapshot) = runtime.block_on(StorageService::open(&database_path))?;
    info!(path = %database_path.display(), "opened clipboard database");

    let (clipboard, mut clipboard_events) = ClipboardService::start()?;
    clipboard.set_primary_enabled(initial_snapshot.settings.primary_enabled);
    let translator = Translator::system();
    info!(language = ?translator.language(), "selected UI language");
    let tray_snapshot = Arc::new(Mutex::new(TraySnapshot::default()));
    let (command_tx, mut command_rx) = tokio::sync::mpsc::unbounded_channel::<TrayCommand>();
    let tray_handle = spawn_tray(
        &runtime,
        Arc::clone(&tray_snapshot),
        command_tx,
        translator.clone(),
    )?;
    let (storage_event_tx, mut storage_event_rx) = tokio::sync::mpsc::unbounded_channel();

    let ui = AppUi::new(AppUiInit {
        app: app.clone(),
        tray_snapshot: Arc::clone(&tray_snapshot),
        tray_handle: tray_handle.clone(),
        tokio_handle: runtime.handle().clone(),
        clipboard: clipboard.clone(),
        storage: storage.clone(),
        storage_events: storage_event_tx,
        initial_snapshot,
        translator,
    });

    let clipboard_ui = ui.clone();
    glib::spawn_future_local(async move {
        while let Some(CapturedEvent { source, payload }) = clipboard_events.recv().await {
            clipboard_ui.add_capture(source, payload);
        }
    });

    let command_ui = ui.clone();
    glib::spawn_future_local(async move {
        while let Some(command) = command_rx.recv().await {
            command_ui.handle_command(command);
        }
    });

    let storage_ui = ui.clone();
    glib::spawn_future_local(async move {
        while let Some(event) = storage_event_rx.recv().await {
            storage_ui.handle_storage_event(event);
        }
    });

    // Keep GTK alive while only the tray is active.
    let hold = app.hold();
    info!("clipman-rs started");
    Ok(ApplicationState {
        ui,
        _runtime: runtime,
        _tray_handle: tray_handle,
        _clipboard: clipboard,
        _storage: storage,
        _hold: hold,
    })
}
