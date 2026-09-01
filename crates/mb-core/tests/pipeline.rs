//! The whole pipeline, end to end, in one process.
//!
//! Capture on one machine, over a real authenticated QUIC connection, injected on
//! another. The only simulated parts are the two ends that touch hardware —
//! `VirtualCapture` stands in for an event tap, `VirtualInjector` for `SendInput`
//! — and both are real state machines, not stubs.
//!
//! This is what milestone 0's virtual backend was for: the entire feature is
//! verifiable without a second computer, a permission grant, or a human.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use mb_core::routing::{Destination, ReceiveState, Router, forward, to_input_event};
use mb_input::capture::InputCapture;
use mb_input::keycode::keys;
use mb_input::virtual_backend::{VirtualCapture, VirtualInjector};
use mb_input::{InputEvent, InputInject, MouseButton};
use mb_net::Endpoint;
use mb_net::session::{ClosedReason, Session, SessionEvent};
use mb_security::{Identity, TrustStore};
use mb_types::DeviceName;
use std::net::{Ipv4Addr, SocketAddr};
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;

fn name(text: &str) -> DeviceName {
    DeviceName::new(text).expect("valid")
}

fn endpoint(label: &str, identity: Arc<Identity>, trusted: &Identity) -> Endpoint {
    let mut store = TrustStore::new();
    store.trust(name(label), trusted.certificate(), 1);
    Endpoint::bind(
        SocketAddr::from((Ipv4Addr::LOCALHOST, 0)),
        identity,
        name(label),
        Arc::new(RwLock::new(store)),
    )
    .expect("binds")
}

/// The whole system: a sending machine and a receiving machine.
struct Rig {
    /// Drives capture on the sending machine.
    capture: VirtualCapture,
    /// Chooses whether input stays local or is forwarded.
    router: Router,
    /// Held only to keep the receiving session alive. Dropping a session handle
    /// ends the session, even one that is purely a receiver.
    _receiver_handle: mb_net::session::SessionHandle,
    /// What the receiving machine has been made to do.
    remote: Arc<Mutex<VirtualInjector>>,
    /// What the receiving machine believes it is holding.
    remote_state: Arc<Mutex<ReceiveState>>,
    /// Resolves when the receiving machine's session ends.
    closed: tokio::sync::watch::Receiver<Option<ClosedReason>>,
}

async fn build_rig() -> Rig {
    let sender_id = Arc::new(Identity::generate().expect("generates"));
    let receiver_id = Arc::new(Identity::generate().expect("generates"));

    let listener = endpoint("Receiver", Arc::clone(&receiver_id), &sender_id);
    let dialer = endpoint("Sender", Arc::clone(&sender_id), &receiver_id);
    let addr = listener.local_addr().expect("bound");

    let accepting = tokio::spawn(async move {
        listener
            .accept()
            .await
            .expect("incoming")
            .expect("handshake")
    });
    let outgoing = dialer.connect(addr).await.expect("connects");
    let incoming = accepting.await.expect("task");

    // --- sending machine ---
    let (send_handle, mut send_events, send_session) = Session::split(outgoing);
    tokio::spawn(send_session.run());
    // Drain the sender's inbound events. Letting this channel fill or close
    // would stall or end the session it belongs to.
    tokio::spawn(async move { while send_events.recv().await.is_some() {} });

    let (router, queue) = Router::new();
    let mut capture = VirtualCapture::new();
    capture.start(router.as_sink()).expect("starts");
    tokio::spawn(forward(queue, send_handle, router.monitor()));

    // --- receiving machine ---
    let (receiver_handle, mut recv_events, recv_session) = Session::split(incoming);
    tokio::spawn(recv_session.run());

    let remote = Arc::new(Mutex::new(VirtualInjector::new()));
    let remote_state = Arc::new(Mutex::new(ReceiveState::new()));
    let (closed_tx, closed) = tokio::sync::watch::channel(None);

    {
        let remote = Arc::clone(&remote);
        let remote_state = Arc::clone(&remote_state);
        tokio::spawn(async move {
            while let Some(event) = recv_events.recv().await {
                match event {
                    SessionEvent::Input(message) => {
                        let event = to_input_event(&message);
                        remote_state.lock().expect("lock").observe(&event);
                        let _ = remote.lock().expect("lock").inject(&event);
                    }
                    SessionEvent::Motion(motion) => {
                        let event = InputEvent::MouseMoveTo {
                            x: f64::from(motion.x),
                            y: f64::from(motion.y),
                        };
                        let _ = remote.lock().expect("lock").inject(&event);
                    }
                    SessionEvent::Closed(reason) => {
                        // The guarantee: however a session ends, release.
                        let releases = remote_state.lock().expect("lock").release_all();
                        let mut injector = remote.lock().expect("lock");
                        for release in releases {
                            let _ = injector.inject(&release);
                        }
                        let _ = closed_tx.send(Some(reason));
                        return;
                    }
                    _ => {}
                }
            }
        });
    }

    Rig {
        capture,
        router,
        _receiver_handle: receiver_handle,
        remote,
        remote_state,
        closed,
    }
}

/// Waits for a condition on the receiving machine.
async fn eventually(mut check: impl FnMut() -> bool) -> bool {
    for _ in 0..200 {
        if check() {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    false
}

#[tokio::test]
async fn a_keystroke_captured_here_lands_on_the_other_machine() {
    let mut rig = build_rig().await;
    rig.router.set_destination(Destination::Remote);

    rig.capture
        .emit(&InputEvent::Key {
            key: keys::A,
            pressed: true,
            repeat: false,
        })
        .expect("emitted");

    let remote = Arc::clone(&rig.remote);
    assert!(
        eventually(|| !remote.lock().expect("lock").events().is_empty()).await,
        "the keystroke never arrived"
    );

    let injector = rig.remote.lock().expect("lock");
    assert_eq!(injector.tracker().keys(), &[keys::A]);
}

#[tokio::test]
async fn local_input_never_leaves_this_machine() {
    // The default. Until the user crosses a boundary, nothing is forwarded.
    let mut rig = build_rig().await;

    for _ in 0..20 {
        rig.capture
            .emit(&InputEvent::Key {
                key: keys::A,
                pressed: true,
                repeat: false,
            })
            .expect("emitted");
    }

    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(
        rig.remote.lock().expect("lock").events().is_empty(),
        "local input leaked to the remote machine"
    );
    assert_eq!(rig.capture.stats().passed_through, 20);
}

#[tokio::test]
async fn pointer_motion_moves_the_remote_cursor() {
    let mut rig = build_rig().await;
    rig.router.set_destination(Destination::Remote);

    for _ in 0..10 {
        rig.capture
            .emit(&InputEvent::MouseMove { dx: 10.0, dy: -5.0 })
            .expect("emitted");
    }

    let remote = Arc::clone(&rig.remote);
    assert!(
        eventually(|| {
            let cursor = remote.lock().expect("lock").cursor();
            (cursor.0 - 100.0).abs() < 1.0 && (cursor.1 + 50.0).abs() < 1.0
        })
        .await,
        "cursor ended at {:?}, expected about (100, -50)",
        rig.remote.lock().expect("lock").cursor()
    );
}

#[tokio::test]
async fn a_chord_arrives_intact_and_in_order() {
    let mut rig = build_rig().await;
    rig.router.set_destination(Destination::Remote);

    for event in [
        InputEvent::Key {
            key: keys::LEFT_META,
            pressed: true,
            repeat: false,
        },
        InputEvent::Key {
            key: keys::C,
            pressed: true,
            repeat: false,
        },
        InputEvent::Key {
            key: keys::C,
            pressed: false,
            repeat: false,
        },
        InputEvent::Key {
            key: keys::LEFT_META,
            pressed: false,
            repeat: false,
        },
    ] {
        rig.capture.emit(&event).expect("emitted");
    }

    let remote = Arc::clone(&rig.remote);
    assert!(
        eventually(|| remote.lock().expect("lock").events().len() >= 4).await,
        "the chord never fully arrived"
    );

    let injector = rig.remote.lock().expect("lock");
    assert!(
        injector.is_clean(),
        "the remote machine is still holding {:?}",
        injector.tracker().keys()
    );
}

#[tokio::test]
async fn the_remote_machine_releases_when_the_sender_vanishes() {
    // The guarantee the whole product rests on, exercised through the real
    // transport: a key held down when the sending machine disappears without
    // warning must not stay down.
    let mut rig = build_rig().await;
    rig.router.set_destination(Destination::Remote);

    for event in [
        InputEvent::Key {
            key: keys::LEFT_CTRL,
            pressed: true,
            repeat: false,
        },
        InputEvent::MouseButton {
            button: MouseButton::Left,
            pressed: true,
        },
    ] {
        rig.capture.emit(&event).expect("emitted");
    }

    let state = Arc::clone(&rig.remote_state);
    assert!(
        eventually(|| !state.lock().expect("lock").is_clean()).await,
        "the remote machine never took the input"
    );

    // The sending machine disappears: no goodbye, no clean close. Dropping the
    // capture and the router closes the forwarding queue, which drops the
    // sending session's handle and takes the connection with it.
    drop(rig.capture);
    drop(rig.router);

    // It must find out, and let go.
    let closed = tokio::time::timeout(
        Duration::from_secs(20),
        rig.closed.wait_for(Option::is_some),
    )
    .await;
    assert!(
        closed.is_ok(),
        "the remote machine never learned the session ended"
    );

    assert!(
        rig.remote_state.lock().expect("lock").is_clean(),
        "the remote machine was left holding input"
    );
    assert!(
        rig.remote.lock().expect("lock").is_clean(),
        "the remote injector was left holding input"
    );
}
