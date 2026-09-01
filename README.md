# MouseBridge

Share one mouse and keyboard across several computers on a local network. Move
the pointer past the edge of one screen and it continues on the next machine.

Windows and macOS. An original implementation — no code, protocol or assets are
taken from any existing software KVM.

> **Status: milestone 3 of 14.** Foundation, input model, and the macOS input
> backend — a CoreGraphics event tap for capture and synthetic events for
> injection. There is still no networking, so it cannot yet share input between
> computers. See [Roadmap](#roadmap).

## Requirements

| | |
|---|---|
| macOS | 13 or later — Intel and Apple Silicon |
| Windows | 10 or later — x64 and ARM64 |
| Rust | 1.98 (pinned in `rust-toolchain.toml`) |
| Node | 20 or later |

## Getting started

```sh
# Frontend dependencies
cd apps/desktop && npm install && cd ../..

# Run the whole workspace's tests
cargo test --workspace

# Launch the app with hot reload
cd apps/desktop && npm run tauri dev
```

Type-check the platform backends you cannot run locally:

```sh
cargo check --target x86_64-pc-windows-msvc
cargo check --target aarch64-pc-windows-msvc
cargo check --target aarch64-apple-darwin
```

Inspect what the platform layer sees on this machine:

```sh
cargo run -p mb-platform --example probe
```

Exercise the macOS input backend, including an injection self-test:

```sh
cargo run -p mb-input-native --example input-check -- --seconds 10
```

A command-line binary inherits the privacy grants of whatever launched it, so
this needs **Terminal** to hold Accessibility and Input Monitoring. It reports
event counts and kinds only — never which keys were pressed.

## macOS development note

macOS ties Accessibility and Input Monitoring grants to an application's **code
signature**. An unsigned or ad-hoc-signed build gets a new identity on every
rebuild, so every rebuild revokes the permission and the app silently stops
capturing input.

Set a stable development identity before starting work on the input layer:

```sh
export APPLE_SIGNING_IDENTITY="Apple Development: you@example.com (TEAMID)"
```

`security find-identity -v -p codesigning` lists the identities available.

## Layout

```
apps/desktop/          Tauri shell — React + TypeScript + Tailwind
  src/                 dashboard, IPC bindings
  src-tauri/           window, tray, IPC commands (thin)
crates/
  mb-types/            shared primitives; no I/O, no async, no platform code
  mb-config/           schema, atomic persistence, migration
  mb-platform/         displays, permissions, host identity
  mb-core/             orchestration, logging, status snapshot
  mb-input/            input events, state tracking, capture/inject traits
  mb-input-native/     OS input backends (CoreGraphics event tap, injection)
docs/
  adr/                 architecture decision records
  platform-validation.md   what has actually been tested, and where
```

The Rust core holds the application logic. The frontend renders state and issues
commands; it makes no decisions of its own. Low-level input will never be
implemented in TypeScript.

## Configuration

`~/Library/Application Support/MouseBridge/config.toml` on macOS,
`%APPDATA%\MouseBridge\config.toml` on Windows. Hand-editable TOML.

Writes are atomic. A corrupt file is preserved alongside and replaced with
defaults rather than blocking startup; a file written by a *newer* build is
refused outright rather than silently downgraded, because parsing it with an
older schema would discard settings permanently.

## Roadmap

| | Milestone | Status |
|---|---|---|
| 1 | Foundation: workspace, config, logging, platform abstraction, shell | **done** |
| 2 | Input event abstraction + virtual backend | **done** |
| 3 | macOS input capture and injection | **code complete, needs a permission grant to validate** |
| 4 | Windows input capture and injection | next |
| 5 | QUIC transport, discovery, heartbeat, TLS | |
| 6 | Remote mouse and keyboard | |
| 7 | Screen-edge switching | |
| 8 | Multiple computers | |
| 9 | Multiple monitors and DPI | |
| 10 | Pairing, verification codes, trust store | |
| 11 | Clipboard synchronisation | |
| 12 | File transfer | |
| 13 | Production UX | |
| 14 | Packaging, signing, notarisation | |

Encryption is present from milestone 5, not added at 10: the transport is QUIC,
which has no plaintext mode. Milestone 10 adds the pairing UX and key pinning on
top of a transport that was never insecure.

## Known limitations

These are design boundaries, not defects, and are surfaced in the UI rather than
discovered by users:

- **Windows elevated windows.** A normal-privilege process cannot inject input
  into an elevated window (UIPI). Planned mitigation: a signed build with
  `uiAccess`.
- **Windows secure desktop.** The UAC prompt, lock screen and Ctrl+Alt+Del are
  unreachable from a user-session process. Supporting them requires a session-0
  service, deferred beyond v1.
- **macOS Secure Input.** When a password field has focus, macOS blocks all
  keyboard taps system-wide. MouseBridge detects this and explains it rather than
  appearing broken.

## Licence

MIT OR Apache-2.0.
