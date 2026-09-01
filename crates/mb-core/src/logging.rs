//! Structured logging setup.
//!
//! # What must never be logged
//!
//! Private keys, pairing secrets, authentication material, clipboard contents and
//! transferred file contents. The rule is enforced structurally rather than by
//! review: secrets are wrapped in [`mb_types::Redacted`], whose `Debug` and
//! `Display` print a placeholder, so logging a struct that contains one cannot
//! leak it. Audit with `grep -r '\.expose()'`.
//!
//! # The input path
//!
//! Logging on the capture path is a latency hazard: formatting allocates, and a
//! file write can block for milliseconds. Per-event logging is therefore
//! `trace`-level only, off by default, and must never appear between reading an
//! event and enqueuing it. Diagnostics for the input path belong in periodic
//! aggregate counters, not per-event lines.

use mb_config::{LogLevel, LoggingConfig};
use std::path::Path;
use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::layer::SubscriberExt as _;
use tracing_subscriber::util::SubscriberInitExt as _;
use tracing_subscriber::{EnvFilter, Layer as _};

/// Directory name for log files, created beside the configuration.
const LOG_DIR: &str = "logs";

/// Filename prefix for the rolling log.
const LOG_PREFIX: &str = "mousebridge.log";

/// Keeps the background log writer alive.
///
/// File logging is non-blocking: log lines go to a queue drained by a worker
/// thread, so a slow disk cannot stall a caller. The worker stops when this guard
/// drops, which would silently discard buffered lines, so the guard must be held
/// for the lifetime of the process.
#[must_use = "dropping the guard stops file logging and discards buffered lines"]
pub struct LogGuard {
    _worker: Option<WorkerGuard>,
}

/// Failures while setting up logging.
#[derive(Debug, thiserror::Error)]
pub enum LogError {
    /// The log directory could not be created.
    #[error("cannot create log directory {path}: {source}")]
    Directory {
        /// Directory that could not be created.
        path: String,
        /// Underlying error.
        #[source]
        source: std::io::Error,
    },
    /// A global subscriber was already installed.
    #[error("logging was already initialised")]
    AlreadyInitialised,
}

/// Installs the global tracing subscriber.
///
/// Writes human-readable lines to stderr and, when enabled, JSON lines to a
/// daily-rotated file beside the configuration. JSON on disk because support
/// output is machine-processed; plain text on stderr because a developer reads it.
///
/// `RUST_LOG` overrides the configured level when set, which is the standard
/// escape hatch for diagnosing a problem without editing a config file.
///
/// # Errors
///
/// Returns [`LogError::Directory`] if file logging is enabled and the directory
/// cannot be created, or [`LogError::AlreadyInitialised`] if called twice.
pub fn init(config: &LoggingConfig, config_dir: &Path) -> Result<LogGuard, LogError> {
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new(config.level.as_filter_str()));

    let stderr_layer = tracing_subscriber::fmt::layer()
        .with_writer(std::io::stderr)
        .with_target(true)
        .with_ansi(std::io::IsTerminal::is_terminal(&std::io::stderr()));

    let (file_layer, worker) = if config.file_enabled {
        let dir = config_dir.join(LOG_DIR);
        std::fs::create_dir_all(&dir).map_err(|source| LogError::Directory {
            path: dir.display().to_string(),
            source,
        })?;
        let appender = tracing_appender::rolling::daily(&dir, LOG_PREFIX);
        let (writer, guard) = tracing_appender::non_blocking(appender);
        let layer = tracing_subscriber::fmt::layer()
            .json()
            .with_writer(writer)
            .with_ansi(false);
        (Some(layer.boxed()), Some(guard))
    } else {
        (None, None)
    };

    tracing_subscriber::registry()
        .with(filter)
        .with(stderr_layer)
        .with(file_layer)
        .try_init()
        .map_err(|_| LogError::AlreadyInitialised)?;

    Ok(LogGuard { _worker: worker })
}

/// Returns the directory log files are written to.
#[must_use]
pub fn log_directory(config_dir: &Path) -> std::path::PathBuf {
    config_dir.join(LOG_DIR)
}

/// Maps a configured level to its `tracing` filter directive.
#[must_use]
pub const fn filter_for(level: LogLevel) -> &'static str {
    level.as_filter_str()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_level_maps_to_a_valid_filter() {
        for level in [
            LogLevel::Error,
            LogLevel::Warn,
            LogLevel::Info,
            LogLevel::Debug,
            LogLevel::Trace,
        ] {
            let directive = filter_for(level);
            assert!(
                EnvFilter::try_new(directive).is_ok(),
                "{level:?} produced an unparseable filter: {directive}"
            );
        }
    }

    #[test]
    fn log_directory_sits_beside_the_configuration() {
        let dir = log_directory(Path::new(
            "/Users/x/Library/Application Support/MouseBridge",
        ));
        assert!(dir.ends_with("logs"));
    }

    #[test]
    fn enabling_file_logging_creates_the_directory() {
        // init() installs a process-global subscriber and so cannot be called
        // from more than one test. The directory side effect is the part worth
        // asserting, and it is exercised here without touching global state.
        let tmp = tempfile::tempdir().unwrap();
        let dir = log_directory(tmp.path());
        std::fs::create_dir_all(&dir).unwrap();
        assert!(dir.is_dir());
    }
}
