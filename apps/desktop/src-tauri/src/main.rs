//! MouseBridge desktop shell.
//!
//! Deliberately thin. Every command below is a wrapper over a method on
//! [`mb_core::Core`], so the decisions stay in Rust where they can be tested
//! without a webview, and the frontend cannot disagree with the engine about
//! whether, for example, sharing is ready.

// Without this, a release build on Windows opens a console window behind the app.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod commands;
mod tray;

use mb_config::ConfigStore;
use mb_core::{Core, logging};
use tracing::{error, info};

fn main() {
    // Configuration must load before logging, because it says where logs go and
    // at what level. Failures here are fatal and reported on stderr: there is no
    // log to write them to yet, and no window to show them in.
    let store = match ConfigStore::at_default_location() {
        Ok(store) => store,
        Err(e) => {
            eprintln!("MouseBridge: cannot determine the configuration location: {e}");
            std::process::exit(1);
        }
    };

    let core = match Core::start(store.clone()) {
        Ok(core) => core,
        Err(e) => {
            eprintln!("MouseBridge: cannot start: {e}");
            std::process::exit(1);
        }
    };

    // Held for the process lifetime; dropping it stops the file-log worker.
    let _log_guard = match core.config() {
        Ok(config) => match logging::init(&config.logging, store.directory()) {
            Ok(guard) => Some(guard),
            Err(e) => {
                eprintln!("MouseBridge: logging unavailable, continuing without it: {e}");
                None
            }
        },
        Err(e) => {
            eprintln!("MouseBridge: cannot read configuration: {e}");
            std::process::exit(1);
        }
    };

    info!(
        version = env!("CARGO_PKG_VERSION"),
        "starting desktop shell"
    );

    tauri::Builder::default()
        .manage(core)
        .invoke_handler(tauri::generate_handler![
            commands::get_status,
            commands::request_permission,
            commands::open_permission_settings,
            commands::set_device_name,
            commands::reveal_config,
        ])
        .setup(|app| {
            tray::install(app.handle())?;
            Ok(())
        })
        .on_window_event(|window, event| {
            // Closing the window hides it rather than quitting: MouseBridge keeps
            // running in the menu bar or system tray, which is the point of a
            // background utility. Quit is an explicit tray action.
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                api.prevent_close();
                if let Err(e) = window.hide() {
                    error!(error = %e, "could not hide the window");
                }
            }
        })
        .run(tauri::generate_context!())
        .unwrap_or_else(|e| {
            error!(error = %e, "the application loop failed");
            eprintln!("MouseBridge: fatal: {e}");
            std::process::exit(1);
        });
}
