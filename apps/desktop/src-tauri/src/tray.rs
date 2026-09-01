//! Menu-bar and system-tray integration.
//!
//! The tray is the application's primary surface: the dashboard is something the
//! user opens occasionally, and everything they need day to day is here.

use crate::window;
use mb_core::Core;
use tauri::menu::{CheckMenuItem, Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::{AppHandle, Manager as _, Runtime};
use tracing::{error, info, warn};

/// Menu item identifiers.
const ID_OPEN: &str = "open-dashboard";
const ID_SHARING: &str = "toggle-sharing";
const ID_QUIT: &str = "quit";

/// Installs the tray icon and its menu.
///
/// # Errors
///
/// Returns an error if the tray or its menu could not be created, which on both
/// platforms means the application cannot present a background interface at all.
pub fn install<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<()> {
    let sharing_enabled = app
        .try_state::<Core>()
        .and_then(|core| core.config().ok())
        .is_some_and(|config| config.switching.enabled);

    let open = MenuItem::with_id(app, ID_OPEN, "Open Dashboard", true, None::<&str>)?;
    let sharing = CheckMenuItem::with_id(
        app,
        ID_SHARING,
        "Share Input",
        true,
        sharing_enabled,
        None::<&str>,
    )?;
    let quit = MenuItem::with_id(app, ID_QUIT, "Quit MouseBridge", true, None::<&str>)?;
    let menu = Menu::with_items(
        app,
        &[
            &open,
            &PredefinedMenuItem::separator(app)?,
            &sharing,
            &PredefinedMenuItem::separator(app)?,
            &quit,
        ],
    )?;

    let mut builder = TrayIconBuilder::with_id("main")
        .menu(&menu)
        // On macOS a menu-bar item with no left-click action reads as broken.
        .show_menu_on_left_click(true)
        .on_menu_event(handle_menu_event);

    if let Some(icon) = app.default_window_icon().cloned() {
        builder = builder.icon(icon);
    } else {
        // A tray item with no icon is invisible in the menu bar, so this must be
        // reported rather than silently accepted.
        error!("no default window icon available; the tray item will be blank");
    }

    builder.build(app)?;
    info!(sharing_enabled, "tray installed");
    Ok(())
}

/// Handles a tray menu selection.
fn handle_menu_event<R: Runtime>(app: &AppHandle<R>, event: MenuEvent) {
    match event.id().as_ref() {
        ID_OPEN => window::show(app),
        ID_SHARING => toggle_sharing(app),
        ID_QUIT => {
            info!("quit requested from the tray");
            // Sharing must stop before the process does. Exiting while a peer
            // believes it is holding a key would leave that key down on a machine
            // with nobody left to release it.
            release_and_exit(app);
        }
        other => error!(id = other, "unhandled tray menu item"),
    }
}

/// Flips the sharing switch and persists it.
fn toggle_sharing<R: Runtime>(app: &AppHandle<R>) {
    let Some(core) = app.try_state::<Core>() else {
        error!("no core available to toggle sharing");
        return;
    };

    let Ok(config) = core.config() else {
        error!("could not read the configuration");
        return;
    };
    let enabling = !config.switching.enabled;

    match core.update_config(|config| config.switching.enabled = enabling) {
        Ok(_) => info!(enabled = enabling, "sharing toggled from the tray"),
        Err(e) => error!(error = %e, "could not save the sharing setting"),
    }
}

/// Stops sharing cleanly, then exits.
fn release_and_exit<R: Runtime>(app: &AppHandle<R>) {
    if let Some(core) = app.try_state::<Core>()
        && let Err(e) = core.update_config(|config| config.switching.enabled = false)
    {
        // Not fatal — we are leaving anyway — but worth recording, because the
        // next launch will come up sharing when the user expected it off.
        warn!(error = %e, "could not record sharing as stopped before quitting");
    }
    app.exit(0);
}
