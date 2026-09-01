# Architecture

The full proposal — protocol, security model, topology design, risk register —
lives in the milestone-0 design record. This document tracks what is **built**,
and is updated at the end of each milestone.

## Principles

1. **The Rust core owns the logic.** The frontend renders state and issues
   commands. It never computes a boundary, decides whether sharing is ready, or
   validates a device name — Rust does, so the two can never disagree.
2. **The input path never blocks.** No allocation, no locks, no async runtime, no
   disk I/O and no logging between reading an input event and enqueuing it.
3. **Platform code lives behind traits.** Generic crates contain no
   `#[cfg(target_os)]`. Anything that cannot be tested on the developer's machine
   is marked as requiring platform validation and listed in
   `platform-validation.md`.
4. **Failures are visible.** Silent degradation is the failure mode this class of
   software is known for: a missing permission, a disabled event tap, a blocked
   multicast. Each is detected and explained.

## Built so far (milestones 1-2)

```
   React + TypeScript + Tailwind          apps/desktop/src
            │
            │  Tauri IPC — control plane only
            ▼
   ┌──────────────────────────────┐
   │ mb-core                      │       orchestration, logging, status
   │  ├── Core                    │
   │  ├── logging                 │
   │  └── status (UI projection)  │
   └───────┬──────────────┬───────┘
           │              │
           │              │        ┌────────────────────────────────┐
           │              │        │ mb-input                       │
           │              │        │  InputEvent, KeyCode (HID)     │
           │              │        │  InputStateTracker             │
           │              │        │  InputCapture / InputInject    │
           │              │        │  virtual_backend (simulation)  │
           │              │        └────────────────┬───────────────┘
           │              │                         │
           ▼              ▼                         │
   ┌──────────────┐  ┌──────────────────────────┐
   │ mb-config    │  │ mb-platform              │
   │ schema       │  │  trait Platform          │
   │ store        │  │   ├── macos (real)       │
   │ migrate      │  │   ├── windows (real)     │
   └──────┬───────┘  │   └── mock (test double) │
          │          └───────────┬──────────────┘
          └──────────┬───────────┘
                     ▼
             ┌──────────────┐
             │ mb-types     │◄────────────┘
             └──────────────┘  primitives; no I/O, no async, no platform
```

Dependencies point one way. `mb-types` depends on nothing; `mb-config` and
`mb-platform` do not know about each other; `mb-core` composes them.

## Crate responsibilities

| Crate | Owns | Async | Platform code |
|---|---|---|---|
| `mb-types` | ids, names, geometry, `Redacted` | no | none (`forbid(unsafe_code)`) |
| `mb-config` | schema, atomic store, migration | no | none |
| `mb-platform` | displays, permissions, host identity | no | isolated in `macos/`, `windows/` |
| `mb-input` | events, key codes, held-state tracking, backend traits | no | none (`forbid(unsafe_code)`) |
| `mb-input-native` | OS capture and injection backends | no | isolated in `macos/`, `windows/` |
| `mb-core` | orchestration, logging, status snapshot | no (yet) | none |
| `mousebridge-desktop` | window, tray, IPC commands | — | thin shims only |

## Decisions worth knowing

- **[ADR 0001](adr/0001-display-coordinate-space.md)** — displays are reported in
  each device's *native* cursor space with the scale factor alongside, not
  pre-normalised to logical points. Normalising at the platform boundary breaks
  mixed-DPI adjacency and discards the space cursor injection needs.

- **Secrets are protected by type, not by discipline.** `Redacted<T>` renders as
  `<redacted N bytes>` from both `Debug` and `Display`, so logging a struct that
  contains one cannot leak it. Audit with `grep -r '\.expose()'`.

- **`unsafe` is confined, and the justification is enforced.** It appears in five
  files, all of them OS bindings; everywhere else `unsafe_code` is a workspace
  lint, and `mb-types`, `mb-config`, `mb-core`, `mb-input` and `xtask` forbid it
  outright. Every block carries a `SAFETY` comment discharging its contract, and
  `xtask::unsafe_audit` fails `cargo test --workspace` if one does not — so the
  rule cannot decay between reviews. Currently 34 blocks, 34 justifications.

- **Config recovery is asymmetric on purpose.** A *corrupt* file is preserved and
  replaced with defaults, because refusing to launch is worse for a background
  utility. A file from a *newer* build is refused, because parsing it with an
  older schema would discard settings the user still depends on.

- **`mb-input` has no logger and no async runtime, by dependency.** Rendering
  which keys a user pressed is keylogging whatever the log level, so the ability
  is removed rather than discouraged: the crate does not depend on `tracing`, and
  `InputEvent`'s own `Display` prints `KeyDown`, never which key.

- **Keys travel as HID usage IDs, not characters or platform key codes.**
  Characters depend on the sender's layout, so a QWERTZ user would type the wrong
  letters on a US-layout machine. HID usage names the physical key; the receiver
  applies its own layout, exactly as if the keyboard were plugged into it.

- **Malformed events cannot reach an OS input API.** `Validated<T>` wraps the
  platform injector and rejects non-finite coordinates. It is a wrapper rather
  than a default trait method because a default method can be overridden, and a
  NaN passed to `SendInput` is undefined behaviour in the focused application.

- **Platform backends live in their own crate.** `mb-input-native` is separate
  from `mb-input` so the event model stays `forbid(unsafe_code)`, and so
  `core-graphics` and the `windows` crate stay out of the dependency graph for
  the protocol, topology and core crates that only need the model.

- **Every platform's pure logic compiles on every host.** Only the modules that
  call the OS are `#[cfg]`-gated; the key tables and field conversions for both
  platforms build and test anywhere. This is what makes the cross-table test
  possible — the two key tables are hand-written from different sources, and each
  round-trips perfectly on its own while potentially disagreeing with the other.
  Gating by host would hide exactly that.

## Testing

`cargo test --workspace` — 201 tests, no hardware required beyond the host.

Nine of those are property tests over the input state machine, asserting that
after *any* sequence of events — including sequences no real keyboard produces —
the release sequence returns the machine to holding nothing. They earned their
place immediately by finding two real stuck-input bugs:

- an undefined mouse-button bit from the network was storable but not
  enumerable, so `is_empty()` reported "still holding something" that no release
  sequence could ever clear;
- a peer reporting the same key twice had it tracked twice, so the single
  release that followed left one copy held forever.

Both now have named regression tests beside them.

`mb-platform::mock::MockPlatform` is a configurable fake, not a stub: it models
mixed-DPI dual displays, denied permissions, display hotplug and query failure,
so states that are impractical to reproduce on demand are covered in CI.

Cross-compilation is part of the check, not an afterthought:

```sh
cargo check --target x86_64-pc-windows-msvc
cargo check --target aarch64-pc-windows-msvc
cargo check --target aarch64-apple-darwin
```

This type-checks the Windows backend from a Mac. It proves signatures and types;
it proves nothing about runtime behaviour, which is why
`platform-validation.md` exists and why nothing there is marked ✅ without a
human having run it.
