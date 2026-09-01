//! Security errors.

/// A failure in identity handling or trust evaluation.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum SecurityError {
    /// A key or certificate could not be generated.
    #[error("could not generate {what}: {detail}")]
    Generation {
        /// What was being generated.
        what: &'static str,
        /// Underlying cause.
        detail: String,
    },

    /// Stored key material could not be loaded.
    #[error("stored identity is unusable: {detail}")]
    CorruptIdentity {
        /// Underlying cause.
        detail: String,
    },

    /// The peer presented a certificate for a device we do not trust.
    #[error("device {fingerprint} is not paired with this computer")]
    UntrustedDevice {
        /// Short fingerprint of the offending certificate.
        fingerprint: String,
    },

    /// The peer presented a *different* certificate than the one recorded.
    ///
    /// The most serious error this crate produces. A known device presenting new
    /// key material is either a reinstall or an impersonation attempt, and the
    /// two are indistinguishable from here — so it is a hard failure that the
    /// user must resolve, never a silent re-pair.
    #[error(
        "device {device} presented a different certificate than the one recorded; \
         it may have been reinstalled, or something may be impersonating it"
    )]
    CertificateChanged {
        /// The device claiming the identity.
        device: String,
    },

    /// The trust store could not be read or written.
    #[error("trust store i/o failed at {path}: {detail}")]
    Io {
        /// Path involved.
        path: String,
        /// Underlying cause.
        detail: String,
    },

    /// The trust store file was malformed.
    #[error("trust store is malformed: {detail}")]
    CorruptTrustStore {
        /// Underlying cause.
        detail: String,
    },
}
