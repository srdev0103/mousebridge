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

/** Reveals the configuration file in the system file manager. */
export function revealConfig(): Promise<void> {
  return invoke("reveal_config");
}
