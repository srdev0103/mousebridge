//! A live mDNS round trip on this machine's real network interfaces.
//!
//! This advertises a service and then browses for it, which exercises the actual
//! multicast path rather than only the record encoding. It is the one part of
//! discovery that can be verified without a second computer.
//!
//! It depends on the machine's network, so a failure means one of two things and
//! the test distinguishes them: multicast being unavailable is reported and
//! skipped, because that is a legitimate network condition the product must
//! tolerate; a service that advertises but never resolves is a real failure.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use mb_discovery::beacon::Announcement;
use mb_discovery::mdns::MdnsResponder;
use mb_protocol::version::VersionRange;
use mb_types::{DeviceId, DeviceName, OsKind};
use std::net::{IpAddr, Ipv4Addr};
use std::time::{Duration, Instant};

#[test]
fn a_service_advertises_and_is_discovered_on_this_machine() {
    let device = DeviceId::from_bytes([0x5A; 32]);
    let announcement = Announcement {
        device,
        name: DeviceName::new("Loopback Test Device").expect("valid"),
        os: OsKind::current(),
        port: 25470,
        versions: VersionRange::current(),
    };

    let mut responder = match MdnsResponder::start() {
        Ok(responder) => responder,
        Err(e) => {
            // A legitimate network condition, and one the product must degrade
            // through rather than fail on. Broadcast and manual addressing still
            // work when this happens.
            eprintln!("SKIP: mDNS unavailable on this machine: {e}");
            return;
        }
    };

    let browse = responder.browse().expect("browsing starts");

    responder
        .advertise(&announcement, &[IpAddr::V4(Ipv4Addr::LOCALHOST)])
        .expect("advertises");

    // mDNS is not instant: the daemon publishes, then answers a query. Ten
    // seconds is generous; the interesting outcome is whether it ever resolves.
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut found = None;
    let mut saw_any_event = false;

    while Instant::now() < deadline {
        let remaining = deadline.saturating_duration_since(Instant::now());
        match browse.recv_timeout(remaining) {
            Ok(mdns_sd::ServiceEvent::ServiceResolved(service)) => {
                saw_any_event = true;
                if let Some((resolved, address)) = MdnsResponder::resolve(&service)
                    && resolved.device == device
                {
                    found = Some((resolved, address));
                    break;
                }
            }
            Ok(_) => saw_any_event = true,
            Err(_) => break,
        }
    }

    let Some((resolved, address)) = found else {
        if saw_any_event {
            panic!("mDNS produced events but never resolved our own service");
        }
        eprintln!("SKIP: no mDNS traffic observed; multicast is likely filtered here");
        return;
    };

    // The record survived a genuine multicast round trip.
    assert_eq!(resolved.device, device);
    assert_eq!(resolved.name.as_str(), "Loopback Test Device");
    assert_eq!(resolved.port, 25470);
    assert_eq!(address.port(), 25470);
    assert!(resolved.is_compatible());

    eprintln!("mDNS round trip verified: resolved {resolved:?} at {address}");
}
