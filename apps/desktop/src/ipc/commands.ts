import { invoke } from "@tauri-apps/api/core";
import type { CoreStatus } from "./types";

/** Reads the current snapshot of application state. */
export function getStatus(): Promise<CoreStatus> {
  return invoke<CoreStatus>("get_status");
}

/**
 * Asks the OS to prompt for a permission.
 *
 * Resolving does not mean the permission was granted — on macOS the user acts in
 * System Settings and the app usually needs relaunching. Callers must keep
 * polling {@link getStatus}.
 */
export function requestPermission(id: string): Promise<void> {
  return invoke("request_permission", { id });
}

/** Opens the OS settings pane for a permission. */
export function openPermissionSettings(id: string): Promise<void> {
  return invoke("open_permission_settings", { id });
}

/** Renames this device. Returns the updated snapshot. */
export function setDeviceName(name: string): Promise<CoreStatus> {
  return invoke<CoreStatus>("set_device_name", { name });
}

/** Turns input sharing on or off. */
export function setSharingEnabled(enabled: boolean): Promise<CoreStatus> {
  return invoke<CoreStatus>("set_sharing_enabled", { enabled });
}

/** Updates the edge-crossing behaviour. Rejected values are refused by the core. */
export function setSwitching(
  overshoot: number,
  cooldownMs: number,
  cornerDeadzone: number,
): Promise<CoreStatus> {
  return invoke<CoreStatus>("set_switching", {
    overshoot,
    cooldownMs,
    cornerDeadzone,
  });
}

/** Closes the dashboard. The app keeps running in the menu bar. */
export function closeDashboard(): Promise<void> {
  return invoke("close_dashboard");
}

/** Reveals the configuration file in the system file manager. */
export function revealConfig(): Promise<void> {
  return invoke("reveal_config");
}
