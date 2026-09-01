//! Dashboard window lifecycle.
//!
//! # Why the window is destroyed, not hidden
//!
//! MouseBridge spends almost all of its life with no window open. Hiding one and
//! keeping it around is the obvious approach and it does not work: measured on
//! macOS 15, hiding reclaimed nothing at all — the process held ~75 MB either
//! way, against a target nearer 30 MB.
//!
//! The reason is that the webview does not live in this process. `WKWebView` and
//! `WebView2` run in their own system-managed processes, so hiding a window
//! destroys none of them, and they are not children of ours — which is why a
//! naive `ps` on the parent makes the cost invisible rather than absent.
//!
//! Destroying the window releases them. The cost is that reopening rebuilds the
//! webview, which takes a moment. For a utility whose window is opened
//! occasionally and whose *job* is to sit in the background, that is the right
//! trade.

use tauri::{AppHandle, Manager as _, Runtime, WebviewUrl, WebviewWindowBuilder};
use tracing::{debug, error};

/// Label of the dashboard window.
pub const MAIN: &str = "main";

/// Brings the dashboard to the front, creating it if it is not open.
pub fn show<R: Runtime>(app: &AppHandle<R>) {
    if let Some(window) = app.get_webview_window(MAIN) {
        if let Err(e) = window.show().and_then(|()| window.set_focus()) {
            error!(error = %e, "could not focus the existing window");
        }
        return;
    }

    debug!("rebuilding the dashboard window");
    let built = WebviewWindowBuilder::new(app, MAIN, WebviewUrl::default())
        .title("MouseBridge")
        .inner_size(760.0, 900.0)
        .min_inner_size(520.0, 560.0)
        .resizable(true)
        .build();

    match built {
        Ok(window) => {
            if let Err(e) = window.set_focus() {
                error!(error = %e, "could not focus the new window");
            }
        }
        // Not fatal: the application's actual job needs no window at all, and
        // quitting because a window failed to open would be a worse outcome than
        // running headless with a tray icon.
        Err(e) => error!(error = %e, "could not open the dashboard window"),
    }
}

/// Closes the dashboard, releasing the webview processes behind it.
pub fn hide<R: Runtime>(app: &AppHandle<R>) {
    if let Some(window) = app.get_webview_window(MAIN)
        && let Err(e) = window.destroy()
    {
        error!(error = %e, "could not close the dashboard window");
    }
}
