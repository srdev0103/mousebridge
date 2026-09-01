# Platform validation checklist

Some behaviour cannot be meaningfully automated: input capture, injection,
privacy permissions, sleep/wake, lock/unlock and display hotplug all need a human
at a real machine. This file records what must be checked by hand, and what has
actually been checked.

**Rule: nothing in this file may be described as working until a human has run it
on the target OS.** A successful compile is not validation. Cross-compiling with
`cargo check --target` proves signatures and types are correct; it proves nothing
about runtime behaviour.

## Status legend

| Mark | Meaning |
|---|---|
| ✅ | Run on real hardware, result recorded |
| 🔧 | Compile-checked only — **not** validated |
| ⬜ | Not yet attempted |

## Milestone 1 — platform abstraction

### macOS

| Check | Status | Notes |
|---|---|---|
| Computer name matches `scutil --get ComputerName` | ✅ | macOS 15.7.9, Intel. Returned `Kalkaudy's iMac`, matched exactly including the typographic apostrophe |
| OS version matches `sw_vers -productVersion` | ✅ | Returned `15.7.9` |
| Display bounds and scale match `system_profiler SPDisplaysDataType` | ✅ | 2560x1440 @ 2.00x against a Retina 5K (5120x2880) panel |
| Accessibility status reflects real TCC state | ✅ | Reported `NotDetermined` for an unsigned CLI binary, correctly not optimistic |
| Input Monitoring status reflects real TCC state | ✅ | Reported `Denied`, correctly distinct from the Accessibility result |
| Status probes do not prompt or mutate state | ✅ | Repeated probes returned identical values |
| Multi-monitor enumeration | ⬜ | Single-display host; needs a second display |
| Display hotplug / resolution change | ⬜ | Milestone 9 |
| Apple Silicon (arm64) | 🔧 | Cross-compiles; host is Intel. **Needs real Apple Silicon hardware** |

### Windows

| Check | Status | Notes |
|---|---|---|
| Builds for `x86_64-pc-windows-msvc` | 🔧 | `cargo check` clean |
| Builds for `aarch64-pc-windows-msvc` | 🔧 | `cargo check` clean |
| `GetComputerNameExW` returns the device name | ⬜ | |
| `RtlGetVersion` reports the real, unshimmed build | ⬜ | |
| `EnumDisplayMonitors` finds every monitor | ⬜ | |
| `GetDpiForMonitor` reports per-monitor scaling | ⬜ | |
| Primary monitor flag is correct | ⬜ | |
| **Mixed-DPI adjacency** (150% laptop + 100% external) | ⬜ | Highest-risk case; see ADR 0001 |
| Monitor rects are physical pixels under PMv2 | ⬜ | Assumption behind ADR 0001; must be confirmed |

## Measured baselines

Recorded so the milestone-13 budget has real numbers to argue with, not guesses.

| Measurement | Value | Notes |
|---|---|---|
| Release binary | 7.3 MB | thin LTO, `codegen-units = 1`, stripped |
| RSS, window open | 73 MB | macOS 15.7.9, Intel |
| RSS, window "closed" | **75 MB** | **hiding reclaims nothing — see below** |

**Finding: hide-on-close does not reduce memory.** The shell currently hides the
window on close so the app keeps running in the menu bar. On macOS the WKWebView
lives in separate system-managed processes that are not children of ours, and
hiding a window destroys none of it. The tray-only idle state therefore costs the
same ~75 MB as the visible one, against a target of roughly 30 MB.

The fix is to **destroy** the window on close and rebuild it on demand rather
than hiding it. That is a milestone-13 change (it needs the real tray UX around
it), and it is recorded here so the budget is not quietly assumed to be met.

Idle CPU was not measured meaningfully: `ps %cpu` on macOS is an average over
process lifetime, so a short-lived sample is dominated by startup. Steady-state
CPU needs a sampling harness, which arrives with the latency harness in
milestone 6.

## Milestone 3-4 — input capture and injection

Nothing here is implemented yet. Listed now because the input model surfaced each
one as a platform obligation, and the list should exist before the code does.

### macOS

Run `cargo run -p mb-input-native --example input-check -- --seconds 10`.

Note that a command-line binary inherits the TCC grants of whatever launched it,
so the harness needs **Terminal** to hold Accessibility and Input Monitoring. It
therefore does not validate the permission path a signed `.app` takes, which has
its own identity; that needs the bundled application.

| Check | Status | Notes |
|---|---|---|
| Tap creation reports the missing permission specifically | ✅ | Returned "create event tap requires the Accessibility permission", not a generic failure |
| **Injection fails silently without Accessibility** | ✅ | `CGEventPost` returns void, `CGEventSourceCreate` still succeeds, cursor does not move, **no error reported**. Callers must gate on the permission check; tests must assert the observable effect |
| Event tap re-arms after `kCGEventTapDisabledByTimeout` | ⬜ | Needs a grant; silent failure if missed |
| Capture observes moves, buttons, wheel, keys, modifiers | ⬜ | Needs a grant |
| Injected events are ignored by our own tap | ⬜ | Needs a grant. Without the marker check this is an infinite echo |
| Suppression stops the local cursor while control is remote | ⬜ | Needs a grant |
| Modifier `flagsChanged` maps to key down/up correctly | ⬜ | macOS has no keyUp for modifiers; diffing logic is unit tested |
| Secure Input detection identifies the blocking process | ⬜ | `kCGSSessionSecureInputPID`; API needs confirming on 15 |
| Pixel-precise trackpad scroll converts to lines sensibly | ⬜ | Conversion is unit tested; feel is not |
| Drag across the boundary continues (uses `*MouseDragged`) | ⬜ | Posting `MouseMoved` mid-drag would break it |

### Windows

All Windows code is **compile-checked only**. That proves signatures and types.
It proves nothing below.

Since milestone 5 the cross-check no longer covers the whole workspace. `rustls`
depends on `ring`, whose build script compiles C and assembly for the target, and
this development Mac has no MSVC toolchain. The check therefore runs over the
crates that are pure Rust:

```sh
cargo check --target x86_64-pc-windows-msvc \
  -p mb-types -p mb-config -p mb-platform -p mb-input -p mb-input-native -p mb-core
```

**`mb-net`, `mb-security` and `mb-protocol` are not cross-checked for Windows at
all.** They must be built and tested on a Windows machine or a Windows CI runner
before anything about them is described as working there. This is a real
reduction in local coverage introduced by this milestone, not an oversight.

The pure logic *is* tested, on any host: the scan-code table, wheel conversion,
absolute-coordinate normalisation and the shell-shortcut state machine account
for 20 of the passing tests and run in CI on macOS.

| Check | Status | Notes |
|---|---|---|
| Hooks install and deliver on a dedicated message-pump thread | ⬜ | |
| Hook procedure stays inside `LowLevelHooksTimeout` under load | ⬜ | 300 ms default; exceeding it silently skips the hook |
| Recovery after a timeout skip works via stop-then-start | ⬜ | Unlike a macOS tap, a Windows hook cannot be re-enabled in place |
| Suppression stops the local cursor while control is remote | ⬜ | |
| **Delta accumulation survives cursor anchoring** | ⬜ | Suppressed moves leave the cursor still, so deltas come from diffing the reported destination. Pinned at a screen edge the destination is clamped and movement is lost — hence parking at the primary display centre. This is the riskiest untested assumption in the backend |
| Absolute injection avoids double pointer acceleration | ⬜ | Core reason for `MouseMoveTo`; see ADR 0001 |
| **Lone `Meta` release does not open the Start menu** | ⬜ | Guard implemented and unit tested (`ShellShortcutGuard`); whether `VK_NONAME` actually suppresses the shell action needs real Windows |
| Same for a lone `Alt` and the menu bar | ⬜ | |
| `WM_SYSKEYDOWN` captures Alt combinations | ⬜ | Handled; without it every Alt chord is invisible |
| Injected events are ignored by our own hook | ⬜ | Via `dwExtraInfo` marker plus the `LLHF_INJECTED` flag |
| `WHEEL_DELTA` notches use the user's `SPI_GETWHEELSCROLLLINES` | ⬜ | Conversion tested; the value is not yet read from the OS |
| AltGr on a European layout produces accented characters | ⬜ | Mapped as extended Right Alt |
| Injection into an elevated window fails cleanly and visibly | ⬜ | Expected to fail (UIPI). `SendInput` returns a short count, which is reported specifically — it must not fail *silently* |
| Media keys report through the PS/2 set, not a vendor collection | ⬜ | Table entries are a best guess; some keyboards differ |

## Milestone 5 — transport and discovery

Verified on this machine, over real sockets:

| Check | Status | Notes |
|---|---|---|
| Two paired devices complete a QUIC handshake | ✅ | Real TLS 1.3 over loopback UDP |
| An unpaired device is refused | ✅ | |
| Trust must be mutual | ✅ | A one-sided relationship is rejected |
| A motion datagram survives the round trip | ✅ | Confirms datagram support negotiates |
| A silent peer is dropped at the handshake timeout | ✅ | Completes TLS, never sends Hello |
| A UDP announcement survives a socket round trip | ✅ | Loopback |
| Foreign traffic on the discovery port is skipped | ✅ | HTTP and oversized junk do not stall discovery |
| Two sockets can share the discovery port | ✅ | `SO_REUSEADDR` / `SO_REUSEPORT` |
| **A live mDNS advertisement is discovered** | ✅ | Real multicast on this machine's interfaces; resolved in 0.84 s |

Still unverified, and needing a second machine:

| Check | Status | Notes |
|---|---|---|
| Discovery across two real machines | ⬜ | Loopback proves the code, not the network |
| mDNS on consumer Wi-Fi with client isolation | ⬜ | The case broadcast fallback exists for |
| Broadcast on a network that filters multicast | ⬜ | |
| Connection migration across Wi-Fi ↔ Ethernet | ⬜ | A stated reason for choosing QUIC |
| Heartbeat detects a genuinely wedged peer | ⬜ | State machine is unit tested; the wiring is not |
| Latency under load, p50/p99 | ⬜ | Needs two physical machines; milestone 6 |

## Known environment limitations

The development host is an Intel iMac running macOS 15.7.9 on non-Apple hardware
(i5-9600K). Three consequences:

1. **Apple Silicon cannot be validated here.** All arm64 support is
   cross-compiled and unverified until run on real hardware.
2. **TCC and SIP behaviour may deviate from stock hardware.** Permission results
   above are indicative; they should be re-confirmed on an Apple-manufactured Mac
   before release.
3. **Windows validation needs a second machine.** VMware Fusion is available and
   is adequate for functional work, but not for latency measurement, not for the
   UAC/secure-desktop path, and not for ARM64 on an Intel host.
