# ADR 0001 — Displays are reported in each device's native cursor space

- Status: accepted
- Date: 2026-09-01
- Milestone: 1

## Context

`DisplayInfo.bounds` uses the type `LogicalRect`, and the original architecture
said all geometry would be in DPI-normalised logical points. Implementing the two
backends showed that a single normalised space cannot be produced at the platform
layer without breaking something more important.

The two operating systems differ in a way that matters:

- **macOS** reports display bounds via `CGDisplayBounds` in logical points within
  one global display space. A Retina panel already appears as 2560x1440 with a
  backing scale factor of 2.0. Adjacency in that space is exact.
- **Windows** reports monitor rectangles via `GetMonitorInfoW` in the
  virtual-screen space, in **physical pixels**, for a Per-Monitor-V2 DPI-aware
  process. Each monitor carries its own effective DPI.

The tempting move is to divide each Windows monitor's rectangle by its own scale
factor at the platform boundary, producing "logical points" everywhere.

That is wrong. In a mixed-DPI arrangement — a 150% laptop panel beside a 100%
external monitor, which is the common case — dividing each rectangle by its own
factor moves the monitors relative to each other. Screens that are physically
adjacent in the virtual-screen space stop touching: they overlap or leave a gap.
The seam is exactly where edge crossing happens, so the error lands precisely on
the feature it would break.

Worse, cursor injection needs the *native* space. `SendInput` with
`MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK` normalises against the
virtual-screen rectangle in physical pixels. Having already discarded that space,
we would have to reconstruct it, and any rounding error becomes cursor drift.

## Decision

The platform layer reports `bounds` in **the device's own native cursor
coordinate space** and reports `scale` alongside it, without combining them.

- macOS: global display space, logical points. `scale` is the backing factor.
- Windows: virtual-screen space, physical pixels. `scale` is effective DPI / 96.

Normalisation across devices is the topology engine's responsibility. It has what
the platform layer does not: knowledge of *both* devices, so it can size screens
relative to one another for the layout editor while resolving crossings through
normalised `0.0..=1.0` edge positions, which are unit-free by construction.

`LogicalRect` keeps its name. The alternative — a second near-identical
`PhysicalRect` type — would double the geometry surface to encode a distinction
that only two call sites in the whole system care about.

## Consequences

- Cursor injection stays exact on both platforms; no reconstruction, no drift.
- Adjacency is preserved exactly as the OS reports it, including in mixed-DPI
  arrangements.
- The topology engine must never compare a raw coordinate from one device with a
  raw coordinate from another. Cross-device geometry goes through normalised edge
  positions. This is a real constraint and is enforced by the protocol: motion
  packets carry positions already resolved into the *target's* space.
- `DisplayInfo` documentation must state the space explicitly, because the type
  name alone no longer does. Done.
- Milestone 9 must include an explicit mixed-DPI adjacency test on real hardware.
  Listed in `docs/platform-validation.md`.

## Alternatives rejected

**Normalise to logical points at the platform boundary.** Breaks mixed-DPI
adjacency and discards the space injection needs. This is the option the original
architecture implied, and implementing it is what surfaced the problem.

**Introduce `PhysicalRect` and `LogicalRect` as distinct types.** Type-safe, and
genuinely tempting. Rejected because the conversion happens at two places and the
cost is a doubled geometry API plus conversions threaded through every call site
that does not care.
