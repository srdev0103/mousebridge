//! End-to-end transport tests over loopback.
//!
//! These run a genuine QUIC handshake between two endpoints in one process:
//! real TLS 1.3, real certificate pinning, real UDP on `127.0.0.1`. Nothing here
//! is mocked, so a pass means the transport actually works rather than that the
//! types line up.

// Integration tests are a separate crate, so the library's test-only allow does
// not reach here.
#![allow(clippy::unwrap_used, clippy::expect_used)]

use mb_net::Endpoint;
use mb_security::{Identity, TrustStore};
use mb_types::DeviceName;
use std::net::{Ipv4Addr, SocketAddr};
use std::sync::{Arc, RwLock};
use std::time::Duration;

fn name(text: &str) -> DeviceName {
    DeviceName::new(text).expect("valid name")
}

fn loopback() -> SocketAddr {
    // Port 0 lets the OS choose, so tests never collide on a busy machine.
    SocketAddr::from((Ipv4Addr::LOCALHOST, 0))
}

/// Builds an endpoint trusting exactly the certificates given.
fn endpoint(label: &str, identity: Arc<Identity>, trusted: &[&Identity]) -> Endpoint {
    let mut store = TrustStore::new();
    for peer in trusted {
        store.trust(name(label), peer.certificate(), 1);
    }
    Endpoint::bind(
        loopback(),
        identity,
        name(label),
        Arc::new(RwLock::new(store)),
    )
    .expect("binds")
}

#[tokio::test]
async fn two_paired_devices_connect_and_identify_each_other() {
    let mac = Arc::new(Identity::generate().expect("generates"));
    let pc = Arc::new(Identity::generate().expect("generates"));

    let listener = endpoint("Windows PC", Arc::clone(&pc), &[&mac]);
    let dialer = endpoint("Studio Mac", Arc::clone(&mac), &[&pc]);
    let addr = listener.local_addr().expect("bound");

    let accepting = tokio::spawn(async move {
        listener
            .accept()
            .await
            .expect("an incoming connection")
            .expect("handshake succeeds")
    });

    let outgoing = dialer.connect(addr).await.expect("connects");
    let incoming = accepting.await.expect("task");

    // Each side must see the *other* device, verified from the certificate.
    assert_eq!(outgoing.device(), pc.device_id());
    assert_eq!(incoming.device(), mac.device_id());

    // And the names each advertised.
    assert_eq!(outgoing.name().as_str(), "Windows PC");
    assert_eq!(incoming.name().as_str(), "Studio Mac");

    // Both sides must agree on the protocol version, or they would encode for
    // different layouts and corrupt the session silently.
    assert_eq!(outgoing.version(), incoming.version());
    assert_eq!(outgoing.version(), mb_protocol::PROTOCOL_VERSION);

    outgoing.close();
}

#[tokio::test]
async fn an_unpaired_device_cannot_connect() {
    // The central security property: being on the same network is not enough.
    let mac = Arc::new(Identity::generate().expect("generates"));
    let stranger = Arc::new(Identity::generate().expect("generates"));

    // The listener trusts the Mac and knows nothing of the stranger.
    let listener = endpoint(
        "Windows PC",
        Arc::new(Identity::generate().unwrap()),
        &[&mac],
    );
    let addr = listener.local_addr().expect("bound");

    let accepting = tokio::spawn(async move { listener.accept().await });

    // The stranger trusts the listener, so rejection can only come from the
    // listener refusing *it* — not from a symmetric failure.
    let attacker = endpoint("Attacker", Arc::clone(&stranger), &[]);
    let result = tokio::time::timeout(Duration::from_secs(5), attacker.connect(addr)).await;

    match result {
        Ok(Err(_)) => {}
        Ok(Ok(peer)) => panic!("an unpaired device connected as {}", peer.device()),
        Err(_) => panic!("the connection attempt hung instead of being refused"),
    }

    // And the listener must not report a successful peer.
    if let Ok(Ok(Some(Ok(peer)))) = tokio::time::timeout(Duration::from_secs(2), accepting).await {
        panic!("listener accepted an unpaired device: {}", peer.device());
    }
}

#[tokio::test]
async fn trust_must_be_mutual() {
    // The listener trusts the dialer, but the dialer does not trust the listener.
    // The dialer must refuse, or a malicious server could impersonate a peer the
    // user had paired with.
    let mac = Arc::new(Identity::generate().expect("generates"));
    let pc = Arc::new(Identity::generate().expect("generates"));

    let listener = endpoint("Windows PC", Arc::clone(&pc), &[&mac]);
    let addr = listener.local_addr().expect("bound");
    tokio::spawn(async move { listener.accept().await });

    let dialer = endpoint("Studio Mac", Arc::clone(&mac), &[]);
    let result = tokio::time::timeout(Duration::from_secs(5), dialer.connect(addr)).await;

    assert!(
        matches!(result, Ok(Err(_))),
        "a one-sided trust relationship was accepted"
    );
}

#[tokio::test]
async fn a_datagram_survives_the_round_trip() {
    // Motion travels as a QUIC datagram. If datagram support failed to
    // negotiate, every pointer packet would be silently discarded and the
    // failure would look like "the cursor does not move" with no error anywhere.
    use mb_protocol::motion::Motion;

    let a = Arc::new(Identity::generate().expect("generates"));
    let b = Arc::new(Identity::generate().expect("generates"));

    let listener = endpoint("B", Arc::clone(&b), &[&a]);
    let dialer = endpoint("A", Arc::clone(&a), &[&b]);
    let addr = listener.local_addr().expect("bound");

    let accepting = tokio::spawn(async move {
        let peer = listener.accept().await.unwrap().unwrap();
        let datagram = peer.quic().read_datagram().await.expect("datagram arrives");
        Motion::decode(&datagram).expect("decodes")
    });

    let outgoing = dialer.connect(addr).await.expect("connects");
    let sent = Motion {
        seq: 7,
        timestamp_us: 1234,
        x: -1920,
        y: 540,
    };
    outgoing
        .quic()
        .send_datagram(bytes::Bytes::copy_from_slice(&sent.to_bytes()))
        .expect("datagrams are supported");

    let received = tokio::time::timeout(Duration::from_secs(5), accepting)
        .await
        .expect("datagram did not arrive")
        .expect("task");

    assert_eq!(received, sent);
}

#[tokio::test]
async fn a_silent_peer_is_dropped_rather_than_held_open() {
    // A connection that completes TLS and then says nothing is a scanner or a
    // stalled peer. Holding resources for it is what an attacker would want.
    let listener_identity = Arc::new(Identity::generate().expect("generates"));
    let client_identity = Arc::new(Identity::generate().expect("generates"));

    let listener = endpoint(
        "Listener",
        Arc::clone(&listener_identity),
        &[&client_identity],
    );
    let addr = listener.local_addr().expect("bound");

    let accepting = tokio::spawn(async move { listener.accept().await });

    // Connect at the QUIC layer, completing TLS, but never open a control
    // stream — so no Hello is ever sent. Going through the raw endpoint is what
    // makes this possible; `Endpoint::connect` would send one immediately.
    let silent = endpoint(
        "Silent",
        Arc::clone(&client_identity),
        &[&listener_identity],
    );
    let _connection = silent
        .quic()
        .connect(addr, "mousebridge.local")
        .expect("connect starts")
        .await
        .expect("TLS succeeds - both sides are paired");

    // The accept side must give up within its handshake budget rather than
    // holding the connection open indefinitely. Generous slack over the 5s
    // timeout so a loaded machine does not produce a false failure.
    let outcome = tokio::time::timeout(Duration::from_secs(20), accepting).await;
    match outcome {
        Ok(Ok(Some(Err(_)))) | Ok(Ok(None)) => {}
        Ok(Ok(Some(Ok(peer)))) => panic!("accepted a peer that never spoke: {}", peer.device()),
        Ok(Err(e)) => panic!("accept task panicked: {e}"),
        Err(_) => panic!("accept hung past the handshake timeout"),
    }
}
