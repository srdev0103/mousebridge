//! The platform-independent input event model.

use crate::keycode::KeyCode;
use crate::modifiers::{MouseButton, MouseButtons};
use mb_types::ScreenId;
use serde::{Deserialize, Serialize};
use std::fmt;

/// Scroll wheel movement.
///
/// Expressed in **lines**, fractionally. Lines rather than pixels because the two
/// platforms disagree about pixels and agree about lines: Windows reports notches
/// of `WHEEL_DELTA` (120) which the user's settings turn into a line count, while
/// macOS reports either line counts or pixel-precise deltas depending on the
/// device. Converting both to lines at the platform boundary means the model
/// carries one unit, and the receiver applies its own preferences.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct ScrollDelta {
    /// Horizontal lines. Positive is the direction the OS itself reports as
    /// positive, which on both platforms is "to the right" from the user.
    pub x: f32,
    /// Vertical lines. Positive is away from the user — the traditional "scroll
    /// up" direction. macOS `deltaAxis1` and the Windows `WHEEL_DELTA` sign
    /// already agree on this, so the value passes through unchanged.
    ///
    /// macOS applies its "natural scrolling" preference *before* the event
    /// reaches us, so a user with it enabled sends an already-inverted delta to a
    /// Windows machine that has no such preference. Whether that feels right is a
    /// question only real hardware can answer; it is listed for validation, and a
    /// per-peer inversion setting is the fallback if it does not.
    pub y: f32,
    /// True when the source device reported a continuous, pixel-precise delta —
    /// a trackpad or a free-spinning wheel — rather than discrete notches.
    ///
    /// The injector needs this: quantising a trackpad's smooth scroll to whole
    /// notches feels broken, and emitting fractional lines for a notched wheel
    /// makes some applications ignore the event entirely.
    pub precise: bool,
}

impl ScrollDelta {
    /// A notched vertical scroll, in whole lines.
    #[must_use]
    pub const fn lines(x: f32, y: f32) -> Self {
        Self {
            x,
            y,
            precise: false,
        }
    }

    /// A continuous scroll from a trackpad or free-spinning wheel.
    #[must_use]
    pub const fn precise(x: f32, y: f32) -> Self {
        Self {
            x,
            y,
            precise: true,
        }
    }

    /// Returns true when the delta would move nothing.
    ///
    /// Both platforms emit zero-delta scroll events — macOS for momentum phase
    /// changes, Windows for horizontal tilt release. Forwarding them wastes
    /// bandwidth on the hot path and can confuse the receiver's scroll physics.
    #[must_use]
    pub fn is_zero(self) -> bool {
        self.x == 0.0 && self.y == 0.0
    }

    /// Returns true if either axis is not finite.
    ///
    /// Defensive: this value can arrive from the network, and a NaN reaching a
    /// platform scroll API produces undefined behaviour in the receiving app.
    #[must_use]
    pub fn is_finite(self) -> bool {
        self.x.is_finite() && self.y.is_finite()
    }
}

/// Input state carried across a screen boundary.
///
/// When the pointer crosses to another machine, the keys and buttons the user is
/// *physically* holding must be re-asserted on the receiving side. Dragging a
/// window across the boundary with the left button down, or holding Shift while
/// crossing, would otherwise break the moment control transfers.
///
/// The complementary half is that the *sending* machine releases everything
/// locally, so nothing is left stuck there.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct HeldInput {
    /// Non-modifier keys physically held, in press order.
    pub keys: Vec<KeyCode>,
    /// Modifier keys physically held.
    pub modifiers: crate::modifiers::Modifiers,
    /// Mouse buttons physically held.
    pub buttons: MouseButtons,
}

impl HeldInput {
    /// True when nothing is held.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.keys.is_empty() && self.modifiers.is_empty() && self.buttons.is_empty()
    }
}

/// A single input event.
///
/// One enum spans both directions of the system, which is deliberate. The
/// alternative — separate `Captured` and `Injected` types — would double the
/// surface to express a distinction that only two variants actually care about.
/// Those two are called out below.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum InputEvent {
    /// Relative pointer motion, in the source device's cursor units.
    ///
    /// **Capture side.** This is what a mouse or trackpad produces. It is never
    /// injected directly: relative injection is subject to the receiving
    /// platform's pointer acceleration, which the sender has already applied
    /// once, so the motion would be accelerated twice and drift out of sync.
    ///
    /// Deltas are `f64` rather than integers because trackpads report sub-pixel
    /// movement, and truncating it makes slow, precise motion stutter or stall.
    MouseMove {
        /// Horizontal delta. Positive is right.
        dx: f64,
        /// Vertical delta. Positive is down.
        dy: f64,
    },

    /// Absolute pointer placement in the *target* device's cursor space.
    ///
    /// **Injection side.** The router resolves accumulated motion against the
    /// topology and emits a position the receiver can apply directly. Carrying an
    /// absolute position also makes motion self-healing over an unreliable
    /// transport: a dropped packet costs one frame instead of permanently
    /// desynchronising the two cursors.
    MouseMoveTo {
        /// Horizontal position in the target's cursor space.
        x: f64,
        /// Vertical position in the target's cursor space.
        y: f64,
    },

    /// A mouse button changed state.
    MouseButton {
        /// Which button.
        button: MouseButton,
        /// True for press, false for release.
        pressed: bool,
    },

    /// The scroll wheel or trackpad scrolled.
    MouseWheel {
        /// Movement in lines.
        delta: ScrollDelta,
    },

    /// A key changed state.
    Key {
        /// The physical key.
        key: KeyCode,
        /// True for press, false for release.
        pressed: bool,
        /// True when this press was generated by the OS auto-repeat rather than
        /// a fresh physical press.
        ///
        /// Tracked separately because a repeat must not be counted as a second
        /// press: doing so would make the release arithmetic wrong and leave the
        /// key marked as held forever.
        repeat: bool,
    },

    /// Control arrived at this device.
    Enter {
        /// Which of the receiving device's screens the pointer entered.
        screen: ScreenId,
        /// Where along that screen the pointer entered, each axis in `0.0..=1.0`.
        ///
        /// Normalised rather than absolute so that screens of different sizes and
        /// scale factors hand off proportionally.
        position: (f64, f64),
        /// Keys and buttons the user is physically holding.
        held: HeldInput,
    },

    /// Control left this device.
    Leave,

    /// Release everything currently held.
    ///
    /// The recovery primitive for the failure this system must never produce: a
    /// key stuck down on a machine the user is no longer controlling. Emitted on
    /// every boundary crossing, on disconnect, on sleep, on lock, and by the
    /// user-facing panic action.
    ///
    /// Idempotent by construction — releasing nothing is a no-op — so it is
    /// always safe to send one more.
    ReleaseAll,
}

impl InputEvent {
    /// True for events that occur at high frequency on the input path.
    ///
    /// Used to decide what may be coalesced or dropped under queue pressure.
    /// Motion is safe to drop because the next event supersedes it; a key or
    /// button transition never is, because the state change would be lost.
    #[must_use]
    pub const fn is_droppable(&self) -> bool {
        matches!(self, Self::MouseMove { .. } | Self::MouseMoveTo { .. })
    }

    /// True if this event carries no information and can be discarded.
    #[must_use]
    pub fn is_noop(&self) -> bool {
        match self {
            Self::MouseMove { dx, dy } => *dx == 0.0 && *dy == 0.0,
            Self::MouseWheel { delta } => delta.is_zero(),
            _ => false,
        }
    }

    /// True if every numeric field is finite.
    ///
    /// Events arriving from the network are untrusted. A NaN coordinate reaching
    /// `CGWarpMouseCursorPosition` or `SendInput` is not a rendering glitch — it
    /// is undefined behaviour in someone else's process.
    #[must_use]
    pub fn is_finite(&self) -> bool {
        match self {
            Self::MouseMove { dx, dy } => dx.is_finite() && dy.is_finite(),
            Self::MouseMoveTo { x, y } => x.is_finite() && y.is_finite(),
            Self::MouseWheel { delta } => delta.is_finite(),
            Self::Enter { position, .. } => position.0.is_finite() && position.1.is_finite(),
            _ => true,
        }
    }
}

impl fmt::Display for InputEvent {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MouseMove { dx, dy } => write!(f, "MouseMove({dx:+.1}, {dy:+.1})"),
            Self::MouseMoveTo { x, y } => write!(f, "MouseMoveTo({x:.1}, {y:.1})"),
            Self::MouseButton { button, pressed } => {
                write!(f, "Mouse{}({button})", if *pressed { "Down" } else { "Up" })
            }
            Self::MouseWheel { delta } => write!(f, "Wheel({:+.2}, {:+.2})", delta.x, delta.y),
            // Deliberately does not render which key. This type is not logged on
            // the input path, and making the rendering harmless is one more
            // barrier against a keystroke reaching a log by accident.
            Self::Key {
                pressed, repeat, ..
            } => write!(
                f,
                "Key{}{}",
                if *pressed { "Down" } else { "Up" },
                if *repeat { "(repeat)" } else { "" }
            ),
            Self::Enter { screen, .. } => write!(f, "Enter({screen})"),
            Self::Leave => f.write_str("Leave"),
            Self::ReleaseAll => f.write_str("ReleaseAll"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keycode::keys;

    #[test]
    fn only_motion_is_droppable() {
        assert!(InputEvent::MouseMove { dx: 1.0, dy: 0.0 }.is_droppable());
        assert!(InputEvent::MouseMoveTo { x: 5.0, y: 5.0 }.is_droppable());
        // Dropping any of these loses a state transition, which strands the
        // receiving machine holding something the user has let go of.
        assert!(
            !InputEvent::MouseButton {
                button: MouseButton::Left,
                pressed: true
            }
            .is_droppable()
        );
        assert!(
            !InputEvent::Key {
                key: keys::A,
                pressed: false,
                repeat: false
            }
            .is_droppable()
        );
        assert!(!InputEvent::ReleaseAll.is_droppable());
        assert!(
            !InputEvent::MouseWheel {
                delta: ScrollDelta::lines(0.0, 3.0)
            }
            .is_droppable()
        );
    }

    #[test]
    fn zero_events_are_recognised_as_noops() {
        assert!(InputEvent::MouseMove { dx: 0.0, dy: 0.0 }.is_noop());
        assert!(
            InputEvent::MouseWheel {
                delta: ScrollDelta::lines(0.0, 0.0)
            }
            .is_noop()
        );
        assert!(!InputEvent::MouseMove { dx: 0.0, dy: 0.5 }.is_noop());
        // An absolute move to (0,0) is meaningful: it is the top-left corner.
        assert!(!InputEvent::MouseMoveTo { x: 0.0, y: 0.0 }.is_noop());
    }

    #[test]
    fn non_finite_values_are_rejected() {
        assert!(
            !InputEvent::MouseMove {
                dx: f64::NAN,
                dy: 0.0
            }
            .is_finite()
        );
        assert!(
            !InputEvent::MouseMoveTo {
                x: f64::INFINITY,
                y: 0.0
            }
            .is_finite()
        );
        assert!(
            !InputEvent::MouseWheel {
                delta: ScrollDelta::precise(f32::NAN, 0.0)
            }
            .is_finite()
        );
        assert!(
            !InputEvent::Enter {
                screen: ScreenId(0),
                position: (f64::NAN, 0.5),
                held: HeldInput::default(),
            }
            .is_finite()
        );
        assert!(InputEvent::ReleaseAll.is_finite());
    }

    #[test]
    fn display_never_reveals_which_key_was_pressed() {
        // Rendering a keystroke is keylogging regardless of log level. This crate
        // has no logger, and the rendering is harmless as a second barrier.
        let rendered = InputEvent::Key {
            key: keys::A,
            pressed: true,
            repeat: false,
        }
        .to_string();
        assert_eq!(rendered, "KeyDown");
        assert!(!rendered.contains('A'));
    }

    #[test]
    fn events_round_trip_through_serde() {
        let events = vec![
            InputEvent::MouseMove { dx: -3.5, dy: 12.0 },
            InputEvent::MouseMoveTo { x: 1920.0, y: 0.0 },
            InputEvent::MouseButton {
                button: MouseButton::Middle,
                pressed: true,
            },
            InputEvent::MouseWheel {
                delta: ScrollDelta::precise(0.0, -1.25),
            },
            InputEvent::Key {
                key: keys::LEFT_META,
                pressed: true,
                repeat: false,
            },
            InputEvent::Enter {
                screen: ScreenId(2),
                position: (0.0, 0.75),
                held: HeldInput {
                    keys: vec![keys::A],
                    modifiers: crate::modifiers::Modifiers::LEFT_SHIFT,
                    buttons: MouseButtons::NONE.with(MouseButton::Left),
                },
            },
            InputEvent::Leave,
            InputEvent::ReleaseAll,
        ];
        for event in events {
            let json = serde_json::to_string(&event).expect("serialises");
            let back: InputEvent = serde_json::from_str(&json).expect("deserialises");
            assert_eq!(event, back, "round trip failed for {json}");
        }
    }

    #[test]
    fn held_input_emptiness() {
        assert!(HeldInput::default().is_empty());
        assert!(
            !HeldInput {
                keys: vec![keys::A],
                ..Default::default()
            }
            .is_empty()
        );
        assert!(
            !HeldInput {
                buttons: MouseButtons::NONE.with(MouseButton::Left),
                ..Default::default()
            }
            .is_empty()
        );
    }
}
