//! mDNS-SD service advertisement and browsing.
//!
//! The pleasant discovery path: no port scanning, no broadcast, and it composes
//! with the `.local` name resolution both platforms already do.
//!
//! It is not the only path, because on real networks it frequently does not
//! work. Access-point client isolation, IGMP snooping and multicast-to-unicast
//! conversion all silently drop mDNS on ordinary consumer Wi-Fi, and enterprise
//! networks often filter it outright. When that happens the user sees "no
//! computers found" with nothing to act on — which is why [`crate::socket`] and
//! [`crate::manual`] exist alongside this.
//!
//! # What is advertised
//!
//! The same information as a UDP announcement, in TXT records. See
//! [`crate::beacon`] for what may and may not appear there — mDNS records are
//! visible to every machine on the network and are cached by other resolvers,
//! so the constraint is, if anything, stronger.

use crate::beacon::Announcement;
use mb_protocol::version::{Version, VersionRange};
use mb_types::{DeviceId, DeviceName, OsKind};
use std::net::{IpAddr, SocketAddr};
use tracing::{debug, warn};

/// Service type advertised and browsed for.
///
/// `_udp` because the transport is QUIC. Versioned in the same way as the ALPN
/// identifier, so a future incompatible transport can coexist rather than being
/// discovered and then failing to connect.
pub const SERVICE_TYPE: &str = "_mousebridge._udp.local.";

/// TXT record keys.
mod keys {
    /// Device identity fingerprint, hex-encoded.
    pub const DEVICE: &str = "id";
    /// Display name.
    pub const NAME: &str = "name";
    /// Operating system family.
    pub const OS: &str = "os";
    /// Lowest accepted protocol version.
    pub const VERSION_MIN: &str = "vmin";
    /// Highest accepted protocol version.
    pub const VERSION_MAX: &str = "vmax";
}

/// Errors from the mDNS responder.
#[derive(Debug, thiserror::Error)]
pub enum MdnsError {
    /// The mDNS daemon could not be started.
    ///
    /// Usually means multicast is unavailable on every interface, which is
    /// common enough that it must degrade to the other mechanisms rather than
    /// failing startup.
    #[error("mDNS is unavailable on this network: {detail}")]
    Unavailable {
        /// Underlying cause.
        detail: String,
    },
    /// The service could not be registered.
    #[error("could not advertise over mDNS: {detail}")]
    Register {
        /// Underlying cause.
        detail: String,
    },
    /// Browsing could not be started.
    #[error("could not browse for peers over mDNS: {detail}")]
    Browse {
        /// Underlying cause.
        detail: String,
    },
}

/// Builds the TXT properties advertised for this device.
#[must_use]
pub fn txt_properties(announcement: &Announcement) -> Vec<(String, String)> {
    vec![
        (keys::DEVICE.to_owned(), announcement.device.to_hex()),
        (keys::NAME.to_owned(), announcement.name.to_string()),
        (keys::OS.to_owned(), os_token(announcement.os).to_owned()),
        (
            keys::VERSION_MIN.to_owned(),
            announcement.versions.min.0.to_string(),
        ),
        (
            keys::VERSION_MAX.to_owned(),
            announcement.versions.max.0.to_string(),
        ),
    ]
}

/// Reconstructs an announcement from TXT records and a resolved address.
///
/// Returns `None` when a required record is missing or malformed. These records
/// are unauthenticated and can be published by anyone on the network, so every
/// field is parsed defensively and a failure simply means the service is ignored.
#[must_use]
pub fn announcement_from_txt(properties: &[(String, String)], port: u16) -> Option<Announcement> {
    let get = |key: &str| {
        properties
            .iter()
            .find(|(k, _)| k == key)
            .map(|(_, v)| v.as_str())
    };

    let device = DeviceId::parse_hex(get(keys::DEVICE)?).ok()?;
    let name = DeviceName::new(get(keys::NAME)?).ok()?;
    let os = os_from_token(get(keys::OS)?);
    let min = Version(get(keys::VERSION_MIN)?.parse().ok()?);
    let max = Version(get(keys::VERSION_MAX)?.parse().ok()?);
    let versions = VersionRange::new(min, max).ok()?;

    if port == 0 {
        return None;
    }

    Some(Announcement {
        device,
        name,
        os,
        port,
        versions,
    })
}

const fn os_token(os: OsKind) -> &'static str {
    match os {
        OsKind::MacOs => "macos",
        OsKind::Windows => "windows",
        OsKind::Unknown => "unknown",
    }
}

fn os_from_token(token: &str) -> OsKind {
    match token {
        "macos" => OsKind::MacOs,
        "windows" => OsKind::Windows,
        _ => OsKind::Unknown,
    }
}

/// An mDNS responder advertising this device and browsing for others.
pub struct MdnsResponder {
    daemon: mdns_sd::ServiceDaemon,
    fullname: Option<String>,
}

impl MdnsResponder {
    /// Starts the mDNS daemon.
    ///
    /// # Errors
    ///
    /// [`MdnsError::Unavailable`] if multicast is not usable. Callers must treat
    /// this as a degraded mode rather than a fatal error: broadcast and manual
    /// addressing still work, and failing startup here would make the whole
    /// application unusable on networks where mDNS is merely filtered.
    pub fn start() -> Result<Self, MdnsError> {
        let daemon = mdns_sd::ServiceDaemon::new().map_err(|e| MdnsError::Unavailable {
            detail: e.to_string(),
        })?;
        Ok(Self {
            daemon,
            fullname: None,
        })
    }

    /// Advertises this device.
    ///
    /// # Errors
    ///
    /// [`MdnsError::Register`] if the service could not be published.
    pub fn advertise(
        &mut self,
        announcement: &Announcement,
        addresses: &[IpAddr],
    ) -> Result<(), MdnsError> {
        // The instance name must be unique on the network. The device
        // fingerprint's short form is both unique and stable, where a display
        // name is neither — two machines called "MacBook Pro" is the normal case,
        // not the exception.
        let instance = format!("mousebridge-{}", announcement.device.short());
        let host = format!("{}.local.", instance);

        let properties = txt_properties(announcement);
        let service = mdns_sd::ServiceInfo::new(
            SERVICE_TYPE,
            &instance,
            &host,
            addresses,
            announcement.port,
            &properties[..],
        )
        .map_err(|e| MdnsError::Register {
            detail: e.to_string(),
        })?;

        let fullname = service.get_fullname().to_owned();
        self.daemon
            .register(service)
            .map_err(|e| MdnsError::Register {
                detail: e.to_string(),
            })?;

        debug!(%fullname, "advertising over mDNS");
        self.fullname = Some(fullname);
        Ok(())
    }

    /// Starts browsing, returning a channel of discovered peers.
    ///
    /// # Errors
    ///
    /// [`MdnsError::Browse`] if browsing could not be started.
    pub fn browse(&self) -> Result<mdns_sd::Receiver<mdns_sd::ServiceEvent>, MdnsError> {
        self.daemon
            .browse(SERVICE_TYPE)
            .map_err(|e| MdnsError::Browse {
                detail: e.to_string(),
            })
    }

    /// Converts a resolved service into an announcement and address.
    ///
    /// Returns `None` for services that are missing records or advertise no
    /// usable address — including our own, which the caller identifies by
    /// comparing the device fingerprint.
    #[must_use]
    pub fn resolve(service: &mdns_sd::ResolvedService) -> Option<(Announcement, SocketAddr)> {
        let properties: Vec<(String, String)> = service
            .txt_properties
            .iter()
            .map(|p| (p.key().to_owned(), p.val_str().to_owned()))
            .collect();

        let announcement = announcement_from_txt(&properties, service.port)?;

        // Prefer IPv4: the transport binds an IPv4 socket, and a link-local IPv6
        // address without its scope would fail to connect in a way that looks
        // like the peer being offline.
        let address = service
            .addresses
            .iter()
            .map(|scoped| scoped.to_ip_addr())
            .find(IpAddr::is_ipv4)
            .or_else(|| service.addresses.iter().map(|s| s.to_ip_addr()).next())?;

        Some((announcement, SocketAddr::new(address, service.port)))
    }

    /// Withdraws the advertisement and stops the daemon.
    pub fn shutdown(&mut self) {
        if let Some(fullname) = self.fullname.take() {
            // Withdrawing explicitly means peers drop us immediately instead of
            // waiting for the record to expire, which otherwise leaves a ghost
            // entry in every device list for minutes after quitting.
            if let Err(e) = self.daemon.unregister(&fullname) {
                warn!(error = %e, "could not withdraw the mDNS advertisement");
            }
        }
        if let Err(e) = self.daemon.shutdown() {
            warn!(error = %e, "could not stop the mDNS daemon");
        }
    }
}

impl Drop for MdnsResponder {
    fn drop(&mut self) {
        self.shutdown();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn announcement() -> Announcement {
        Announcement {
            device: DeviceId::from_bytes([0xAB; 32]),
            name: DeviceName::new("Studio Mac").expect("valid"),
            os: OsKind::MacOs,
            port: 25470,
            versions: VersionRange::current(),
        }
    }

    #[test]
    fn txt_records_round_trip() {
        let original = announcement();
        let properties = txt_properties(&original);
        let back = announcement_from_txt(&properties, original.port).expect("parses");
        assert_eq!(back, original);
    }

    #[test]
    fn every_os_token_round_trips() {
        for os in [OsKind::MacOs, OsKind::Windows, OsKind::Unknown] {
            assert_eq!(os_from_token(os_token(os)), os);
        }
    }

    #[test]
    fn an_unknown_os_token_degrades_rather_than_failing() {
        // A future version advertising an OS this build has never heard of must
        // still appear in the list.
        assert_eq!(os_from_token("haiku"), OsKind::Unknown);
    }

    #[test]
    fn missing_records_are_rejected() {
        // These are published by anyone on the network and are unauthenticated.
        let full = txt_properties(&announcement());
        for skip in 0..full.len() {
            let partial: Vec<(String, String)> = full
                .iter()
                .enumerate()
                .filter(|(i, _)| *i != skip)
                .map(|(_, p)| p.clone())
                .collect();
            assert!(
                announcement_from_txt(&partial, 25470).is_none(),
                "accepted a record set missing entry {skip}"
            );
        }
    }

    #[test]
    fn malformed_records_are_rejected() {
        let bad = |key: &str, value: &str| {
            let mut properties = txt_properties(&announcement());
            for entry in &mut properties {
                if entry.0 == key {
                    entry.1 = value.to_owned();
                }
            }
            announcement_from_txt(&properties, 25470)
        };

        assert!(bad(keys::DEVICE, "not-hex").is_none());
        assert!(bad(keys::DEVICE, "abcd").is_none(), "short id accepted");
        assert!(bad(keys::NAME, "").is_none());
        assert!(bad(keys::NAME, "line\nbreak").is_none());
        assert!(bad(keys::VERSION_MIN, "x").is_none());
        assert!(bad(keys::VERSION_MAX, "99999999").is_none());
    }

    #[test]
    fn an_inverted_version_range_is_rejected() {
        let mut properties = txt_properties(&announcement());
        for entry in &mut properties {
            if entry.0 == keys::VERSION_MIN {
                entry.1 = "50".to_owned();
            }
        }
        assert!(announcement_from_txt(&properties, 25470).is_none());
    }

    #[test]
    fn a_zero_port_is_rejected() {
        let properties = txt_properties(&announcement());
        assert!(announcement_from_txt(&properties, 0).is_none());
    }

    #[test]
    fn the_service_type_is_well_formed() {
        // A malformed type is accepted by the API and then silently matches
        // nothing, which looks exactly like an empty network.
        assert!(SERVICE_TYPE.starts_with('_'));
        assert!(SERVICE_TYPE.ends_with(".local."));
        assert!(SERVICE_TYPE.contains("._udp."));
    }
}
