//! Tauri IPC commands.
//!
//! Each command validates its arguments and delegates. Arguments arrive from the
//! webview and are treated as untrusted: an unrecognised permission key is an
//! error, never a default.

use mb_core::{Core, CoreStatus};
use mb_types::DeviceName;
use tauri::State;
use tracing::warn;

/// Result type for commands. Errors cross IPC as strings for display.
type CommandResult<T> = Result<T, String>;

/// Returns the current application status.
#[tauri::command]
pub fn get_status(core: State<'_, Core>) -> CommandResult<CoreStatus> {
    core.status().map_err(|e| e.to_string())
}

/// Asks the OS to prompt for a permission.
#[tauri::command]
pub fn request_permission(core: State<'_, Core>, id: &str) -> CommandResult<()> {
    let permission = mb_core::permission_from_key(id).ok_or_else(|| {
        warn!(key = id, "frontend requested an unknown permission");
        format!("unknown permission: {id}")
    })?;
    core.platform()
        .request_permission(permission)
        .map_err(|e| e.to_string())
}

/// Opens the OS settings pane for a permission.
#[tauri::command]
pub fn open_permission_settings(core: State<'_, Core>, id: &str) -> CommandResult<()> {
    let permission =
        mb_core::permission_from_key(id).ok_or_else(|| format!("unknown permission: {id}"))?;
    core.platform()
        .open_permission_settings(permission)
        .map_err(|e| e.to_string())
}

/// Renames this device and returns the refreshed status.
#[tauri::command]
pub fn set_device_name(core: State<'_, Core>, name: &str) -> CommandResult<CoreStatus> {
    // Validated here rather than in the UI: the config file is also hand-editable
    // and the rule must hold for both entry points.
    let validated = DeviceName::new(name).map_err(|e| e.to_string())?;
    core.update_config(|config| config.device.name = validated)
        .map_err(|e| e.to_string())?;
    core.status().map_err(|e| e.to_string())
}

/// A block in the layout editor, in shared coordinates.
#[derive(Debug, Clone, Copy, serde::Serialize, serde::Deserialize)]
pub struct EditorBlock {
    /// Left edge.
    pub x: f64,
    /// Top edge.
    pub y: f64,
    /// Width.
    pub width: f64,
    /// Height.
    pub height: f64,
}

impl EditorBlock {
    fn to_rect(self) -> Option<mb_types::LogicalRect> {
        mb_types::LogicalRect::from_parts(self.x, self.y, self.width, self.height)
    }

    const fn from_rect(rect: mb_types::LogicalRect) -> Self {
        Self {
            x: rect.origin.x,
            y: rect.origin.y,
            width: rect.size.width,
            height: rect.size.height,
        }
    }
}

/// Where a dragged block should come to rest.
#[derive(Debug, Clone, Copy, serde::Serialize)]
pub struct SnapOutcome {
    /// The resting position.
    pub block: EditorBlock,
    /// Whether it attached to a neighbour.
    pub snapped: bool,
    /// Whether it now overlaps something, which cannot be saved.
    pub overlapping: bool,
}

/// Places a dragged block, snapping it to its neighbours.
///
/// The geometry lives in `mb-topology` rather than the interface, because a
/// one-point gap between two screens is invisible on a scaled canvas and
/// completely breaks crossing — the pointer reaches the edge and stops, with
/// nothing on screen to explain why.
#[tauri::command]
pub fn snap_block(dragged: EditorBlock, others: Vec<EditorBlock>) -> CommandResult<SnapOutcome> {
    let dragged_rect = dragged
        .to_rect()
        .ok_or_else(|| "a screen cannot have zero size".to_owned())?;
    let other_rects: Vec<mb_types::LogicalRect> =
        others.iter().filter_map(|b| b.to_rect()).collect();

    let result = mb_topology::snap(dragged_rect, &other_rects);
    let placed = mb_types::LogicalRect::new(result.origin, dragged_rect.size);

    Ok(SnapOutcome {
        block: EditorBlock::from_rect(placed),
        snapped: result.snapped,
        overlapping: mb_topology::overlaps_any(placed, &other_rects),
    })
}

/// Turns input sharing on or off, and persists the choice.
#[tauri::command]
pub fn set_sharing_enabled(core: State<'_, Core>, enabled: bool) -> CommandResult<CoreStatus> {
    core.update_config(|config| config.switching.enabled = enabled)
        .map_err(|e| e.to_string())?;
    core.status().map_err(|e| e.to_string())
}

/// Updates the edge-crossing behaviour.
///
/// Validation happens in the core, so a value the engine would refuse cannot be
/// saved from the settings pane either.
#[tauri::command]
pub fn set_switching(
    core: State<'_, Core>,
    overshoot: f64,
    cooldown_ms: u64,
    corner_deadzone: f64,
) -> CommandResult<CoreStatus> {
    core.update_config(|config| {
        config.switching.edge_overshoot_points = overshoot;
        config.switching.cooldown_ms = cooldown_ms;
        config.switching.corner_deadzone_points = corner_deadzone;
    })
    .map_err(|e| e.to_string())?;
    core.status().map_err(|e| e.to_string())
}

/// Closes the dashboard window.
///
/// The application keeps running in the menu bar. See `window` for why this
/// destroys the window rather than hiding it.
#[tauri::command]
pub fn close_dashboard(app: tauri::AppHandle) {
    crate::window::hide(&app);
}

/// Reveals the configuration file in the system file manager.
///
/// Implemented with the platform's own file manager rather than a Tauri plugin:
/// it is two lines of `Command`, and every plugin added to the capability set
/// widens what a compromised webview could reach.
#[tauri::command]
pub fn reveal_config(core: State<'_, Core>) -> CommandResult<()> {
    let status = core.status().map_err(|e| e.to_string())?;
    let path = status.config_path;

    #[cfg(target_os = "macos")]
    let result = std::process::Command::new("/usr/bin/open")
        .args(["-R", &path])
        .spawn();

    #[cfg(target_os = "windows")]
    let result = std::process::Command::new("explorer.exe")
        .arg(format!("/select,{path}"))
        .spawn();

    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    let result: std::io::Result<std::process::Child> = Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "no file manager integration on this platform",
    ));

    result.map(|_| ()).map_err(|e| e.to_string())
}
