//! Pairing two devices with a short verification code.
//!
//! # The problem
//!
//! Certificate pinning answers "is this the device I paired with". It cannot
//! answer "is the device I am pairing with the one in front of me" — the very
//! first connection has nothing to compare against. An attacker positioned
//! between the two machines can complete that first handshake with *both* sides,
//! presenting its own certificate to each, and thereafter be permanently trusted
//! by both. Trust on first use, alone, is trust in whoever gets there first.
//!
//! # The construction
//!
//! Both machines derive a six-digit code from the certificates they can each
//! see, and the user checks that the two screens agree.
//!
//! ```text
//!        A ──────────────── B          A ───── M ───── B
//!    sees cert_A, cert_B            sees cert_A, cert_M
//!    code: 482 193                  code: 482 193
//!                                          M sees cert_M, cert_B
//!                                          code: 730 016  ← differs
//! ```
//!
//! An interposed attacker necessarily presents a different certificate on each
//! side, so the two machines derive different codes and the mismatch is visible.
//! To defeat it, an attacker would have to find key material producing a
//! matching six-digit code *and* have the user confirm it on both machines.
//!
//! This is the same construction as Bluetooth numeric comparison and Signal's
//! safety numbers.
//!
//! # Why both sides must confirm
//!
//! Confirming on one machine only would let an attacker who controls the *other*
//! machine's display simply claim the codes matched. The state machine here
//! refuses to complete until both have confirmed independently — see
//! [`PairingState`].
//!
//! # What this does not defend against
//!
//! A user who confirms without looking. Nothing can fix that, which is why the
//! code is short enough to compare at a glance and why the prompt shows the
//! device name alongside it.

use crate::error::SecurityError;
use mb_types::{DeviceName, Redacted};
use rustls_pki_types::CertificateDer;
use sha2::{Digest as _, Sha256};

/// Domain separator, so this hash cannot collide with any other use of SHA-256
/// in the protocol.
const DOMAIN: &[u8] = b"mousebridge/pairing/v1";

/// Length of the random contribution each side makes.
pub const NONCE_LEN: usize = 32;

/// Number of digits in the verification code.
///
/// Six is the same length as a Bluetooth pairing code or a two-factor token —
/// short enough to compare across two screens at a glance, which is the only way
/// it gets compared honestly. Longer codes get skimmed.
pub const CODE_DIGITS: u32 = 6;

/// One device's contribution to the exchange.
///
/// The nonce ensures a code is never reused between pairing attempts, so an
/// observer who saw a previous code learns nothing about this one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PairingOffer {
    /// The certificate this device will be pinned under.
    pub certificate: Vec<u8>,
    /// Fresh random bytes.
    pub nonce: [u8; NONCE_LEN],
    /// Display name, shown alongside the code.
    pub name: DeviceName,
}

/// A six-digit verification code.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VerificationCode(u32);

impl VerificationCode {
    /// The numeric value, always below one million.
    #[must_use]
    pub const fn value(self) -> u32 {
        self.0
    }

    /// The code as six digits, grouped in threes: `482 193`.
    ///
    /// Grouped because a run of six digits is measurably harder to compare
    /// across two screens than two groups of three.
    #[must_use]
    pub fn display(self) -> String {
        let text = format!("{:06}", self.0);
        format!("{} {}", &text[..3], &text[3..])
    }
}

/// Derives the verification code both machines must show.
///
/// The inputs are ordered by certificate so that both sides compute the same
/// value regardless of which initiated the connection. Without that, the two
/// machines would disagree on every pairing and the feature would never work.
#[must_use]
pub fn verification_code(a: &PairingOffer, b: &PairingOffer) -> VerificationCode {
    let (first, second) = if a.certificate <= b.certificate {
        (a, b)
    } else {
        (b, a)
    };

    let mut hasher = Sha256::new();
    hasher.update(DOMAIN);
    // Lengths are hashed alongside the values so that concatenation is
    // unambiguous: without them, an attacker could shift bytes between adjacent
    // fields and produce the same digest from different inputs.
    hasher.update((first.certificate.len() as u64).to_le_bytes());
    hasher.update(&first.certificate);
    hasher.update(first.nonce);
    hasher.update((second.certificate.len() as u64).to_le_bytes());
    hasher.update(&second.certificate);
    hasher.update(second.nonce);

    let digest = hasher.finalize();
    let raw = u32::from_le_bytes([digest[0], digest[1], digest[2], digest[3]]);
    VerificationCode(raw % 10u32.pow(CODE_DIGITS))
}

/// Generates a fresh nonce.
///
/// # Errors
///
/// Returns [`SecurityError::Generation`] if the OS random source is unavailable.
pub fn generate_nonce() -> Result<Redacted<[u8; NONCE_LEN]>, SecurityError> {
    let mut nonce = [0u8; NONCE_LEN];
    getrandom::fill(&mut nonce).map_err(|_| SecurityError::Generation {
        what: "pairing nonce",
        detail: "system entropy source unavailable".to_owned(),
    })?;
    Ok(Redacted::new(nonce))
}

/// How far along a pairing attempt is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PairingState {
    /// Waiting for the peer's offer.
    AwaitingPeer,
    /// Both offers are in; the code is showing on this machine.
    AwaitingConfirmation {
        /// Whether this machine's user has confirmed.
        local: bool,
        /// Whether the peer reported its user confirming.
        remote: bool,
    },
    /// Both users confirmed. The peer may now be trusted.
    Confirmed,
    /// Someone rejected, or the attempt was abandoned.
    Rejected,
}

/// Drives one pairing attempt.
///
/// Deliberately a state machine rather than a sequence of calls: the ordering
/// rules — a code before any confirmation, both confirmations before trust — are
/// the security property, and they should be impossible to get wrong from
/// outside.
#[derive(Debug)]
pub struct PairingSession {
    local: PairingOffer,
    peer: Option<PairingOffer>,
    state: PairingState,
}

impl PairingSession {
    /// Starts an attempt with this machine's offer.
    #[must_use]
    pub const fn new(local: PairingOffer) -> Self {
        Self {
            local,
            peer: None,
            state: PairingState::AwaitingPeer,
        }
    }

    /// This machine's offer, to send to the peer.
    #[must_use]
    pub const fn offer(&self) -> &PairingOffer {
        &self.local
    }

    /// Current state.
    #[must_use]
    pub const fn state(&self) -> PairingState {
        self.state
    }

    /// Records the peer's offer and computes the code.
    ///
    /// # Errors
    ///
    /// [`SecurityError::UntrustedDevice`] if the peer offered the same
    /// certificate as this machine. That means the connection is looped back to
    /// ourselves, or something is replaying our own offer at us; either way the
    /// code would trivially match and the check would prove nothing.
    pub fn accept_peer(&mut self, peer: PairingOffer) -> Result<VerificationCode, SecurityError> {
        if peer.certificate == self.local.certificate {
            self.state = PairingState::Rejected;
            return Err(SecurityError::UntrustedDevice {
                fingerprint: "peer presented this device's own certificate".to_owned(),
            });
        }

        let code = verification_code(&self.local, &peer);
        self.peer = Some(peer);
        self.state = PairingState::AwaitingConfirmation {
            local: false,
            remote: false,
        };
        Ok(code)
    }

    /// The code, once both offers are in.
    #[must_use]
    pub fn code(&self) -> Option<VerificationCode> {
        self.peer
            .as_ref()
            .map(|peer| verification_code(&self.local, peer))
    }

    /// The peer's display name, for the prompt.
    #[must_use]
    pub fn peer_name(&self) -> Option<&DeviceName> {
        self.peer.as_ref().map(|p| &p.name)
    }

    /// Records this machine's user confirming the code matches.
    pub fn confirm_local(&mut self) {
        if let PairingState::AwaitingConfirmation { remote, .. } = self.state {
            self.state = PairingState::AwaitingConfirmation {
                local: true,
                remote,
            };
            self.settle();
        }
    }

    /// Records the peer reporting that its user confirmed.
    pub fn confirm_remote(&mut self) {
        if let PairingState::AwaitingConfirmation { local, .. } = self.state {
            self.state = PairingState::AwaitingConfirmation {
                local,
                remote: true,
            };
            self.settle();
        }
    }

    /// Abandons the attempt.
    ///
    /// Terminal: a rejected attempt can never be revived, so a stray later
    /// confirmation cannot resurrect it.
    pub const fn reject(&mut self) {
        self.state = PairingState::Rejected;
    }

    const fn settle(&mut self) {
        if let PairingState::AwaitingConfirmation {
            local: true,
            remote: true,
        } = self.state
        {
            self.state = PairingState::Confirmed;
        }
    }

    /// The peer's certificate, once pairing is confirmed.
    ///
    /// Returns `None` in any other state. This is the gate: nothing can be
    /// written to the trust store until both users have agreed the codes matched.
    #[must_use]
    pub fn confirmed_certificate(&self) -> Option<CertificateDer<'static>> {
        if self.state != PairingState::Confirmed {
            return None;
        }
        self.peer
            .as_ref()
            .map(|peer| CertificateDer::from(peer.certificate.clone()))
    }

    /// The peer's name, once pairing is confirmed.
    #[must_use]
    pub fn confirmed_name(&self) -> Option<DeviceName> {
        (self.state == PairingState::Confirmed)
            .then(|| self.peer.as_ref().map(|p| p.name.clone()))
            .flatten()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::identity::Identity;

    fn offer(identity: &Identity, nonce: u8, name: &str) -> PairingOffer {
        PairingOffer {
            certificate: identity.certificate().as_ref().to_vec(),
            nonce: [nonce; NONCE_LEN],
            name: DeviceName::new(name).expect("valid"),
        }
    }

    #[test]
    fn both_sides_derive_the_same_code() {
        // The whole feature depends on this. If the two machines disagreed, every
        // pairing would look like an attack.
        let a = Identity::generate().expect("generates");
        let b = Identity::generate().expect("generates");
        let offer_a = offer(&a, 1, "Mac");
        let offer_b = offer(&b, 2, "PC");

        assert_eq!(
            verification_code(&offer_a, &offer_b),
            verification_code(&offer_b, &offer_a),
            "the code depended on who initiated"
        );
    }

    #[test]
    fn an_interposed_attacker_produces_a_different_code() {
        // The attack this defends against: someone between the two machines,
        // presenting their own certificate to each side.
        let a = Identity::generate().expect("generates");
        let b = Identity::generate().expect("generates");
        let attacker = Identity::generate().expect("generates");

        let honest = verification_code(&offer(&a, 1, "Mac"), &offer(&b, 2, "PC"));
        let what_a_sees = verification_code(&offer(&a, 1, "Mac"), &offer(&attacker, 2, "PC"));
        let what_b_sees = verification_code(&offer(&attacker, 1, "Mac"), &offer(&b, 2, "PC"));

        assert_ne!(what_a_sees, what_b_sees, "the two screens would agree");
        assert_ne!(what_a_sees, honest);
        assert_ne!(what_b_sees, honest);
    }

    #[test]
    fn a_fresh_nonce_changes_the_code() {
        // So an observer who saw a previous code learns nothing about this one.
        let a = Identity::generate().expect("generates");
        let b = Identity::generate().expect("generates");

        let first = verification_code(&offer(&a, 1, "Mac"), &offer(&b, 2, "PC"));
        let second = verification_code(&offer(&a, 9, "Mac"), &offer(&b, 8, "PC"));
        assert_ne!(first, second);
    }

    #[test]
    fn the_name_does_not_affect_the_code() {
        // Names are cosmetic and user-editable. Binding the code to one would
        // mean a rename broke pairing for no security benefit.
        let a = Identity::generate().expect("generates");
        let b = Identity::generate().expect("generates");
        assert_eq!(
            verification_code(&offer(&a, 1, "Mac"), &offer(&b, 2, "PC")),
            verification_code(&offer(&a, 1, "Renamed"), &offer(&b, 2, "Also renamed")),
        );
    }

    #[test]
    fn codes_are_six_digits_and_readable() {
        let a = Identity::generate().expect("generates");
        let b = Identity::generate().expect("generates");
        let code = verification_code(&offer(&a, 1, "Mac"), &offer(&b, 2, "PC"));

        assert!(code.value() < 1_000_000);
        let shown = code.display();
        assert_eq!(shown.len(), 7, "expected `NNN NNN`, got {shown}");
        assert_eq!(shown.chars().filter(char::is_ascii_digit).count(), 6);
    }

    #[test]
    fn a_low_code_is_padded_not_truncated() {
        // A code of 42 must read `000 042`, or the two screens would show
        // different-length strings and look like a mismatch.
        assert_eq!(VerificationCode(42).display(), "000 042");
        assert_eq!(VerificationCode(0).display(), "000 000");
        assert_eq!(VerificationCode(999_999).display(), "999 999");
    }

    #[test]
    fn nonces_differ_between_calls() {
        let first = generate_nonce().expect("entropy");
        let second = generate_nonce().expect("entropy");
        assert_ne!(first.expose(), second.expose());
    }

    #[test]
    fn a_nonce_never_appears_in_debug_output() {
        let nonce = generate_nonce().expect("entropy");
        assert_eq!(format!("{nonce:?}"), "<redacted 32 bytes>");
    }

    #[test]
    fn pairing_completes_only_when_both_sides_confirm() {
        let a = Identity::generate().expect("generates");
        let b = Identity::generate().expect("generates");

        let mut session = PairingSession::new(offer(&a, 1, "Mac"));
        assert_eq!(session.state(), PairingState::AwaitingPeer);
        assert!(session.code().is_none());

        session.accept_peer(offer(&b, 2, "PC")).expect("accepted");
        assert!(session.code().is_some());
        assert!(
            session.confirmed_certificate().is_none(),
            "trust was available before any confirmation"
        );

        session.confirm_local();
        assert!(
            session.confirmed_certificate().is_none(),
            "one confirmation was enough"
        );

        session.confirm_remote();
        assert_eq!(session.state(), PairingState::Confirmed);
        assert!(session.confirmed_certificate().is_some());
        assert_eq!(session.confirmed_name().expect("named").as_str(), "PC");
    }

    #[test]
    fn confirmation_order_does_not_matter() {
        let a = Identity::generate().expect("generates");
        let b = Identity::generate().expect("generates");

        let mut session = PairingSession::new(offer(&a, 1, "Mac"));
        session.accept_peer(offer(&b, 2, "PC")).expect("accepted");
        session.confirm_remote();
        assert!(session.confirmed_certificate().is_none());
        session.confirm_local();
        assert_eq!(session.state(), PairingState::Confirmed);
    }

    #[test]
    fn a_rejection_is_terminal() {
        // A stray later confirmation must not resurrect a rejected attempt.
        let a = Identity::generate().expect("generates");
        let b = Identity::generate().expect("generates");

        let mut session = PairingSession::new(offer(&a, 1, "Mac"));
        session.accept_peer(offer(&b, 2, "PC")).expect("accepted");
        session.reject();

        session.confirm_local();
        session.confirm_remote();
        assert_eq!(session.state(), PairingState::Rejected);
        assert!(session.confirmed_certificate().is_none());
    }

    #[test]
    fn confirming_before_the_peer_offers_does_nothing() {
        let a = Identity::generate().expect("generates");
        let mut session = PairingSession::new(offer(&a, 1, "Mac"));

        session.confirm_local();
        session.confirm_remote();
        assert_eq!(session.state(), PairingState::AwaitingPeer);
        assert!(session.confirmed_certificate().is_none());
    }

    #[test]
    fn a_peer_offering_our_own_certificate_is_refused() {
        // A loopback, or something replaying our offer back at us. The codes
        // would trivially match and the comparison would prove nothing.
        let a = Identity::generate().expect("generates");
        let mut session = PairingSession::new(offer(&a, 1, "Mac"));

        let result = session.accept_peer(offer(&a, 2, "Impostor"));
        assert!(result.is_err());
        assert_eq!(session.state(), PairingState::Rejected);
    }

    #[test]
    fn a_confirmed_pairing_produces_the_certificate_the_peer_offered() {
        let a = Identity::generate().expect("generates");
        let b = Identity::generate().expect("generates");

        let mut session = PairingSession::new(offer(&a, 1, "Mac"));
        session.accept_peer(offer(&b, 2, "PC")).expect("accepted");
        session.confirm_local();
        session.confirm_remote();

        let certificate = session.confirmed_certificate().expect("confirmed");
        assert_eq!(certificate.as_ref(), b.certificate().as_ref());
        assert_eq!(crate::fingerprint(&certificate), b.device_id());
    }

    #[test]
    fn codes_are_spread_across_the_range() {
        // A derivation that clustered would shrink the effective code space and
        // weaken the comparison.
        let mut seen = std::collections::HashSet::new();
        for i in 0..200u8 {
            let a = Identity::generate().expect("generates");
            let b = Identity::generate().expect("generates");
            seen.insert(verification_code(&offer(&a, i, "A"), &offer(&b, i, "B")).value());
        }
        assert!(
            seen.len() > 190,
            "only {} distinct codes in 200",
            seen.len()
        );
    }
}
