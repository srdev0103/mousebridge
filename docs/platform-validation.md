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

**Finding: hide-on-close does not reduce memory.** The shell originally hid the
window on close so the app keeps running in the menu bar. On macOS the WKWebView
lives in separate system-managed processes that are not children of ours, and
hiding a window destroys none of it. The tray-only idle state therefore cost the
same as the visible one.

**Correction (milestone 13): the original figure undercounted.** The 75 MB above
was the application process alone. The webview runs in separate processes that a
`ps` on the parent does not see. Measured again with those included:

| | App process | Webview processes | Total |
|---|---|---|---|
| Window open | 72 MB | 26 MB | **~98 MB** |

**The fix is implemented but unverified.** Milestone 13 changed close-to-destroy
rather than hide (see `apps/desktop/src-tauri/src/window.rs`). Whether it
actually reclaims the webview processes **has not been demonstrated**: closing
the window needs a click or a keystroke, and `osascript` on this machine is
refused assistive access, so the automated attempt silently did nothing and the
window stayed open. The unchanged reading that followed is evidence about the
test, not about the fix.

To verify: launch the app, open Activity Monitor, close the dashboard window by
hand, and check whether the `MouseBridge Web Content` processes exit.

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

## Milestone 6 — remote input

The full pipeline is verified **headlessly**, in one process: a virtual capture
backend feeds the router, which forwards over a real authenticated QUIC
connection to a virtual injector. Only the two ends that touch hardware are
simulated, and both are real state machines rather than stubs.

| Check | Status | Notes |
|---|---|---|
| A keystroke captured here lands on the other machine | ✅ | Through the real transport |
| Local input never leaves the machine | ✅ | Nothing forwarded until the destination changes |
| Pointer motion moves the remote cursor | ✅ | Accumulated position over datagrams |
| A chord arrives intact and in order | ✅ | Cmd+C down/up leaves the remote clean |
| **The remote releases when the sender vanishes** | ✅ | No goodbye, no clean close; remote still lets go |
| Input arrives in order over the reliable stream | ✅ | A key-up cannot overtake its key-down |
| A clean shutdown carries its reason | ✅ | Distinguishes "user quit" from "cable pulled" |
| An idle session stays open | ✅ | Heartbeats hold it for at least 6 s |

Not verified, and needing real machines:

| Check | Status | Notes |
|---|---|---|
| macOS → macOS with real capture and injection | ⬜ | Needs the permission grant plus a second Mac |
| Windows → Windows | ⬜ | Needs two Windows machines |
| **Windows ↔ macOS** | ⬜ | The case both key tables exist for |
| Input latency, p50/p99 | ⬜ | No figure has been measured. A VM cannot produce a trustworthy one |
| Behaviour on real Wi-Fi under contention | ⬜ | Loopback has no loss and no jitter |

## Milestone 7 — screen-edge switching

All logic, all headless. 41 tests across `mb-topology` and the handoff
coordinator, 7 of them property tests over arbitrary movement sequences.

| Check | Status | Notes |
|---|---|---|
| The cursor is always inside the screen it claims to be on | ✅ | Property test, arbitrary movement |
| The position never becomes non-finite | ✅ | Including NaN and infinite deltas |
| A crossing lands inside the screen it reports entering | ✅ | Property test |
| Entry positions are always within `0.0..=1.0` | ✅ | The receiver multiplies by its own screen size |
| **No single movement ever crosses** | ✅ | However fast or far; arriving at an edge is not crossing |
| Crossings respect the cooldown | ✅ | Property test with sub-cooldown time steps |
| Corners are excluded | ✅ | Menu bar, Start button, close buttons |
| Entry is proportional across mismatched screen sizes | ✅ | 4K panel to an 800-point laptop |
| A removed screen re-places the cursor | ✅ | Rather than stranding it off-desktop |
| Moving between two local screens is not a handoff | ✅ | Multi-monitor on one machine |
| The capture thread never blocks on layout state | ✅ | `try_lock`, skip-and-count |

Needing real hardware:

| Check | Status | Notes |
|---|---|---|
| The crossing threshold feels right in the hand | ⬜ | 12 points is a guess; only use can tune it |
| Corner dead zone does not block legitimate crossings | ⬜ | 8 points, likewise |
| Crossing behaviour on a mixed-DPI arrangement | ⬜ | See ADR 0001 |
| Handoff during an active drag | ⬜ | Button held across the boundary |

## Milestone 8 — multiple computers

Verified with three machines in one process, over two real QUIC connections:

| Check | Status | Notes |
|---|---|---|
| A layout spans every connected machine | ✅ | Mac — Windows — Mac Mini, 5760 points wide |
| Input reaches the selected machine and no other | ✅ | The far machine receives nothing meant for the middle one |
| Switching between peers routes correctly | ✅ | |
| **Losing the machine being typed into reclaims control** | ✅ | Over a real connection, with a modifier held |
| The departed machine releases what it was holding | ✅ | Autonomously, on its own session-closed event |
| A crossing towards a departed machine is refused | ✅ | The cursor stays put rather than vanishing |
| Losing an idle machine does not move control | ✅ | |
| A peer losing every screen reclaims control | ✅ | Display unplugged on the other machine |
| A degraded peer keeps control | ✅ | One lost packet must not yank the cursor back |
| Overlapping screens are reported, not accepted | ✅ | Two machines cannot own the same region |
| The layout is stable across rebuilds | ✅ | A reshuffling device list is unusable |

Needing real hardware:

| Check | Status | Notes |
|---|---|---|
| Three physical machines in a chain | ⬜ | |
| Crossing a chain end to end without stopping | ⬜ | Mac → Windows → Mac Mini in one motion |
| Behaviour when the middle machine of a chain leaves | ⬜ | The far machine becomes unreachable by cursor |

## Milestone 9 — multiple monitors and DPI

| Check | Status | Notes |
|---|---|---|
| macOS bounds are treated as already-shared units | ✅ | `CGDisplayBounds` accounts for user scaling |
| Windows bounds are divided by effective scale | ✅ | 3840 at 150% becomes 2560 |
| A Retina Mac and a scaled PC end up comparable | ✅ | Same perceived size, same shared size |
| The native space is preserved exactly | ✅ | Injection depends on it; no rounding |
| **Mixed-DPI screens on one device stay adjacent** | ✅ | Property test across every scale pairing |
| A converted arrangement is always a valid layout | ✅ | Property test |
| Multi-monitor devices keep their own arrangement | ✅ | The block is translated, never reflowed |
| A negative native origin normalises correctly | ✅ | Monitor left of the Windows primary |
| **A peer stranded by a departed neighbour is reported** | ✅ | Closes the milestone 8 gap |
| Corner-touching screens are not reachable | ✅ | One shared point is not a crossable edge |

Needing real hardware:

| Check | Status | Notes |
|---|---|---|
| A real mixed-DPI Windows arrangement | ⬜ | The assumption behind ADR 0001 is still unconfirmed on hardware |
| Cursor speed feels consistent across a DPI boundary | ⬜ | The conversion is arithmetically right; the feel is untested |
| Display hotplug mid-session rebuilds the layout | ⬜ | |
| macOS "looks like" scaling changes mid-session | ⬜ | |

## Milestone 10 — pairing

| Check | Status | Notes |
|---|---|---|
| Two strangers derive the same code | ✅ | Over a real pairing-mode connection |
| **An interposed attacker cannot make both screens agree** | ✅ | Attacker pairs with each side; codes differ |
| Both sides must confirm before anything is trusted | ✅ | One-sided confirmation yields no certificate |
| A rejected pairing yields nothing to trust | ✅ | Terminal; a later confirmation cannot revive it |
| A freshly paired device connects normally afterwards | ✅ | Pinned connection succeeds where it was refused before |
| A peer offering our own certificate is refused | ✅ | Loopback or replay; the code would trivially match |
| The code is order-independent | ✅ | Both sides compute the same value whoever initiated |
| A fresh nonce changes the code | ✅ | An observer of a past code learns nothing |
| Renaming a device does not change the code | ✅ | Names are cosmetic and user-editable |
| Codes are spread across the range | ✅ | 190+ distinct in 200 derivations |
| The pairing verifier still checks signatures | ✅ | Accepting any *identity* is not accepting any *claim* |

Needing a person:

| Check | Status | Notes |
|---|---|---|
| The code is genuinely comparable at a glance | ⬜ | Six digits grouped in threes; only use confirms it |
| The prompt makes a mismatch obvious | ⬜ | Milestone 13 |

## Milestone 11 — clipboard

The synchronisation rules and chunked transfer are complete and tested. The
platform watchers that detect a clipboard change are **not written**; see below.

| Check | Status | Notes |
|---|---|---|
| **A copy crosses once and stops** | ✅ | The loop, broken. Property-tested over arbitrary interleavings |
| Repeated notifications for one write are all suppressed | ✅ | Windows emits several; macOS coalesces |
| A genuine new copy is never swallowed | ✅ | The failure mode of a stuck suppression flag |
| Copying in both directions settles | ✅ | |
| A three-way exchange settles | ✅ | More paths for the loop to travel |
| Applications and suppressions stay balanced | ✅ | Property test; a growing gap means a slow loop |
| Text and an image with identical bytes do not collide | ✅ | |
| Clipboard contents never appear in `Debug` or `Display` | ✅ | Including inside a containing struct |
| Oversized content is refused, not truncated | ✅ | A silently shortened clipboard is worse than none |
| Remote content is re-validated on receipt | ✅ | The sender's claim is not evidence |
| A truncated transfer never becomes valid content | ✅ | Hash-checked; a partial file must not become a file |
| A lying offer cannot be exceeded | ✅ | Running total checked against the declared size |
| Out-of-order chunks are refused, not buffered | ✅ | Buffering would do an attacker's allocation |

Not yet written:

| Item | Status | Notes |
|---|---|---|
| macOS clipboard watcher | ⬜ | `NSPasteboard.changeCount` polling |
| Windows clipboard watcher | ⬜ | `AddClipboardFormatListener` |
| Reading and writing the actual clipboard | ⬜ | `arboard` for transfer, custom watcher for detection |
| Image formats beyond PNG | ⬜ | Deliberately deferred |

## Milestone 12 — file transfer

The state machine and destination-path safety are complete and tested. Streaming
bytes to disk is not written; see below.

| Check | Status | Notes |
|---|---|---|
| **Traversal cannot escape the download folder** | ✅ | Property-tested over arbitrary strings |
| A sanitised name is always one path component | ✅ | Property test |
| A destination lands directly inside the chosen folder | ✅ | Property test |
| Both separators are treated as separators everywhere | ✅ | A hostile peer sends whichever the receiver ignores |
| A null byte cannot survive | ✅ | It would truncate a path in any C API downstream |
| Windows-forbidden characters are replaced on both platforms | ✅ | Or transfers work one way and fail the other |
| Reserved device names are defused | ✅ | `CON.txt` is still `CON` |
| `CONTRACT.pdf` is left alone | ✅ | Only exact device names are mangled |
| Trailing dots and spaces are stripped | ✅ | Windows strips them silently, changing the file |
| Over-long names truncate on a character boundary | ✅ | |
| Sanitising is idempotent | ✅ | A second pass must not rename the file |
| **An existing file is never overwritten** | ✅ | Property-tested; suffixed instead |
| A transfer starts awaiting consent | ✅ | Paired is not permission to write files |
| An oversized offer is refused before prompting | ✅ | |
| Corrupted content fails rather than completing | ✅ | Hash-checked |
| A truncated transfer never completes | ✅ | |
| A cancellation crossing a completion cannot undo it | ✅ | Routine on a real network |

Not yet written:

| Item | Status | Notes |
|---|---|---|
| Streaming chunks to disk | ⬜ | Currently assembled in memory, bounded at 2 GiB |
| Removing the partial file on failure | ⬜ | Caller's job; the event says so |
| Multi-file transfers | ⬜ | One offer per file today |
| Drag-and-drop UI | ⬜ | Milestone 13 |

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
