//! Parsing manually entered peer addresses.
//!
//! The escape hatch that has to work when everything else is filtered. Users type
//! these by hand, so the parser accepts the forms people actually write and
//! rejects the rest with a reason they can act on.

use std::net::{IpAddr, SocketAddr};

/// Why a manually entered address was rejected.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum AddressError {
    /// The field was empty.
    #[error("enter an address, for example 192.168.1.42")]
    Empty,
    /// The port was not a number, or was zero.
    #[error("'{found}' is not a valid port number")]
    BadPort {
        /// What was typed.
        found: String,
    },
    /// The host part was not an IP address.
    ///
    /// Host *names* are deliberately not resolved here: resolution is a blocking
    /// network operation, and doing it inside a parser would stall whichever
    /// thread happened to call it.
    #[error("'{found}' is not an IP address")]
    BadHost {
        /// What was typed.
        found: String,
    },
}

/// Parses `host`, `host:port`, or `[v6]:port`.
///
/// Falls back to `default_port` when none is given, which is what most users
/// will type.
///
/// # Errors
///
/// See [`AddressError`]; each variant carries text intended for the user.
pub fn parse_peer_address(input: &str, default_port: u16) -> Result<SocketAddr, AddressError> {
    let text = input.trim();
    if text.is_empty() {
        return Err(AddressError::Empty);
    }

    // A bracketed IPv6 literal, with or without a port.
    if let Some(rest) = text.strip_prefix('[') {
        let (host, tail) = rest.split_once(']').ok_or_else(|| AddressError::BadHost {
            found: text.to_owned(),
        })?;
        let ip: IpAddr = host.parse().map_err(|_| AddressError::BadHost {
            found: host.to_owned(),
        })?;
        let port = match tail.strip_prefix(':') {
            Some(p) => parse_port(p)?,
            None if tail.is_empty() => default_port,
            None => {
                return Err(AddressError::BadHost {
                    found: text.to_owned(),
                });
            }
        };
        return Ok(SocketAddr::new(ip, port));
    }

    // A bare IPv6 literal has several colons, so a single trailing colon is the
    // only unambiguous "host:port" form for the unbracketed case.
    if text.matches(':').count() == 1
        && let Some((host, port)) = text.split_once(':')
    {
        let ip: IpAddr = host.parse().map_err(|_| AddressError::BadHost {
            found: host.to_owned(),
        })?;
        return Ok(SocketAddr::new(ip, parse_port(port)?));
    }

    let ip: IpAddr = text.parse().map_err(|_| AddressError::BadHost {
        found: text.to_owned(),
    })?;
    Ok(SocketAddr::new(ip, default_port))
}

fn parse_port(text: &str) -> Result<u16, AddressError> {
    match text.parse::<u16>() {
        // Port 0 asks the OS to choose, which is meaningless as a destination.
        Ok(0) | Err(_) => Err(AddressError::BadPort {
            found: text.to_owned(),
        }),
        Ok(port) => Ok(port),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{Ipv4Addr, Ipv6Addr};

    const DEFAULT: u16 = 25470;

    #[test]
    fn a_bare_ipv4_address_uses_the_default_port() {
        assert_eq!(
            parse_peer_address("192.168.1.42", DEFAULT),
            Ok(SocketAddr::from((Ipv4Addr::new(192, 168, 1, 42), DEFAULT)))
        );
    }

    #[test]
    fn an_explicit_port_wins() {
        assert_eq!(
            parse_peer_address("192.168.1.42:9000", DEFAULT),
            Ok(SocketAddr::from((Ipv4Addr::new(192, 168, 1, 42), 9000)))
        );
    }

    #[test]
    fn surrounding_whitespace_is_forgiven() {
        // Users paste addresses; a trailing space should not be an error.
        assert_eq!(
            parse_peer_address("  10.0.0.1  ", DEFAULT),
            Ok(SocketAddr::from((Ipv4Addr::new(10, 0, 0, 1), DEFAULT)))
        );
    }

    #[test]
    fn a_bare_ipv6_literal_is_accepted() {
        assert_eq!(
            parse_peer_address("fe80::1", DEFAULT),
            Ok(SocketAddr::from((
                "fe80::1".parse::<Ipv6Addr>().unwrap(),
                DEFAULT
            )))
        );
    }

    #[test]
    fn a_bracketed_ipv6_literal_takes_a_port() {
        assert_eq!(
            parse_peer_address("[fe80::1]:9000", DEFAULT),
            Ok(SocketAddr::from((
                "fe80::1".parse::<Ipv6Addr>().unwrap(),
                9000
            )))
        );
        assert_eq!(
            parse_peer_address("[::1]", DEFAULT),
            Ok(SocketAddr::from((Ipv6Addr::LOCALHOST, DEFAULT)))
        );
    }

    #[test]
    fn errors_name_what_the_user_typed() {
        // The message goes straight into a text field's error label.
        assert_eq!(parse_peer_address("   ", DEFAULT), Err(AddressError::Empty));
        assert_eq!(
            parse_peer_address("192.168.1.42:abc", DEFAULT),
            Err(AddressError::BadPort {
                found: "abc".to_owned()
            })
        );
        assert_eq!(
            parse_peer_address("not-an-address", DEFAULT),
            Err(AddressError::BadHost {
                found: "not-an-address".to_owned()
            })
        );
    }

    #[test]
    fn port_zero_is_rejected() {
        // Zero means "let the OS choose", which cannot be a destination.
        assert!(parse_peer_address("10.0.0.1:0", DEFAULT).is_err());
    }

    #[test]
    fn a_hostname_is_rejected_rather_than_resolved() {
        // Resolution is a blocking network call; doing it in a parser would
        // stall whichever thread happened to call it.
        assert!(parse_peer_address("my-pc.local", DEFAULT).is_err());
    }

    #[test]
    fn an_out_of_range_port_is_rejected() {
        assert!(parse_peer_address("10.0.0.1:70000", DEFAULT).is_err());
    }

    #[test]
    fn a_malformed_bracket_form_is_rejected() {
        assert!(parse_peer_address("[fe80::1", DEFAULT).is_err());
        assert!(parse_peer_address("[not-ipv6]:80", DEFAULT).is_err());
    }
}
