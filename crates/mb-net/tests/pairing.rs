//! Pairing two devices that have never met, over a real connection.
//!
//! The security claim being tested: an attacker positioned between the two
//! machines cannot make both screens show the same verification code. Everything
//! else about the trust model rests on that.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use mb_net::Endpoint;
use mb_net::session::{Session, SessionEvent, SessionHandle};
use mb_protocol::message::ControlMessage;
use mb_security::pairing::{PairingOffer, PairingSession, PairingState};
use mb_security::{Identity, TrustStore};
use mb_types::DeviceName;
use std::net::{Ipv4Addr, SocketAddr};
use std::sync::{Arc, RwLock};
use std::time::Duration;
use tokio::sync::mpsc::Receiver;

fn name(text: &str) -> DeviceName {
    DeviceName::new(text).expect("valid")
}

async fn next_control(events: &mut Receiver<SessionEvent>) -> ControlMessage {
    let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
    loop {
        let event = tokio::time::timeout(deadline - tokio::time::Instant::now(), events.recv())
            .await
            .expect("timed out")
            .expect("session ended");
        if let SessionEvent::Control(control) = event {
            return control;
        }
    }
}

/// One side of a pairing attempt, as the application would drive it.
struct Side {
    session: PairingSession,
    handle: SessionHandle,
    events: Receiver<SessionEvent>,
}

/// Connects two unpaired devices in pairing mode and exchanges nonces.
async fn exchange(
    dialer_identity: &Arc<Identity>,
    dialer_name: &str,
    listener_identity: &Arc<Identity>,
    listener_name: &str,
) -> (Side, Side) {
    let listener = Endpoint::bind_for_pairing(
        SocketAddr::from((Ipv4Addr::LOCALHOST, 0)),
        Arc::clone(listener_identity),
        name(listener_name),
    )
    .expect("binds");
    let dialer = Endpoint::bind_for_pairing(
        SocketAddr::from((Ipv4Addr::LOCALHOST, 0)),
        Arc::clone(dialer_identity),
        name(dialer_name),
    )
    .expect("binds");

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

    // Each side builds its offer from the certificate TLS authenticated, not
    // from anything the peer merely claimed.
    let dialer_offer = PairingOffer {
        certificate: dialer_identity.certificate().as_ref().to_vec(),
        nonce: *mb_security::pairing::generate_nonce()
            .expect("entropy")
            .expose(),
        name: name(dialer_name),
    };
    let listener_offer = PairingOffer {
        certificate: listener_identity.certificate().as_ref().to_vec(),
        nonce: *mb_security::pairing::generate_nonce()
            .expect("entropy")
            .expose(),
        name: name(listener_name),
    };

    let peer_cert_for_dialer = outgoing.certificate().as_ref().to_vec();
    let peer_cert_for_listener = incoming.certificate().as_ref().to_vec();

    let (dialer_handle, dialer_events, dialer_session_driver) = Session::split(outgoing);
    let (listener_handle, listener_events, listener_session_driver) = Session::split(incoming);
    tokio::spawn(dialer_session_driver.run());
    tokio::spawn(listener_session_driver.run());

    // Send nonces across.
    dialer_handle
        .send_control(ControlMessage::PairNonce {
            nonce: dialer_offer.nonce,
        })
        .expect("queued");
    listener_handle
        .send_control(ControlMessage::PairNonce {
            nonce: listener_offer.nonce,
        })
        .expect("queued");

    let mut dialer_side = Side {
        session: PairingSession::new(dialer_offer),
        handle: dialer_handle,
        events: dialer_events,
    };
    let mut listener_side = Side {
        session: PairingSession::new(listener_offer),
        handle: listener_handle,
        events: listener_events,
    };

    // Each side receives the other's nonce and pairs it with the certificate it
    // authenticated during the handshake.
    let ControlMessage::PairNonce { nonce } = next_control(&mut dialer_side.events).await else {
        panic!("expected a nonce");
    };
    dialer_side
        .session
        .accept_peer(PairingOffer {
            certificate: peer_cert_for_dialer,
            nonce,
            name: name(listener_name),
        })
        .expect("accepted");

    let ControlMessage::PairNonce { nonce } = next_control(&mut listener_side.events).await else {
        panic!("expected a nonce");
    };
    listener_side
        .session
        .accept_peer(PairingOffer {
            certificate: peer_cert_for_listener,
            nonce,
            name: name(dialer_name),
        })
        .expect("accepted");

    (dialer_side, listener_side)
}

#[tokio::test]
async fn two_strangers_see_the_same_code_and_end_up_paired() {
    let a = Arc::new(Identity::generate().expect("generates"));
    let b = Arc::new(Identity::generate().expect("generates"));

    let (mut left, mut right) = exchange(&a, "Studio Mac", &b, "Windows PC").await;

    let left_code = left.session.code().expect("code ready");
    let right_code = right.session.code().expect("code ready");
    assert_eq!(
        left_code, right_code,
        "the two screens showed different codes for an honest connection"
    );
    assert_eq!(left_code.display().len(), 7);

    // Each side shows the other's name beside the code.
    assert_eq!(
        left.session.peer_name().expect("named").as_str(),
        "Windows PC"
    );
    assert_eq!(
        right.session.peer_name().expect("named").as_str(),
        "Studio Mac"
    );

    // The user confirms on both machines.
    left.session.confirm_local();
    left.handle
        .send_control(ControlMessage::PairConfirm)
        .expect("queued");
    right.session.confirm_local();
    right
        .handle
        .send_control(ControlMessage::PairConfirm)
        .expect("queued");

    assert!(matches!(
        next_control(&mut left.events).await,
        ControlMessage::PairConfirm
    ));
    left.session.confirm_remote();
    assert!(matches!(
        next_control(&mut right.events).await,
        ControlMessage::PairConfirm
    ));
    right.session.confirm_remote();

    assert_eq!(left.session.state(), PairingState::Confirmed);
    assert_eq!(right.session.state(), PairingState::Confirmed);

    // Now each can pin the other, and a normal connection succeeds where it
    // would previously have been refused.
    let mut left_trust = TrustStore::new();
    left_trust.trust(
        left.session.confirmed_name().expect("named"),
        &left.session.confirmed_certificate().expect("confirmed"),
        1,
    );
    let mut right_trust = TrustStore::new();
    right_trust.trust(
        right.session.confirmed_name().expect("named"),
        &right.session.confirmed_certificate().expect("confirmed"),
        1,
    );

    let listener = Endpoint::bind(
        SocketAddr::from((Ipv4Addr::LOCALHOST, 0)),
        Arc::clone(&b),
        name("Windows PC"),
        Arc::new(RwLock::new(right_trust)),
    )
    .expect("binds");
    let dialer = Endpoint::bind(
        SocketAddr::from((Ipv4Addr::LOCALHOST, 0)),
        Arc::clone(&a),
        name("Studio Mac"),
        Arc::new(RwLock::new(left_trust)),
    )
    .expect("binds");

    let addr = listener.local_addr().expect("bound");
    // The accepted connection must be held. Dropping it closes the QUIC
    // connection immediately, which can discard the `HelloAck` still buffered on
    // the stream and make the dialer see a transport failure instead.
    let accepting = tokio::spawn(async move { listener.accept().await });
    let connected = dialer.connect(addr).await;
    assert!(
        connected.is_ok(),
        "a freshly paired device could not connect: {:?}",
        connected.err()
    );
    let accepted = accepting.await.expect("task");
    assert!(
        accepted.is_some_and(|r| r.is_ok()),
        "the listener did not accept the freshly paired device"
    );
}

#[tokio::test]
async fn an_interposed_attacker_cannot_make_both_screens_agree() {
    // The attack the whole construction exists to stop. The attacker completes a
    // genuine pairing with each side separately, presenting its own certificate
    // to both — which is exactly what a machine-in-the-middle does.
    let a = Arc::new(Identity::generate().expect("generates"));
    let b = Arc::new(Identity::generate().expect("generates"));
    let attacker = Arc::new(Identity::generate().expect("generates"));

    let (victim_a, _) = exchange(&a, "Studio Mac", &attacker, "Windows PC").await;
    let (_, victim_b) = exchange(&attacker, "Studio Mac", &b, "Windows PC").await;

    let seen_by_a = victim_a.session.code().expect("code ready");
    let seen_by_b = victim_b.session.code().expect("code ready");

    assert_ne!(
        seen_by_a, seen_by_b,
        "an interposed attacker produced matching codes on both screens"
    );
}

#[tokio::test]
async fn a_rejected_pairing_yields_nothing_to_trust() {
    let a = Arc::new(Identity::generate().expect("generates"));
    let b = Arc::new(Identity::generate().expect("generates"));
    let (mut left, _right) = exchange(&a, "Studio Mac", &b, "Windows PC").await;

    assert!(left.session.code().is_some());
    left.session.reject();

    assert_eq!(left.session.state(), PairingState::Rejected);
    assert!(
        left.session.confirmed_certificate().is_none(),
        "a rejected pairing produced a certificate to trust"
    );
}

#[tokio::test]
async fn one_sided_confirmation_is_not_enough() {
    // If confirming on one machine sufficed, an attacker controlling the other
    // machine's display could simply claim the codes matched.
    let a = Arc::new(Identity::generate().expect("generates"));
    let b = Arc::new(Identity::generate().expect("generates"));
    let (mut left, _right) = exchange(&a, "Studio Mac", &b, "Windows PC").await;

    left.session.confirm_local();
    assert!(left.session.confirmed_certificate().is_none());
    assert_ne!(left.session.state(), PairingState::Confirmed);
}
