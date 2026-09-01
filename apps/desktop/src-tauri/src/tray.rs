//! Menu-bar and system-tray integration.
//!
//! Intentionally minimal at milestone 1. The full tray described in the
//! architecture — enable/disable sharing, connected computers, diagnostics —
//! arrives in milestone 13. Adding those items now would mean shipping controls
//! that do nothing, which is worse than shipping fewer controls.

use tauri::menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::{AppHandle, Manager as _, Runtime};
use tracing::{error, info};

/// Menu item identifiers.
const ID_OPEN: &str = "open-dashboard";
const ID_QUIT: &str = "quit";

/// Installs the tray icon and its menu.
///
/// # Errors
///
/// Returns an error if the tray icon or menu could not be created, which on both
/// platforms means the process cannot present a background UI at all.
pub fn install<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<()> {
    let open = MenuItem::with_id(app, ID_OPEN, "Open Dashboard", true, None::<&str>)?;
    let separator = PredefinedMenuItem::separator(app)?;
    let quit = MenuItem::with_id(app, ID_QUIT, "Quit MouseBridge", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&open, &separator, &quit])?;

    let mut builder = TrayIconBuilder::with_id("main")
        .menu(&menu)
        // On macOS the menu must open on a left click, because a menu-bar item
        // with no left-click action reads as broken.
        .show_menu_on_left_click(true)
        .on_menu_event(handle_menu_event);

    if let Some(icon) = app.default_window_icon().cloned() {
        builder = builder.icon(icon);
    } else {
        // Not fatal, but a tray item with no icon is invisible in the menu bar,
        // so it must be reported rather than silently accepted.
        error!("no default window icon available; the tray item will be blank");
    }

    builder.build(app)?;
    info!("tray installed");
    Ok(())
}

/// Handles a tray menu selection.
fn handle_menu_event<R: Runtime>(app: &AppHandle<R>, event: MenuEvent) {
    match event.id().as_ref() {
        ID_OPEN => show_dashboard(app),
        ID_QUIT => {
            info!("quit requested from the tray");
            app.exit(0);
        }
        other => error!(id = other, "unhandled tray menu item"),
    }
}

/// Brings the dashboard window to the front, creating nothing if it is missing.
fn show_dashboard<R: Runtime>(app: &AppHandle<R>) {
    let Some(window) = app.get_webview_window("main") else {
        error!("the main window does not exist");
        return;
    };
    if let Err(e) = window.show() {
        error!(error = %e, "could not show the window");
        return;
    }
    if let Err(e) = window.set_focus() {
        error!(error = %e, "could not focus the window");
    }
}
