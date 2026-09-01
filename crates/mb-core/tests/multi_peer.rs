//! Three machines, two real QUIC connections, one keyboard.
//!
//! The single-peer pipeline test proves input can travel. This proves it travels
//! to the *right* machine, and that losing the machine currently receiving it
//! does not strand the user's hands.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use mb_core::peers::{PeerChange, PeerSet, PeerState, ReclaimReason};
use mb_core::routing::{ReceiveState, to_input_event};
use mb_input::InputInject;
use mb_input::keycode::keys;
use mb_input::virtual_backend::VirtualInjector;
use mb_net::Endpoint;
use mb_net::session::{Session, SessionEvent, SessionHandle};
use mb_protocol::message::InputMessage;
use mb_security::{Identity, TrustStore};
use mb_topology::layout::PlacedScreen;
use mb_types::{DeviceId, DeviceName, GlobalScreenId, LogicalRect, Scale, ScreenId};
use std::net::{Ipv4Addr, SocketAddr};
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;

fn name(text: &str) -> DeviceName {
    DeviceName::new(text).expect("valid")
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

fn screen(device: DeviceId, x: f64) -> PlacedScreen {
    PlacedScreen {
        id: GlobalScreenId::new(device, ScreenId(0)),
        bounds: LogicalRect::from_parts(x, 0.0, 1920.0, 1080.0).expect("valid"),
        scale: Scale::ONE,
    }
}

/// A machine that receives input and records what it was made to do.
struct Receiver {
    injector: Arc<Mutex<VirtualInjector>>,
    state: Arc<Mutex<ReceiveState>>,
    closed: tokio::sync::watch::Receiver<bool>,
    /// Held to keep the session alive; a receive-only session dies without it.
    _handle: SessionHandle,
}

/// Connects one peer to the hub and starts driving both sides.
///
/// The peer's identity is supplied by the caller so the hub can be built
/// trusting it up front — trust has to be mutual before the handshake, not after.
async fn attach_with(
    hub: &Endpoint,
    hub_identity: &Arc<Identity>,
    label: &str,
    peer_identity: &Arc<Identity>,
) -> (DeviceId, SessionHandle, Receiver) {
    let listener = endpoint(label, Arc::clone(peer_identity), &[hub_identity]);
    let addr = listener.local_addr().expect("bound");

    let accepting = tokio::spawn(async move {
        listener
            .accept()
            .await
            .expect("incoming")
            .expect("handshake")
    });
    let outgoing = hub.connect(addr).await.expect("connects");
    let incoming = accepting.await.expect("task");

    let (hub_handle, mut hub_events, hub_session) = Session::split(outgoing);
    tokio::spawn(hub_session.run());
    tokio::spawn(async move { while hub_events.recv().await.is_some() {} });

    let (peer_handle, mut peer_events, peer_session) = Session::split(incoming);
    tokio::spawn(peer_session.run());

    let injector = Arc::new(Mutex::new(VirtualInjector::new()));
    let state = Arc::new(Mutex::new(ReceiveState::new()));
    let (closed_tx, closed) = tokio::sync::watch::channel(false);

    {
        let injector = Arc::clone(&injector);
        let state = Arc::clone(&state);
        tokio::spawn(async move {
            while let Some(event) = peer_events.recv().await {
                match event {
                    SessionEvent::Input(message) => {
                        let event = to_input_event(&message);
                        state.lock().expect("lock").observe(&event);
                        let _ = injector.lock().expect("lock").inject(&event);
                    }
                    SessionEvent::Closed(_) => {
                        let releases = state.lock().expect("lock").release_all();
                        let mut guard = injector.lock().expect("lock");
                        for release in releases {
                            let _ = guard.inject(&release);
                        }
                        let _ = closed_tx.send(true);
                        return;
                    }
                    _ => {}
                }
            }
        });
    }

    let device = hub_handle.device();
    (
        device,
        hub_handle,
        Receiver {
            injector,
            state,
            closed,
            _handle: peer_handle,
        },
    )
}

async fn eventually(mut check: impl FnMut() -> bool) -> bool {
    for _ in 0..200 {
        if check() {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    false
}

fn press(key: mb_input::KeyCode) -> InputMessage {
    InputMessage::Key {
        key,
        pressed: true,
        repeat: false,
    }
}

#[tokio::test]
async fn input_goes_to_the_selected_machine_and_no_other() {
    let hub_identity = Arc::new(Identity::generate().expect("generates"));

    // The hub must trust both peers, so build it after generating them.
    let left_identity = Arc::new(Identity::generate().expect("generates"));
    let right_identity = Arc::new(Identity::generate().expect("generates"));
    let hub = {
        let mut store = TrustStore::new();
        store.trust(name("left"), left_identity.certificate(), 1);
        store.trust(name("right"), right_identity.certificate(), 1);
        Endpoint::bind(
            SocketAddr::from((Ipv4Addr::LOCALHOST, 0)),
            Arc::clone(&hub_identity),
            name("Hub Mac"),
            Arc::new(RwLock::new(store)),
        )
        .expect("binds")
    };

    // Bring up two peers that each trust the hub.
    let (left_device, left_handle, left) =
        attach_with(&hub, &hub_identity, "Windows PC", &left_identity).await;
    let (right_device, right_handle, right) =
        attach_with(&hub, &hub_identity, "Mac Mini", &right_identity).await;
    assert_ne!(left_device, right_device);

    // Mac — Windows — Mac Mini.
    let local = hub_identity.device_id();
    let mut peers = PeerSet::new(local, vec![screen(local, 0.0)]);
    peers.connect(
        left_device,
        name("Windows PC"),
        vec![screen(left_device, 1920.0)],
        Some(left_handle),
    );
    peers.connect(
        right_device,
        name("Mac Mini"),
        vec![screen(right_device, 3840.0)],
        Some(right_handle),
    );

    let layout = peers.layout().expect("valid");
    assert_eq!(layout.screens().len(), 3);
    assert_eq!(layout.bounding_box().max_x(), 5760.0);

    // Direct input at the middle machine.
    peers.set_active(Some(left_device)).expect("connected");
    peers
        .active_handle()
        .expect("has a handle")
        .send_input(press(keys::A))
        .expect("queued");

    let left_injector = Arc::clone(&left.injector);
    assert!(
        eventually(|| !left_injector.lock().expect("lock").events().is_empty()).await,
        "the selected machine never received the keystroke"
    );
    assert!(
        right.injector.lock().expect("lock").events().is_empty(),
        "the keystroke leaked to the machine that was not selected"
    );

    // Now the far machine.
    peers.set_active(Some(right_device)).expect("connected");
    peers
        .active_handle()
        .expect("has a handle")
        .send_input(press(keys::Z))
        .expect("queued");

    let right_injector = Arc::clone(&right.injector);
    assert!(
        eventually(|| !right_injector.lock().expect("lock").events().is_empty()).await,
        "the far machine never received the keystroke"
    );
    assert_eq!(
        left.injector.lock().expect("lock").events().len(),
        1,
        "the middle machine received input meant for the far one"
    );
}

#[tokio::test]
async fn losing_the_machine_being_typed_into_reclaims_control_and_releases() {
    let hub_identity = Arc::new(Identity::generate().expect("generates"));
    let peer_identity = Arc::new(Identity::generate().expect("generates"));

    let hub = {
        let mut store = TrustStore::new();
        store.trust(name("peer"), peer_identity.certificate(), 1);
        Endpoint::bind(
            SocketAddr::from((Ipv4Addr::LOCALHOST, 0)),
            Arc::clone(&hub_identity),
            name("Hub Mac"),
            Arc::new(RwLock::new(store)),
        )
        .expect("binds")
    };

    let (device, handle, mut receiver) =
        attach_with(&hub, &hub_identity, "Windows PC", &peer_identity).await;

    let local = hub_identity.device_id();
    let mut peers = PeerSet::new(local, vec![screen(local, 0.0)]);
    peers.connect(
        device,
        name("Windows PC"),
        vec![screen(device, 1920.0)],
        Some(handle.clone()),
    );
    peers.set_active(Some(device)).expect("connected");

    // Hold a modifier down on the remote machine.
    handle.send_input(press(keys::LEFT_CTRL)).expect("queued");

    let state = Arc::clone(&receiver.state);
    assert!(
        eventually(|| !state.lock().expect("lock").is_clean()).await,
        "the remote machine never took the modifier"
    );

    // The machine being typed into disappears.
    drop(handle);
    peers.set_state(device, PeerState::Degraded { missed: 3 });
    let change = peers.disconnect(device);

    // Control must come back here immediately.
    assert_eq!(
        change,
        PeerChange::ReclaimControl {
            from: device,
            reason: ReclaimReason::PeerLost,
        }
    );
    assert_eq!(peers.active(), None);
    assert!(peers.layout().expect("valid").screens().len() == 1);

    // And the departed machine must have let go of the modifier on its own.
    assert!(
        tokio::time::timeout(Duration::from_secs(20), receiver.closed.wait_for(|c| *c))
            .await
            .is_ok(),
        "the remote machine never learned the session ended"
    );
    assert!(
        receiver.state.lock().expect("lock").is_clean(),
        "the remote machine was left holding a modifier"
    );
    assert!(receiver.injector.lock().expect("lock").is_clean());
}
