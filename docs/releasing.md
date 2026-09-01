# Releasing

## What has been verified on this machine

A Universal 2 macOS build, produced and checked locally:

```
$ lipo -info MouseBridge.app/Contents/MacOS/mousebridge-desktop
Architectures in the fat file: x86_64 arm64
```

| Property | Value |
|---|---|
| Bundle size | 14 MB |
| Bundle identifier | `com.mousebridge.desktop` |
| Minimum system version | 13.0 |
| Hardened runtime | enabled (`flags=0x10000(runtime)`) |
| Signature | valid, `--deep --strict` |
| Launches | yes, signed and unsigned |

### The designated requirement, and why it matters

```
designated => identifier "com.mousebridge.desktop"
              and anchor apple generic
              and certificate leaf[subject.CN] = "Apple Development: ..."
```

This resolves the highest-severity risk from the original architecture
assessment. The requirement is keyed to the **bundle identifier and the signing
certificate**, not to a hash of the binary — so a rebuild with the same identity
satisfies the same requirement, and **TCC keeps its Accessibility and Input
Monitoring grants across rebuilds**.

That was flagged as a risk that could halve development velocity for milestones
3 and 4. It is now measured rather than assumed: sign development builds with a
stable identity and the permissions persist.

## What cannot be done from this machine

### Notarisation

**Blocked, and it is a hard blocker for distribution.** Notarisation requires a
*Developer ID Application* certificate, issued through the Apple Developer
Program. Only an *Apple Development* certificate is present here, which is
sufficient for local signing and for TCC persistence, and is not accepted for
notarisation.

Without notarisation, macOS Gatekeeper refuses to open the app on any machine
other than the one that built it. There is no workaround short of asking every
user to bypass Gatekeeper manually, which is not a reasonable thing to ask.

Required before shipping:

1. An Apple Developer Program membership.
2. A Developer ID Application certificate.
3. These repository secrets, which `release.yml` already consumes:
   `APPLE_CERTIFICATE`, `APPLE_CERTIFICATE_PASSWORD`, `APPLE_SIGNING_IDENTITY`,
   `APPLE_ID`, `APPLE_PASSWORD` (an app-specific password), `APPLE_TEAM_ID`.

### Windows builds

**x64 cross-builds from macOS.** An earlier note here said this was impossible;
that was a statement about the toolchain then installed, not about the platform.

```sh
brew install llvm makensis
cargo install cargo-xwin
cd apps/desktop
npx tauri build --runner cargo-xwin --target x86_64-pc-windows-msvc --bundles nsis
```

Produces `MouseBridge_0.1.0_x64-setup.exe`. The installer stub is 32-bit, which is
normal for NSIS; the payload is a PE32+ x86-64 executable, verified by extraction.

**ARM64 does not cross-build**: `ring`'s build script fails for
`aarch64-pc-windows-msvc`. `release.yml` builds it on a native `windows-11-arm`
runner.

Cross-building proves the code compiles for Windows. **Nothing in the Windows
build has ever been executed.** See `docs/platform-validation.md`.

### Windows code signing

The NSIS installer is configured for a certificate thumbprint and a timestamp
server, and no certificate is configured. An unsigned Windows installer triggers
a SmartScreen warning that most users will not click through.

## Automatic updates

Not configured, deliberately. An update mechanism ships new code to users'
machines, and getting its signing wrong is a supply-chain compromise rather than
a bug. `tauri-plugin-updater` is the intended route; it needs a signing keypair
and an endpoint, and neither should be set up until there is somewhere real to
release to.

## Release checklist

1. `cargo test --workspace` — includes the unsafe-code audit.
2. `cargo clippy --workspace --all-targets -- -D warnings`.
3. Update the version in `Cargo.toml`, `apps/desktop/package.json`, and
   `tauri.conf.json`. They are not currently derived from one source, which is a
   known wart.
4. Work through `docs/platform-validation.md` on real hardware. Most of it is
   still unchecked; a release should not claim otherwise.
5. Tag `vX.Y.Z` and push. `release.yml` builds all four targets.
