//! End-to-end session tests over a real QUIC connection.
//!
//! These carry actual input events between two authenticated peers on loopback.
//! The last one is the important one: it asserts the guarantee the whole product
//! rests on — that a machine holding keys down always finds out the session has
//! ended, so it can let go.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use mb_input::keycode::keys;
use mb_input::{InputEvent, InputStateTracker, MouseButton};
use mb_net::Endpoint;
use mb_net::session::{ClosedReason, Session, SessionEvent, SessionHandle};
use mb_protocol::message::{ControlMessage, DisconnectReason, InputMessage};
use mb_protocol::motion::Motion;
use mb_security::{Identity, TrustStore};
use mb_types::DeviceName;
use std::net::{Ipv4Addr, SocketAddr};
use std::sync::{Arc, RwLock};
use std::time::Duration;
use tokio::sync::mpsc::Receiver;

fn name(text: &str) -> DeviceName {
    DeviceName::new(text).expect("valid name")
}

fn endpoint(label: &str, identity: Arc<Identity>, trusted: &[&Identity]) -> Endpoint {
    let mut store = TrustStore::new();
    for peer in trusted {
        store.trust(name(label), peer.certificate(), 1);
    }
    Endpoint::bind(
        SocketAddr::from((Ipv4Addr::LOCALHOST, 0)),
        identity,
        name(label),
        Arc::new(RwLock::new(store)),
    )
    .expect("binds")
}

struct Pair {
    dialer: (SessionHandle, Receiver<SessionEvent>),
    listener: (SessionHandle, Receiver<SessionEvent>),
}

/// Connects two peers and drives both sessions.
async fn connected_pair() -> Pair {
    let a = Arc::new(Identity::generate().expect("generates"));
    let b = Arc::new(Identity::generate().expect("generates"));

    let listener_endpoint = endpoint("Listener", Arc::clone(&b), &[&a]);
    let dialer_endpoint = endpoint("Dialer", Arc::clone(&a), &[&b]);
    let addr = listener_endpoint.local_addr().expect("bound");

    let accepting = tokio::spawn(async move {
        listener_endpoint
            .accept()
            .await
            .expect("incoming")
            .expect("handshake")
    });

    let outgoing = dialer_endpoint.connect(addr).await.expect("connects");
    let incoming = accepting.await.expect("task");

    let (dial_handle, dial_events, dial_session) = Session::split(outgoing);
    let (listen_handle, listen_events, listen_session) = Session::split(incoming);
    tokio::spawn(dial_session.run());
    tokio::spawn(listen_session.run());

    Pair {
        dialer: (dial_handle, dial_events),
        listener: (listen_handle, listen_events),
    }
}

/// Waits for the next event matching a predicate, ignoring heartbeat noise.
async fn next_matching(
    events: &mut Receiver<SessionEvent>,
    mut wanted: impl FnMut(&SessionEvent) -> bool,
) -> SessionEvent {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        let remaining = deadline - tokio::time::Instant::now();
        let event = tokio::time::timeout(remaining, events.recv())
            .await
            .expect("timed out waiting for an event")
            .expect("session ended unexpectedly");
        if wanted(&event) {
            return event;
        }
    }
}

#[tokio::test]
async fn a_keystroke_travels_between_two_machines() {
    let mut pair = connected_pair().await;

    pair.dialer
        .0
        .send_input(InputMessage::Key {
            key: keys::A,
            pressed: true,
            repeat: false,
        })
        .expect("queued");

    let event = next_matching(&mut pair.listener.1, |e| {
        matches!(e, SessionEvent::Input(_))
    })
    .await;

    assert_eq!(
        event,
        SessionEvent::Input(InputMessage::Key {
            key: keys::A,
            pressed: true,
            repeat: false,
        })
    );
}

#[tokio::test]
async fn input_arrives_in_order() {
    // Ordering is the whole reason input uses a reliable stream. A key-up
    // overtaking its key-down would leave the key held forever.
    let mut pair = connected_pair().await;

    let sent = vec![
        InputMessage::Key {
            key: keys::LEFT_SHIFT,
            pressed: true,
            repeat: false,
        },
        InputMessage::Key {
            key: keys::A,
            pressed: true,
            repeat: false,
        },
        InputMessage::Key {
            key: keys::A,
            pressed: false,
            repeat: false,
        },
        InputMessage::Key {
            key: keys::LEFT_SHIFT,
            pressed: false,
            repeat: false,
        },
    ];
    for message in &sent {
        pair.dialer.0.send_input(message.clone()).expect("queued");
    }

    let mut received = Vec::new();
    while received.len() < sent.len() {
        if let SessionEvent::Input(input) = next_matching(&mut pair.listener.1, |e| {
            matches!(e, SessionEvent::Input(_))
        })
        .await
        {
            received.push(input);
        }
    }
    assert_eq!(received, sent);

    // And replaying them leaves the receiving machine holding nothing.
    let mut tracker = InputStateTracker::new();
    for message in &received {
        if let InputMessage::Key {
            key,
            pressed,
            repeat,
        } = message
        {
            tracker.apply(&InputEvent::Key {
                key: *key,
                pressed: *pressed,
                repeat: *repeat,
            });
        }
    }
    assert!(tracker.is_clean(), "replay left input held");
}

#[tokio::test]
async fn pointer_motion_travels_as_a_datagram() {
    let mut pair = connected_pair().await;

    let sent = Motion {
        seq: 42,
        timestamp_us: 99,
        x: 1234,
        y: -567,
    };
    pair.dialer.0.send_motion(sent).expect("sent");

    let event = next_matching(&mut pair.listener.1, |e| {
        matches!(e, SessionEvent::Motion(_))
    })
    .await;
    assert_eq!(event, SessionEvent::Motion(sent));
}

#[tokio::test]
async fn control_messages_travel_both_ways() {
    let mut pair = connected_pair().await;

    pair.listener
        .0
        .send_control(ControlMessage::CursorLeave)
        .expect("queued");

    let event = next_matching(&mut pair.dialer.1, |e| {
        matches!(e, SessionEvent::Control(ControlMessage::CursorLeave))
    })
    .await;
    assert_eq!(event, SessionEvent::Control(ControlMessage::CursorLeave));
}

#[tokio::test]
async fn a_session_stays_alive_while_idle() {
    // Heartbeats must keep an idle connection open. If they did not, a user who
    // stopped typing for six seconds would be disconnected.
    let mut pair = connected_pair().await;

    let quiet = tokio::time::timeout(Duration::from_secs(6), pair.listener.1.recv()).await;
    match quiet {
        Err(_) => {}
        Ok(Some(SessionEvent::Closed(reason))) => {
            panic!("an idle session closed itself: {reason:?}")
        }
        Ok(other) => panic!("unexpected event while idle: {other:?}"),
    }

    // And it still works afterwards.
    pair.dialer
        .0
        .send_input(InputMessage::ReleaseAll)
        .expect("still connected");
    let event = next_matching(&mut pair.listener.1, |e| {
        matches!(e, SessionEvent::Input(_))
    })
    .await;
    assert_eq!(event, SessionEvent::Input(InputMessage::ReleaseAll));
}

#[tokio::test]
async fn a_clean_shutdown_tells_the_peer_why() {
    // Distinguishing "the user quit" from "the cable was pulled" is what stops
    // the peer spending thirty seconds retrying something that is not coming back.
    let mut pair = connected_pair().await;

    pair.dialer
        .0
        .shutdown(DisconnectReason::Sleeping)
        .expect("queued");

    let event = next_matching(&mut pair.listener.1, |e| {
        matches!(e, SessionEvent::Closed(_))
    })
    .await;
    assert_eq!(
        event,
        SessionEvent::Closed(ClosedReason::PeerDisconnected(DisconnectReason::Sleeping))
    );
}

#[tokio::test]
async fn a_machine_holding_input_always_learns_the_session_ended() {
    // The guarantee the product rests on. A key pressed remotely, then the
    // sender vanishes without warning. The receiver must be told, or that key
    // stays down on a machine nobody is controlling any more.
    let mut pair = connected_pair().await;

    for message in [
        InputMessage::Key {
            key: keys::LEFT_CTRL,
            pressed: true,
            repeat: false,
        },
        InputMessage::Button {
            button: MouseButton::Left,
            pressed: true,
        },
    ] {
        pair.dialer.0.send_input(message).expect("queued");
    }

    // Track what the receiver is now holding, as the real injector would.
    let mut tracker = InputStateTracker::new();
    let mut seen = 0;
    while seen < 2 {
        if let SessionEvent::Input(input) = next_matching(&mut pair.listener.1, |e| {
            matches!(e, SessionEvent::Input(_))
        })
        .await
        {
            match input {
                InputMessage::Key {
                    key,
                    pressed,
                    repeat,
                } => {
                    tracker.apply(&InputEvent::Key {
                        key,
                        pressed,
                        repeat,
                    });
                }
                InputMessage::Button { button, pressed } => {
                    tracker.apply(&InputEvent::MouseButton { button, pressed });
                }
                _ => {}
            }
            seen += 1;
        }
    }
    assert!(!tracker.is_clean(), "the receiver should be holding input");

    // The sender disappears: no goodbye, no close, just gone.
    drop(pair.dialer);

    let event = next_matching(&mut pair.listener.1, |e| {
        matches!(e, SessionEvent::Closed(_))
    })
    .await;
    let SessionEvent::Closed(reason) = event else {
        unreachable!("filtered above")
    };
    assert_ne!(
        reason,
        ClosedReason::PeerDisconnected(DisconnectReason::Shutdown),
        "a vanished peer must not look like a clean disconnect"
    );

    // On that event, the receiver releases — and is clean again.
    for event in tracker.release_events() {
        tracker.apply(&event);
    }
    assert!(tracker.is_clean(), "receiver was left holding input");
}
