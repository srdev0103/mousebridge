//! Control-plane and input messages.
//!
//! Encoded with `serde` + `postcard`. These are low-rate and structurally varied,
//! so hand-rolling twenty layouts would trade a real risk of encoding bugs for a
//! saving that does not exist off the input path. Pointer motion, which *is* on
//! that path, is hand-written — see [`crate::motion`].

use crate::error::ProtocolError;
use crate::version::{Version, VersionRange};
use mb_input::event::HeldInput;
use mb_input::modifiers::MouseButton;
use mb_input::{KeyCode, ScrollDelta};
use mb_types::{Arch, DeviceId, DeviceName, OsKind, ScreenId};
use serde::{Deserialize, Serialize};

/// Largest encoded message accepted, matching the frame limit.
pub const MAX_MESSAGE_LEN: usize = crate::frame::MAX_FRAME_LEN;

/// The first message on a new connection.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Hello {
    /// Protocol versions this peer accepts.
    pub versions: VersionRange,
    /// The peer's stable identity.
    pub device: DeviceId,
    /// Display name, for the pairing prompt and the device list.
    pub name: DeviceName,
    /// Operating system family, shown in the UI.
    pub os: OsKind,
    /// CPU architecture, for diagnostics.
    pub arch: Arch,
}

/// Why a peer is disconnecting.
///
/// Sent so the other side can distinguish a deliberate shutdown from a network
/// failure. Without it, quitting the application on one machine is
/// indistinguishable from a cable being pulled, and the peer spends thirty
/// seconds retrying something that is never coming back.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DisconnectReason {
    /// The user quit or disabled sharing.
    Shutdown,
    /// The machine is going to sleep.
    Sleeping,
    /// The session was locked, so input must not be forwarded.
    Locked,
    /// The peer is not, or is no longer, trusted.
    NotPaired,
    /// The protocol was violated; the connection cannot continue.
    ProtocolError,
}

/// Messages on the control stream.
///
/// `Eq` is deliberately not derived: `CursorEnter` carries floating-point
/// coordinates, and an equality that ignores NaN would be a lie.
///
/// # Variant order is part of the wire format
///
/// Serde's *externally tagged* representation is used, which `postcard` encodes
/// as a varint variant index followed by the payload. An internally tagged
/// representation — `#[serde(tag = "...")]` — cannot be used here at all: it
/// relies on `deserialize_any`, which a non-self-describing binary format does
/// not implement, so it compiles and then fails at run time.
///
/// The consequence is that **reordering or removing a variant changes the wire
/// format**, silently, in a way that only shows up against a peer running the
/// other build. Add new variants at the end, and bump
/// [`crate::PROTOCOL_VERSION`] when removing one. `wire_layout_is_stable`
/// guards this.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ControlMessage {
    /// Opening handshake.
    Hello(Hello),
    /// Handshake accepted, with the negotiated version.
    HelloAck {
        /// Version both peers will use for the rest of the session.
        version: Version,
        /// The responder's identity.
        device: DeviceId,
        /// The responder's display name.
        name: DeviceName,
    },
    /// Liveness probe.
    Heartbeat {
        /// Echoed back in the reply, to match request and response.
        seq: u32,
        /// Sender's monotonic clock in microseconds.
        sent_us: u64,
    },
    /// Reply to a [`ControlMessage::Heartbeat`].
    HeartbeatAck {
        /// The `seq` from the probe.
        seq: u32,
        /// The `sent_us` from the probe, so the prober can compute a round trip
        /// without keeping per-probe state.
        sent_us: u64,
    },
    /// Control of the pointer is arriving at the receiving device.
    CursorEnter {
        /// Which of the receiver's screens the pointer entered.
        screen: ScreenId,
        /// Normalised position along the entry edge, each axis in `0.0..=1.0`.
        position: (f32, f32),
        /// Keys and buttons the user is physically holding.
        held: HeldInput,
    },
    /// Control of the pointer is leaving the receiving device.
    CursorLeave,
    /// Deliberate disconnection.
    Disconnect {
        /// Why.
        reason: DisconnectReason,
    },
}

/// Messages on the input stream.
///
/// Everything here changes state, so all of it needs reliable ordered delivery.
/// A lost key-up is a key stuck down on a machine the user has walked away from.
///
/// As with [`ControlMessage`], variant order is part of the wire format.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum InputMessage {
    /// A mouse button changed state.
    Button {
        /// Which button.
        button: MouseButton,
        /// True for press.
        pressed: bool,
    },
    /// The wheel or trackpad scrolled.
    Wheel {
        /// Movement in lines.
        delta: ScrollDelta,
    },
    /// A key changed state.
    Key {
        /// Physical key, as a HID usage.
        key: KeyCode,
        /// True for press.
        pressed: bool,
        /// True if generated by OS auto-repeat.
        repeat: bool,
    },
    /// Release everything currently held.
    ReleaseAll,
}

/// Encodes a message into a length-prefixed frame appended to `out`.
///
/// # Errors
///
/// [`ProtocolError::Malformed`] if serialisation fails, or
/// [`ProtocolError::FrameTooLarge`] if the result exceeds the frame limit.
pub fn encode<T: Serialize>(
    message: &T,
    what: &'static str,
    out: &mut Vec<u8>,
) -> Result<(), ProtocolError> {
    let bytes = postcard::to_stdvec(message).map_err(|_| ProtocolError::Malformed { what })?;
    crate::frame::encode_frame(&bytes, out)
}

/// Decodes a message from a frame payload.
///
/// # Errors
///
/// [`ProtocolError::Malformed`] if the payload does not deserialise, including
/// when it carries trailing bytes — a well-formed peer never sends those, and
/// accepting them would let a hostile peer smuggle data past a length check.
pub fn decode<T: for<'de> Deserialize<'de>>(
    payload: &[u8],
    what: &'static str,
) -> Result<T, ProtocolError> {
    let (value, rest) =
        postcard::take_from_bytes::<T>(payload).map_err(|_| ProtocolError::Malformed { what })?;
    if !rest.is_empty() {
        return Err(ProtocolError::Malformed { what });
    }
    Ok(value)
}

impl ControlMessage {
    /// Checks a decoded message for values that are structurally valid but
    /// semantically impossible.
    ///
    /// `serde` guarantees the shape; it cannot know that a normalised coordinate
    /// must lie in `0.0..=1.0` or that a held-key list has a bound. Both arrive
    /// from the network and both reach code that assumes otherwise.
    ///
    /// # Errors
    ///
    /// [`ProtocolError::Invalid`] naming the offending field.
    pub fn validate(&self) -> Result<(), ProtocolError> {
        match self {
            Self::CursorEnter { position, held, .. } => {
                if !position.0.is_finite() || !position.1.is_finite() {
                    return Err(ProtocolError::Invalid {
                        field: "cursor_enter.position",
                        reason: "coordinate is not a finite number",
                    });
                }
                if !(0.0..=1.0).contains(&position.0) || !(0.0..=1.0).contains(&position.1) {
                    return Err(ProtocolError::Invalid {
                        field: "cursor_enter.position",
                        reason: "normalised coordinate outside 0.0..=1.0",
                    });
                }
                if held.keys.len() > mb_input::MAX_HELD_KEYS {
                    return Err(ProtocolError::Invalid {
                        field: "cursor_enter.held.keys",
                        reason: "more held keys than any keyboard can report",
                    });
                }
                Ok(())
            }
            _ => Ok(()),
        }
    }
}

impl InputMessage {
    /// Checks a decoded message for impossible values.
    ///
    /// # Errors
    ///
    /// [`ProtocolError::Invalid`] naming the offending field.
    pub fn validate(&self) -> Result<(), ProtocolError> {
        match self {
            Self::Wheel { delta } if !delta.is_finite() => Err(ProtocolError::Invalid {
                field: "wheel.delta",
                reason: "scroll delta is not a finite number",
            }),
            _ => Ok(()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frame::{FrameDecode, decode_frame};
    use mb_input::keycode::keys;
    use mb_input::{Modifiers, MouseButtons};

    fn hello() -> Hello {
        Hello {
            versions: VersionRange::current(),
            device: DeviceId::from_bytes([7u8; 32]),
            name: DeviceName::new("Studio Mac").expect("valid"),
            os: OsKind::MacOs,
            arch: Arch::Aarch64,
        }
    }

    fn round_trip<T>(message: &T, what: &'static str) -> T
    where
        T: Serialize + for<'de> Deserialize<'de>,
    {
        let mut buf = Vec::new();
        encode(message, what, &mut buf).expect("encodes");
        match decode_frame(&buf).expect("frames") {
            FrameDecode::Complete { payload, consumed } => {
                assert_eq!(
                    consumed,
                    buf.len(),
                    "frame did not consume the whole buffer"
                );
                decode(payload, what).expect("decodes")
            }
            other => panic!("expected a complete frame, got {other:?}"),
        }
    }

    #[test]
    fn control_messages_round_trip() {
        let messages = vec![
            ControlMessage::Hello(hello()),
            ControlMessage::HelloAck {
                version: Version(1),
                device: DeviceId::from_bytes([9u8; 32]),
                name: DeviceName::new("Windows PC").expect("valid"),
            },
            ControlMessage::Heartbeat {
                seq: 42,
                sent_us: 1_234_567_890,
            },
            ControlMessage::HeartbeatAck {
                seq: 42,
                sent_us: 1_234_567_890,
            },
            ControlMessage::CursorEnter {
                screen: ScreenId(2),
                position: (0.0, 0.75),
                held: HeldInput {
                    keys: vec![keys::A],
                    modifiers: Modifiers::LEFT_SHIFT,
                    buttons: MouseButtons::NONE.with(MouseButton::Left),
                },
            },
            ControlMessage::CursorLeave,
            ControlMessage::Disconnect {
                reason: DisconnectReason::Sleeping,
            },
        ];
        for message in messages {
            assert_eq!(round_trip(&message, "control"), message);
        }
    }

    #[test]
    fn input_messages_round_trip() {
        let messages = vec![
            InputMessage::Button {
                button: MouseButton::Middle,
                pressed: true,
            },
            InputMessage::Wheel {
                delta: ScrollDelta::precise(0.0, -1.25),
            },
            InputMessage::Key {
                key: keys::LEFT_META,
                pressed: true,
                repeat: false,
            },
            InputMessage::ReleaseAll,
        ];
        for message in messages {
            assert_eq!(round_trip(&message, "input"), message);
        }
    }

    #[test]
    fn a_truncated_payload_is_rejected_not_partially_read() {
        let mut buf = Vec::new();
        encode(&ControlMessage::Hello(hello()), "control", &mut buf).expect("encodes");
        let payload = &buf[4..];

        for len in 0..payload.len() {
            let result = decode::<ControlMessage>(&payload[..len], "control");
            assert!(result.is_err(), "accepted a {len}-byte truncation");
        }
    }

    #[test]
    fn trailing_bytes_are_rejected() {
        // A well-formed peer never sends these. Accepting them would let a
        // hostile peer append data that passed the length check unread.
        let mut buf = Vec::new();
        encode(&ControlMessage::CursorLeave, "control", &mut buf).expect("encodes");
        let mut payload = buf[4..].to_vec();
        payload.push(0xFF);
        assert!(decode::<ControlMessage>(&payload, "control").is_err());
    }

    #[test]
    fn arbitrary_bytes_never_panic() {
        // Every byte string here is something a hostile peer can send.
        for seed in 0u16..2000 {
            let bytes: Vec<u8> = (0..16).map(|i| (seed.wrapping_mul(31) ^ i) as u8).collect();
            let _ = decode::<ControlMessage>(&bytes, "control");
            let _ = decode::<InputMessage>(&bytes, "input");
            let _ = decode::<Hello>(&bytes, "hello");
        }
    }

    #[test]
    fn a_normalised_position_outside_the_unit_range_is_rejected() {
        // The receiver uses this to place the cursor on a screen. A value of 40.0
        // would put it forty screens away, or nowhere at all.
        let bad = ControlMessage::CursorEnter {
            screen: ScreenId(0),
            position: (40.0, 0.5),
            held: HeldInput::default(),
        };
        let err = bad.validate().expect_err("must reject");
        assert!(matches!(
            err,
            ProtocolError::Invalid {
                field: "cursor_enter.position",
                ..
            }
        ));
    }

    #[test]
    fn a_non_finite_position_is_rejected() {
        for value in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            let bad = ControlMessage::CursorEnter {
                screen: ScreenId(0),
                position: (value, 0.5),
                held: HeldInput::default(),
            };
            assert!(bad.validate().is_err(), "accepted {value}");
        }
    }

    #[test]
    fn an_absurd_held_key_list_is_rejected() {
        // A peer claiming 10,000 held keys is malfunctioning or probing.
        let bad = ControlMessage::CursorEnter {
            screen: ScreenId(0),
            position: (0.5, 0.5),
            held: HeldInput {
                keys: vec![keys::A; mb_input::MAX_HELD_KEYS + 1],
                modifiers: Modifiers::NONE,
                buttons: MouseButtons::NONE,
            },
        };
        assert!(bad.validate().is_err());
    }

    #[test]
    fn a_non_finite_scroll_delta_is_rejected() {
        let bad = InputMessage::Wheel {
            delta: ScrollDelta::precise(f32::NAN, 0.0),
        };
        assert!(bad.validate().is_err());
    }

    #[test]
    fn valid_messages_pass_validation() {
        assert!(ControlMessage::CursorLeave.validate().is_ok());
        assert!(ControlMessage::Hello(hello()).validate().is_ok());
        assert!(
            ControlMessage::CursorEnter {
                screen: ScreenId(0),
                position: (0.0, 1.0),
                held: HeldInput::default(),
            }
            .validate()
            .is_ok()
        );
        assert!(InputMessage::ReleaseAll.validate().is_ok());
    }

    #[test]
    fn wire_layout_is_stable() {
        // Postcard encodes an externally tagged enum as a varint variant index,
        // so inserting or reordering a variant silently changes the wire format.
        // Nothing else would catch it: both sides of a single build agree
        // perfectly, and only a peer on the other build sees the corruption.
        //
        // These bytes are a fixed point. If this test fails, either restore the
        // variant order or bump PROTOCOL_VERSION deliberately.
        let cases: Vec<(ControlMessage, &[u8])> = vec![
            (
                ControlMessage::HeartbeatAck { seq: 1, sent_us: 2 },
                &[0x03, 0x01, 0x02],
            ),
            (ControlMessage::CursorLeave, &[0x05]),
            (
                ControlMessage::Disconnect {
                    reason: DisconnectReason::Locked,
                },
                &[0x06, 0x02],
            ),
        ];
        for (message, expected) in cases {
            let encoded = postcard::to_stdvec(&message).expect("encodes");
            assert_eq!(
                encoded, expected,
                "wire layout changed for {message:?} - see the note on ControlMessage"
            );
        }

        assert_eq!(
            postcard::to_stdvec(&InputMessage::ReleaseAll).expect("encodes"),
            vec![0x03],
        );
    }

    #[test]
    fn an_internally_tagged_enum_would_not_survive_postcard() {
        // Documents why `#[serde(tag = "...")]` is absent above. The internally
        // tagged representation needs `deserialize_any`, which a
        // non-self-describing format cannot provide, so it compiles cleanly and
        // then fails at run time on every message.
        #[derive(Serialize, Deserialize, Debug, PartialEq)]
        #[serde(tag = "t")]
        enum Tagged {
            A { v: u8 },
        }
        let bytes = postcard::to_stdvec(&Tagged::A { v: 1 }).expect("encodes");
        assert!(
            postcard::from_bytes::<Tagged>(&bytes).is_err(),
            "postcard gained self-describing support; the note above can be revisited"
        );
    }

    #[test]
    fn a_hello_stays_comfortably_inside_one_frame() {
        // The handshake must never be the thing that trips the frame limit.
        let mut buf = Vec::new();
        encode(&ControlMessage::Hello(hello()), "control", &mut buf).expect("encodes");
        assert!(buf.len() < 256, "hello grew to {} bytes", buf.len());
    }
}
