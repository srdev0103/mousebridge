/**
 * Mirrors the `Serialize` types in `mb-core::status`.
 *
 * Kept hand-written rather than generated: the surface is small, and a
 * hand-written file is reviewable in a diff when the Rust side changes. The
 * round-trip is covered by a Rust test that asserts the JSON field names, so a
 * rename on either side fails a test rather than silently rendering `undefined`.
 */

export interface DeviceStatus {
  id_short: string;
  name: string;
  os: string;
  os_version: string;
  arch: string;
}

export interface PermissionEntry {
  id: string;
  title: string;
  rationale: string;
  granted: boolean;
  /** False once denied: macOS will not re-prompt, so offer Settings instead. */
  can_prompt: boolean;
}

export interface DisplayStatus {
  id: number;
  name: string | null;
  x: number;
  y: number;
  width: number;
  height: number;
  scale: number;
  is_primary: boolean;
}

export type StartupNotice =
  | { kind: "first-run" }
  | { kind: "migrated"; from_version: number }
  | { kind: "recovered"; backup_path: string; reason: string };

export interface CoreStatus {
  device: DeviceStatus;
  permissions: PermissionEntry[];
  displays: DisplayStatus[];
  display_error: string | null;
  /** Computed in Rust. The UI must never derive readiness itself. */
  sharing_ready: boolean;
  config_path: string;
  notice: StartupNotice | null;
}
