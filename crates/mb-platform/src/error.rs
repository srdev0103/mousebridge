//! Platform error type.

/// A failure originating in an operating system API.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PlatformError {
    /// The operation is not implemented on this OS.
    #[error("{what} is not supported on this platform")]
    Unsupported {
        /// What was attempted.
        what: &'static str,
    },
    /// An OS call returned a failure code.
    #[error("{api} failed: {detail}")]
    OsCall {
        /// Name of the API that failed, for correlating with OS documentation.
        api: &'static str,
        /// Detail from the OS, such as an error code.
        detail: String,
    },
    /// The OS returned data that could not be interpreted.
    ///
    /// Kept distinct from [`PlatformError::OsCall`] because it usually indicates
    /// a wrong assumption on our side rather than a genuine OS failure.
    #[error("{api} returned unusable data: {detail}")]
    BadResponse {
        /// Name of the API involved.
        api: &'static str,
        /// What was wrong with the response.
        detail: String,
    },
}

impl PlatformError {
    /// Builds an [`PlatformError::OsCall`].
    #[must_use]
    pub fn os_call(api: &'static str, detail: impl Into<String>) -> Self {
        Self::OsCall {
            api,
            detail: detail.into(),
        }
    }

    /// Builds a [`PlatformError::BadResponse`].
    #[must_use]
    pub fn bad_response(api: &'static str, detail: impl Into<String>) -> Self {
        Self::BadResponse {
            api,
            detail: detail.into(),
        }
    }
}
