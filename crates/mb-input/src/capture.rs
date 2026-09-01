//! Capturing input from the operating system.

use crate::error::InputError;
use crate::event::InputEvent;
use std::sync::Arc;

/// What the router decided should happen to a captured event locally.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Disposition {
    /// Let the event reach the local machine as normal.
    ///
    /// Used when the pointer is on this machine's own screens.
    PassThrough,
    /// Swallow the event so the local machine never sees it.
    ///
    /// Used when control is on another machine. This is what stops the local
    /// cursor moving and the local applications receiving keystrokes while the
    /// user is typing into a different computer.
    Suppress,
}

/// Receives captured events.
///
/// # Contract
///
/// [`InputSink::on_event`] is called **directly on the operating system's input
/// thread** — a CoreGraphics event tap callback on macOS, a `WH_*_LL` hook
/// procedure on Windows. Both platforms police how long that call may take, and
/// both respond to a slow one by silently disabling the capture.
///
/// An implementation must therefore not:
///
/// * block, lock a contended mutex, or wait on a channel that can be full,
/// * allocate,
/// * perform I/O of any kind, including logging,
/// * panic — unwinding across the FFI boundary is undefined behaviour.
///
/// The only correct shape is: decide, push into a pre-allocated bounded queue,
/// return.
pub trait InputSink: Send + Sync + 'static {
    /// Handles one captured event and says what should happen to it locally.
    fn on_event(&self, event: &InputEvent) -> Disposition;
}

/// A platform's input capture backend.
pub trait InputCapture: Send {
    /// Begins capturing, delivering events to `sink`.
    ///
    /// # Errors
    ///
    /// [`InputError::PermissionDenied`] when the OS gate is not granted, or
    /// [`InputError::OsCall`] if the tap or hook could not be installed.
    fn start(&mut self, sink: Arc<dyn InputSink>) -> Result<(), InputError>;

    /// Stops capturing and releases OS resources.
    ///
    /// Must be safe to call when not running.
    ///
    /// # Errors
    ///
    /// [`InputError::OsCall`] if the OS refused to release the hook.
    fn stop(&mut self) -> Result<(), InputError>;

    /// True while capture is active.
    fn is_running(&self) -> bool;

    /// Re-arms capture after the OS disabled it.
    ///
    /// Both platforms revoke capture under load without notification.
    /// A supervisor calls this on [`InputError::CaptureDisabled`]; without it,
    /// the application keeps running while silently capturing nothing, which is
    /// the single most common failure mode in this class of software.
    ///
    /// # Errors
    ///
    /// [`InputError::OsCall`] if re-arming failed.
    fn rearm(&mut self) -> Result<(), InputError>;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keycode::keys;
    use std::sync::atomic::{AtomicUsize, Ordering};

    struct CountingSink {
        seen: AtomicUsize,
        disposition: Disposition,
    }

    impl InputSink for CountingSink {
        fn on_event(&self, _event: &InputEvent) -> Disposition {
            self.seen.fetch_add(1, Ordering::Relaxed);
            self.disposition
        }
    }

    #[test]
    fn a_sink_is_usable_as_a_trait_object_across_threads() {
        // The real capture thread is owned by the OS, so the sink must be
        // shareable without the backend knowing its concrete type.
        let sink = Arc::new(CountingSink {
            seen: AtomicUsize::new(0),
            disposition: Disposition::Suppress,
        });
        let erased: Arc<dyn InputSink> = sink.clone();

        let handle = std::thread::spawn(move || {
            erased.on_event(&InputEvent::Key {
                key: keys::A,
                pressed: true,
                repeat: false,
            })
        });
        assert_eq!(handle.join().expect("thread"), Disposition::Suppress);
        assert_eq!(sink.seen.load(Ordering::Relaxed), 1);
    }
}
