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

| Check | Status | Notes |
|---|---|---|
| Event tap re-arms after `kCGEventTapDisabledByTimeout` | ⬜ | Silent failure if missed |
| Secure Input detection identifies the blocking process | ⬜ | `kCGSSessionSecureInputPID`; API needs confirming on 15 |
| Modifier `flagsChanged` maps to key down/up correctly | ⬜ | macOS has no keyUp for modifiers |
| Suppression stops the local cursor while control is remote | ⬜ | |
| Pixel-precise trackpad scroll converts to lines sensibly | ⬜ | |

### Windows

| Check | Status | Notes |
|---|---|---|
| `WH_*_LL` hooks re-install after `LowLevelHooksTimeout` | ⬜ | |
| Absolute injection avoids double pointer acceleration | ⬜ | Core reason for `MouseMoveTo`; see ADR 0001 |
| **Lone `Meta` release does not open the Start menu** | ⬜ | A release sequence ending in a solitary `Meta` up triggers Start. The injector must neutralise it, conventionally by bracketing with an inert key. Same problem for a lone `Alt` and the menu bar. |
| `WHEEL_DELTA` notches convert to lines using the user's setting | ⬜ | `SPI_GETWHEELSCROLLLINES` |
| Injection into an elevated window fails cleanly and visibly | ⬜ | Expected to fail; must not fail *silently* |

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
