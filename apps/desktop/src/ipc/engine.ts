import { invoke } from "@tauri-apps/api/core";

export interface DiscoveredView {
  id_short: string;
  name: string;
  address: string;
  paired: boolean;
  connected: boolean;
  compatible: boolean;
}

export interface PairingView {
  peer_name: string;
  /** Six digits, grouped as `NNN NNN`. */
  code: string;
  confirmed_here: boolean;
  confirmed_there: boolean;
}

export interface EngineSnapshot {
  capturing: boolean;
  capture_error: string | null;
  port: number;
  discovered: DiscoveredView[];
  pairing: PairingView | null;
  /** `local`, or the short id of the machine receiving input. */
  input_destination: string;
  log: string[];
}

export function engineSnapshot(): Promise<EngineSnapshot> {
  return invoke<EngineSnapshot>("engine_snapshot");
}

export function beginPairing(id: string): Promise<void> {
  return invoke("begin_pairing", { id });
}

export function confirmPairing(): Promise<void> {
  return invoke("confirm_pairing");
}

export function rejectPairing(): Promise<void> {
  return invoke("reject_pairing");
}

export function forgetDevice(id: string): Promise<void> {
  return invoke("forget_device", { id });
}
