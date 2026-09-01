//! Manual validation harness for the macOS input backend.
//!
//! Run with:
//!
//! ```sh
//! cargo run -p mb-input-native --example input-check -- --seconds 10
//! ```
//!
//! # Permissions
//!
//! A command-line binary inherits the TCC grants of the application that
//! launched it, so this harness needs **Terminal** (or your IDE) to hold
//! Accessibility and Input Monitoring, not the binary itself. That is convenient
//! for development and it is also a limitation worth stating plainly: it does
//! **not** validate the permission path a signed `.app` bundle takes, which has
//! its own identity. That check needs the bundled application.
//!
//! # What it does not print
//!
//! Which keys were pressed. This tool reports counts and event kinds only.
//! Printing keystrokes is keylogging regardless of intent, and a validation
//! harness is not a reason to build one.

use mb_input::capture::{Disposition, InputSink};
use mb_input::{InputCapture, InputEvent, InputInject};
use mb_input_native::macos::{MacCapture, MacInjector};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

#[derive(Default)]
struct Counters {
    moves: AtomicU64,
    buttons: AtomicU64,
    wheels: AtomicU64,
    keys: AtomicU64,
    modifiers: AtomicU64,
}

struct CountingSink {
    counters: Arc<Counters>,
}

impl InputSink for CountingSink {
    fn on_event(&self, event: &InputEvent) -> Disposition {
        let c = &self.counters;
        match event {
            InputEvent::MouseMove { .. } | InputEvent::MouseMoveTo { .. } => {
                c.moves.fetch_add(1, Ordering::Relaxed);
            }
            InputEvent::MouseButton { .. } => {
                c.buttons.fetch_add(1, Ordering::Relaxed);
            }
            InputEvent::MouseWheel { .. } => {
                c.wheels.fetch_add(1, Ordering::Relaxed);
            }
            InputEvent::Key { key, .. } => {
                if key.is_modifier() {
                    c.modifiers.fetch_add(1, Ordering::Relaxed);
                } else {
                    c.keys.fetch_add(1, Ordering::Relaxed);
                }
            }
            _ => {}
        }
        // Never suppress: this harness must not take the user's input away.
        Disposition::PassThrough
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let seconds: u64 = std::env::args()
        .collect::<Vec<_>>()
        .windows(2)
        .find(|w| w[0] == "--seconds")
        .and_then(|w| w[1].parse().ok())
        .unwrap_or(8);

    println!("== permissions ==");
    match mb_platform::current() {
        Ok(platform) => {
            for p in platform.required_permissions() {
                let status = platform.permission_status(*p);
                println!("  {:<18} {status:?}", p.to_string());
            }
            if !platform.missing_permissions().is_empty() {
                println!(
                    "\n  Grant Accessibility and Input Monitoring to your terminal,\n  \
                     then run this again. Capture will fail without both."
                );
            }
        }
        Err(e) => println!("  unavailable: {e}"),
    }

    println!("\n== injection self-test ==");
    match injection_self_test() {
        Ok(report) => println!("{report}"),
        Err(e) => println!("  FAILED: {e}"),
    }

    println!("\n== capture ==");
    let counters = Arc::new(Counters::default());
    let sink = Arc::new(CountingSink {
        counters: Arc::clone(&counters),
    });

    let mut capture = MacCapture::new();
    if let Err(e) = capture.start(sink) {
        println!("  could not start capture: {e}");
        return Ok(());
    }
    println!("  capturing for {seconds}s - move the mouse, type, scroll, hold modifiers");

    let deadline = Instant::now() + Duration::from_secs(seconds);
    while Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(200));
    }
    capture.stop()?;

    println!("\n== observed ==");
    println!("  mouse moves : {}", counters.moves.load(Ordering::Relaxed));
    println!(
        "  buttons     : {}",
        counters.buttons.load(Ordering::Relaxed)
    );
    println!(
        "  wheel       : {}",
        counters.wheels.load(Ordering::Relaxed)
    );
    println!("  keys        : {}", counters.keys.load(Ordering::Relaxed));
    println!(
        "  modifiers   : {}",
        counters.modifiers.load(Ordering::Relaxed)
    );

    let diag = capture.diagnostics();
    println!("\n== diagnostics ==");
    println!(
        "  tap re-arms   : {}{}",
        diag.rearms,
        if diag.rearms > 0 {
            "   <- the OS disabled the tap; the callback is too slow"
        } else {
            ""
        }
    );
    println!(
        "  unmapped keys : {}{}",
        diag.unmapped_keys,
        if diag.unmapped_keys > 0 {
            "   (expected if you pressed fn)"
        } else {
            ""
        }
    );
    Ok(())
}

/// Moves the cursor and confirms it actually moved, then puts it back.
fn injection_self_test() -> Result<String, Box<dyn std::error::Error>> {
    let start = cursor_position()?;
    let target = (start.0 + 60.0, start.1 + 40.0);

    let mut injector = MacInjector::new()?;
    injector.inject(&InputEvent::MouseMoveTo {
        x: target.0,
        y: target.1,
    })?;
    std::thread::sleep(Duration::from_millis(120));

    let after = cursor_position()?;
    let moved = (after.0 - target.0).abs() < 2.0 && (after.1 - target.1).abs() < 2.0;

    // Always put the cursor back, even if the assertion failed.
    injector.inject(&InputEvent::MouseMoveTo {
        x: start.0,
        y: start.1,
    })?;

    Ok(format!(
        "  cursor {:.0},{:.0} -> requested {:.0},{:.0} -> observed {:.0},{:.0}   [{}]",
        start.0,
        start.1,
        target.0,
        target.1,
        after.0,
        after.1,
        if moved { "PASS" } else { "FAIL" }
    ))
}

/// Reads the current cursor position from an empty CoreGraphics event.
fn cursor_position() -> Result<(f64, f64), Box<dyn std::error::Error>> {
    use core_graphics::event::CGEvent;
    use core_graphics::event_source::{CGEventSource, CGEventSourceStateID};

    let source = CGEventSource::new(CGEventSourceStateID::HIDSystemState)
        .map_err(|()| "could not create an event source")?;
    let event = CGEvent::new(source).map_err(|()| "could not create a probe event")?;
    let point = event.location();
    Ok((point.x, point.y))
}
