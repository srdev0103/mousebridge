//! Configuration for MouseBridge.
//!
//! The configuration file is user-editable TOML. Three properties matter more
//! than the schema itself:
//!
//! * **Saves are atomic.** A crash or power loss during a write must never leave
//!   a half-written file, because the next launch would then lose the user's
//!   paired devices and screen layout.
//! * **A corrupt file never blocks startup.** A system utility that refuses to
//!   launch because of one bad line is worse than one that recovers; the damaged
//!   file is preserved alongside so nothing is silently destroyed.
//! * **A file from a newer version is never silently downgraded.** Parsing it
//!   with an older schema would drop fields the user still relies on.
//!
//! See [`store`] for the persistence rules and [`migrate`] for schema evolution.

#![forbid(unsafe_code)]
#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

pub mod migrate;
pub mod schema;
pub mod store;

pub use schema::{
    CURRENT_VERSION, Config, DeviceConfig, LogLevel, LoggingConfig, NetworkConfig, SwitchingConfig,
};
pub use store::{ConfigError, ConfigStore, LoadOutcome};
