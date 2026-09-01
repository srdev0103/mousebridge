//! Locating, loading and atomically saving the configuration file.

use crate::migrate::{self, MigrationError, MigrationOutcome};
use crate::schema::{Config, ValidationError};
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};
use tracing::{info, warn};

/// Directory name under the platform's per-user configuration root.
///
/// Must stay stable for the life of the product: changing it strands every
/// existing user's settings in a directory nothing reads any more.
///
/// `BaseDirs` is used rather than `ProjectDirs` deliberately. `ProjectDirs`
/// builds a reverse-DNS directory (`com.MouseBridge.MouseBridge` on macOS,
/// `MouseBridge\MouseBridge` on Windows), which is correct for a library but
/// wrong for a shipped desktop application: users open this folder to inspect
/// or back up their settings, and the doubled name is confusing in both places.
const APP_DIR: &str = "MouseBridge";

/// Name of the configuration file within the config directory.
const CONFIG_FILE: &str = "config.toml";

/// What happened while loading.
#[derive(Debug)]
pub enum LoadOutcome {
    /// An existing file was read successfully.
    Loaded(Config),
    /// No file existed, so a first-run configuration was created and saved.
    Created(Config),
    /// The file was upgraded from an older schema version and saved back.
    Migrated {
        /// The upgraded configuration.
        config: Config,
        /// Version the file was written at.
        from_version: u32,
    },
    /// The file was unreadable. It has been preserved and defaults are in use.
    ///
    /// The UI must surface this: the user's paired devices and screen layout are
    /// gone from the live configuration, and quietly carrying on would look like
    /// the application had forgotten them for no reason.
    Recovered {
        /// The freshly bootstrapped configuration now in use.
        config: Config,
        /// Where the damaged file was moved to.
        backup: PathBuf,
        /// Why the original could not be used.
        reason: String,
    },
}

impl LoadOutcome {
    /// Borrows the configuration, whichever way it was obtained.
    #[must_use]
    pub const fn config(&self) -> &Config {
        match self {
            Self::Loaded(c)
            | Self::Created(c)
            | Self::Migrated { config: c, .. }
            | Self::Recovered { config: c, .. } => c,
        }
    }

    /// Consumes the outcome and returns the configuration.
    #[must_use]
    pub fn into_config(self) -> Config {
        match self {
            Self::Loaded(c)
            | Self::Created(c)
            | Self::Migrated { config: c, .. }
            | Self::Recovered { config: c, .. } => c,
        }
    }
}

/// Failures that stop configuration being loaded or saved.
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    /// The platform config directory could not be determined.
    #[error("cannot determine the configuration directory for this platform")]
    NoConfigDir,
    /// A filesystem operation failed.
    #[error("configuration i/o failed at {path}: {source}")]
    Io {
        /// Path being operated on.
        path: PathBuf,
        /// Underlying error.
        #[source]
        source: io::Error,
    },
    /// The document came from a newer build, or a step failed.
    #[error(transparent)]
    Migration(#[from] MigrationError),
    /// A value was outside its acceptable range.
    #[error(transparent)]
    Validation(#[from] ValidationError),
    /// The configuration could not be serialised.
    #[error("cannot serialise configuration: {0}")]
    Serialise(#[from] toml::ser::Error),
    /// A first-run configuration could not be built.
    #[error(transparent)]
    Bootstrap(#[from] crate::schema::BootstrapError),
}

fn io_err(path: &Path) -> impl FnOnce(io::Error) -> ConfigError + '_ {
    move |source| ConfigError::Io {
        path: path.to_path_buf(),
        source,
    }
}

/// Reads and writes the configuration file at a fixed path.
#[derive(Debug, Clone)]
pub struct ConfigStore {
    path: PathBuf,
}

impl ConfigStore {
    /// Uses an explicit path. Primarily for tests and for a `--config` flag.
    #[must_use]
    pub const fn with_path(path: PathBuf) -> Self {
        Self { path }
    }

    /// Uses the platform's standard per-user configuration location.
    ///
    /// `~/Library/Application Support/MouseBridge` on macOS,
    /// `%APPDATA%\MouseBridge` on Windows.
    ///
    /// # Errors
    ///
    /// Returns [`ConfigError::NoConfigDir`] if the platform has no home directory.
    pub fn at_default_location() -> Result<Self, ConfigError> {
        let dirs = directories::BaseDirs::new().ok_or(ConfigError::NoConfigDir)?;
        Ok(Self::with_path(
            dirs.config_dir().join(APP_DIR).join(CONFIG_FILE),
        ))
    }

    /// Path of the configuration file.
    #[must_use]
    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Directory containing the configuration file.
    #[must_use]
    pub fn directory(&self) -> &Path {
        self.path.parent().unwrap_or(Path::new("."))
    }

    /// Loads the configuration, creating or recovering it as necessary.
    ///
    /// `default_name` is used only when a first-run configuration has to be
    /// built. The caller supplies it because resolving the computer's name is an
    /// OS query, which belongs in the platform layer rather than here.
    ///
    /// # Errors
    ///
    /// Returns an error only for conditions a retry cannot fix and where
    /// continuing would destroy user data: a file from a newer build, or an
    /// unwritable configuration directory. Malformed content is *recovered*
    /// rather than reported as an error, because refusing to start is a worse
    /// outcome for a background utility than starting with defaults.
    pub fn load_or_bootstrap(&self, default_name: &str) -> Result<LoadOutcome, ConfigError> {
        let text = match fs::read_to_string(&self.path) {
            Ok(text) => text,
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                let config = Config::bootstrap(default_name)?;
                self.save(&config)?;
                info!(path = %self.path.display(), "created initial configuration");
                return Ok(LoadOutcome::Created(config));
            }
            Err(e) => return Err(io_err(&self.path)(e)),
        };

        match self.parse(&text) {
            Ok((config, MigrationOutcome::AlreadyCurrent)) => Ok(LoadOutcome::Loaded(config)),
            Ok((config, MigrationOutcome::Upgraded { from })) => {
                self.save(&config)?;
                info!(from_version = from, "migrated configuration");
                Ok(LoadOutcome::Migrated {
                    config,
                    from_version: from,
                })
            }
            // A newer file is never recovered from; that would discard settings.
            Err(e @ ConfigError::Migration(MigrationError::FromFuture { .. })) => Err(e),
            Err(reason) => {
                let backup = self.preserve_damaged_file()?;
                let config = Config::bootstrap(default_name)?;
                self.save(&config)?;
                warn!(
                    backup = %backup.display(),
                    %reason,
                    "configuration was unreadable; preserved it and reset to defaults"
                );
                Ok(LoadOutcome::Recovered {
                    config,
                    backup,
                    reason: reason.to_string(),
                })
            }
        }
    }

    /// Parses text into a validated, migrated configuration.
    fn parse(&self, text: &str) -> Result<(Config, MigrationOutcome), ConfigError> {
        let mut doc: toml::Table = text.parse().map_err(|e: toml::de::Error| {
            ConfigError::Validation(ValidationError {
                field: "<document>",
                reason: e.to_string(),
            })
        })?;

        let outcome = migrate::migrate(&mut doc)?;

        let config: Config = doc.try_into().map_err(|e: toml::de::Error| {
            ConfigError::Validation(ValidationError {
                field: "<document>",
                reason: e.to_string(),
            })
        })?;
        config.validate()?;
        Ok((config, outcome))
    }

    /// Moves an unreadable file aside, returning where it went.
    fn preserve_damaged_file(&self) -> Result<PathBuf, ConfigError> {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_secs());
        let backup = self.path.with_extension(format!("toml.corrupt-{stamp}"));
        fs::rename(&self.path, &backup).map_err(io_err(&self.path))?;
        Ok(backup)
    }

    /// Writes the configuration atomically.
    ///
    /// Serialise, write to a sibling temporary file, flush it to stable storage,
    /// then rename over the target. Rename within a directory is atomic on both
    /// APFS and NTFS, so a crash at any point leaves either the previous file or
    /// the new one intact — never a truncated mixture. Writing in place would
    /// risk exactly that, and the casualty would be the user's paired devices.
    ///
    /// # Errors
    ///
    /// Returns [`ConfigError::Io`] if the directory cannot be created or written,
    /// or [`ConfigError::Serialise`] if the value cannot be encoded.
    pub fn save(&self, config: &Config) -> Result<(), ConfigError> {
        let dir = self.directory();
        fs::create_dir_all(dir).map_err(io_err(dir))?;

        let text = toml::to_string_pretty(config)?;
        let tmp = self.path.with_extension("toml.tmp");

        {
            use io::Write as _;
            let mut file = fs::File::create(&tmp).map_err(io_err(&tmp))?;
            file.write_all(text.as_bytes()).map_err(io_err(&tmp))?;
            // Without this, the rename can land before the data does.
            file.sync_all().map_err(io_err(&tmp))?;
        }

        restrict_permissions(&tmp)?;

        // std::fs::rename replaces an existing destination on both Unix and
        // Windows (MoveFileEx with MOVEFILE_REPLACE_EXISTING).
        fs::rename(&tmp, &self.path).map_err(io_err(&self.path))?;
        Ok(())
    }
}

/// Restricts the file to the owning user where the platform supports it.
///
/// The configuration holds this installation's device identity, and from
/// milestone 10 the set of devices it trusts. Neither should be readable by other
/// local accounts.
#[cfg(unix)]
fn restrict_permissions(path: &Path) -> Result<(), ConfigError> {
    use std::os::unix::fs::PermissionsExt as _;
    fs::set_permissions(path, fs::Permissions::from_mode(0o600)).map_err(io_err(path))
}

/// Windows uses inherited ACLs from the per-user AppData directory, which is
/// already restricted to the owning account, so there is nothing to tighten here.
#[cfg(not(unix))]
fn restrict_permissions(_path: &Path) -> Result<(), ConfigError> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::schema::{CURRENT_VERSION, LogLevel};

    fn store(dir: &tempfile::TempDir) -> ConfigStore {
        ConfigStore::with_path(dir.path().join("config.toml"))
    }

    #[test]
    fn first_run_creates_and_persists() {
        let dir = tempfile::tempdir().unwrap();
        let store = store(&dir);

        let outcome = store.load_or_bootstrap("My Mac").unwrap();
        assert!(matches!(outcome, LoadOutcome::Created(_)));
        let created = outcome.into_config();
        assert!(store.path().exists());

        // Second load must return the same identity, not mint a new one.
        let again = store.load_or_bootstrap("My Mac").unwrap();
        assert!(matches!(again, LoadOutcome::Loaded(_)));
        assert_eq!(again.into_config().device.id, created.device.id);
    }

    #[test]
    fn save_is_atomic_and_leaves_no_temp_file() {
        let dir = tempfile::tempdir().unwrap();
        let store = store(&dir);
        let mut config = Config::bootstrap("Mac").unwrap();
        store.save(&config).unwrap();

        config.logging.level = LogLevel::Debug;
        store.save(&config).unwrap();

        let entries: Vec<_> = fs::read_dir(dir.path())
            .unwrap()
            .filter_map(Result::ok)
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        assert_eq!(
            entries,
            vec!["config.toml".to_owned()],
            "temp file left behind"
        );

        let reloaded = store.load_or_bootstrap("Mac").unwrap().into_config();
        assert_eq!(reloaded.logging.level, LogLevel::Debug);
    }

    #[cfg(unix)]
    #[test]
    fn saved_file_is_not_world_readable() {
        use std::os::unix::fs::PermissionsExt as _;
        let dir = tempfile::tempdir().unwrap();
        let store = store(&dir);
        store.save(&Config::bootstrap("Mac").unwrap()).unwrap();
        let mode = fs::metadata(store.path()).unwrap().permissions().mode();
        assert_eq!(mode & 0o077, 0, "config is readable by other accounts");
    }

    #[test]
    fn corrupt_file_is_preserved_and_recovered_from() {
        let dir = tempfile::tempdir().unwrap();
        let store = store(&dir);
        fs::write(store.path(), "this is not [ valid toml").unwrap();

        let outcome = store.load_or_bootstrap("Mac").unwrap();
        let LoadOutcome::Recovered { backup, config, .. } = outcome else {
            panic!("expected recovery, got {outcome:?}");
        };
        assert!(backup.exists(), "damaged file must be kept, not deleted");
        assert_eq!(
            fs::read_to_string(&backup).unwrap(),
            "this is not [ valid toml"
        );
        assert_eq!(config.version, CURRENT_VERSION);
        // The replacement must be on disk, so the next launch is clean.
        assert!(matches!(
            store.load_or_bootstrap("Mac").unwrap(),
            LoadOutcome::Loaded(_)
        ));
    }

    #[test]
    fn out_of_range_values_are_recovered_from_not_obeyed() {
        let dir = tempfile::tempdir().unwrap();
        let store = store(&dir);
        let id = mb_types::DeviceId::generate().unwrap().to_hex();
        fs::write(
            store.path(),
            format!(
                "version = 1\n[device]\nid = \"{id}\"\nname = \"Mac\"\n\
                 [switching]\ncooldown_ms = 999999\n"
            ),
        )
        .unwrap();

        let outcome = store.load_or_bootstrap("Mac").unwrap();
        let LoadOutcome::Recovered { reason, .. } = &outcome else {
            panic!("expected recovery, got {outcome:?}");
        };
        assert!(reason.contains("cooldown_ms"), "unhelpful reason: {reason}");
    }

    #[test]
    fn a_file_from_a_newer_build_is_an_error_not_a_reset() {
        // Resetting here would silently discard settings the user still needs.
        let dir = tempfile::tempdir().unwrap();
        let store = store(&dir);
        fs::write(store.path(), "version = 9999\n").unwrap();

        let err = store.load_or_bootstrap("Mac").unwrap_err();
        assert!(matches!(
            err,
            ConfigError::Migration(MigrationError::FromFuture { found: 9999, .. })
        ));
        // Crucially, the original file is untouched.
        assert_eq!(
            fs::read_to_string(store.path()).unwrap(),
            "version = 9999\n"
        );
    }

    #[test]
    fn missing_directories_are_created() {
        let dir = tempfile::tempdir().unwrap();
        let nested = dir.path().join("a").join("b").join("config.toml");
        let store = ConfigStore::with_path(nested.clone());
        store.save(&Config::bootstrap("Mac").unwrap()).unwrap();
        assert!(nested.exists());
    }

    #[test]
    fn default_location_is_derivable_and_correctly_shaped() {
        let store = ConfigStore::at_default_location().unwrap();
        assert!(store.path().ends_with("config.toml"));
        assert_eq!(
            store.directory().file_name().and_then(|n| n.to_str()),
            Some(APP_DIR),
            "config must live in a single plainly named directory"
        );

        #[cfg(target_os = "macos")]
        assert!(
            store
                .path()
                .to_string_lossy()
                .contains("Library/Application Support/MouseBridge/config.toml"),
            "unexpected macOS path: {}",
            store.path().display()
        );
    }
}
