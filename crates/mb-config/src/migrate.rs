//! Schema migration.
//!
//! Migrations run on the raw TOML document *before* it is deserialised into
//! [`Config`](crate::Config). Deserialising first would defeat the purpose: the
//! current schema cannot represent fields that only existed in an older version,
//! so those values would be lost before a migration ever saw them.

use crate::schema::CURRENT_VERSION;
use toml::Table;

/// A single step, taking a document from `from_version` to `from_version + 1`.
type Step = fn(&mut Table) -> Result<(), MigrationError>;

/// Ordered migration steps.
///
/// Entry `n` upgrades version `n + 1` to version `n + 2`. Version 1 is the first
/// schema, so this is empty until the first breaking change ships.
const STEPS: &[Step] = &[];

/// What [`migrate`] did to the document.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MigrationOutcome {
    /// The document was already current; nothing changed.
    AlreadyCurrent,
    /// The document was upgraded from the given version.
    Upgraded {
        /// Version the document was written at.
        from: u32,
    },
}

/// Reasons a document could not be brought to the current schema.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum MigrationError {
    /// The document was written by a newer build.
    ///
    /// Deliberately fatal rather than best-effort. Parsing a newer document with
    /// an older schema would drop every field this build does not know about,
    /// and the next save would write that loss back to disk permanently.
    #[error(
        "configuration is version {found}, but this build understands at most \
         version {supported}; update MouseBridge or move the file aside"
    )]
    FromFuture {
        /// Version found in the file.
        found: u32,
        /// Highest version this build supports.
        supported: u32,
    },
    /// The `version` key was present but not an integer.
    #[error("configuration `version` must be a positive integer")]
    BadVersionField,
    /// A step could not transform the document.
    #[error("migration from version {from} failed: {reason}")]
    StepFailed {
        /// Version the failing step started from.
        from: u32,
        /// What went wrong.
        reason: String,
    },
}

/// Brings a raw configuration document up to [`CURRENT_VERSION`].
///
/// # Errors
///
/// See [`MigrationError`].
pub fn migrate(doc: &mut Table) -> Result<MigrationOutcome, MigrationError> {
    migrate_with(doc, STEPS, CURRENT_VERSION)
}

/// Implementation of [`migrate`], parameterised for testing.
fn migrate_with(
    doc: &mut Table,
    steps: &[Step],
    target: u32,
) -> Result<MigrationOutcome, MigrationError> {
    let found = match doc.get("version") {
        // A file predating the version key is version 1 by definition.
        None => 1,
        Some(v) => {
            let n = v.as_integer().ok_or(MigrationError::BadVersionField)?;
            u32::try_from(n).map_err(|_| MigrationError::BadVersionField)?
        }
    };

    if found > target {
        return Err(MigrationError::FromFuture {
            found,
            supported: target,
        });
    }
    if found == target {
        return Ok(MigrationOutcome::AlreadyCurrent);
    }

    for version in found..target {
        let index = usize::try_from(version - 1).unwrap_or(usize::MAX);
        let step = steps.get(index).ok_or(MigrationError::StepFailed {
            from: version,
            reason: format!("no migration step registered for version {version}"),
        })?;
        step(doc).map_err(|e| MigrationError::StepFailed {
            from: version,
            reason: e.to_string(),
        })?;
        doc.insert(
            "version".to_owned(),
            toml::Value::Integer(i64::from(version + 1)),
        );
    }

    Ok(MigrationOutcome::Upgraded { from: found })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc(text: &str) -> Table {
        text.parse::<Table>().expect("valid toml")
    }

    #[test]
    fn current_version_is_left_alone() {
        let mut d = doc("version = 1\n");
        assert_eq!(migrate(&mut d), Ok(MigrationOutcome::AlreadyCurrent));
    }

    #[test]
    fn missing_version_is_treated_as_one() {
        let mut d = doc("[device]\nname = \"Mac\"\n");
        assert_eq!(migrate(&mut d), Ok(MigrationOutcome::AlreadyCurrent));
    }

    #[test]
    fn a_newer_document_is_refused_not_downgraded() {
        // The one case where failing to start is the correct behaviour: the
        // alternative silently destroys settings the user still depends on.
        let mut d = doc("version = 99\n");
        assert_eq!(
            migrate(&mut d),
            Err(MigrationError::FromFuture {
                found: 99,
                supported: CURRENT_VERSION,
            })
        );
    }

    #[test]
    fn a_malformed_version_is_refused() {
        assert_eq!(
            migrate(&mut doc("version = \"one\"\n")),
            Err(MigrationError::BadVersionField)
        );
        assert_eq!(
            migrate(&mut doc("version = -3\n")),
            Err(MigrationError::BadVersionField)
        );
    }

    // The step machinery has no real steps yet, so exercise it with synthetic
    // ones. This is what proves the chain works before the first real migration
    // has to be trusted with a user's settings.
    fn rename_port(d: &mut Table) -> Result<(), MigrationError> {
        if let Some(v) = d.remove("prt") {
            d.insert("port".to_owned(), v);
        }
        Ok(())
    }
    fn add_flag(d: &mut Table) -> Result<(), MigrationError> {
        d.insert("discovery_enabled".to_owned(), toml::Value::Boolean(true));
        Ok(())
    }

    #[test]
    fn steps_chain_in_order_and_stamp_the_version() {
        let mut d = doc("version = 1\nprt = 25470\n");
        let outcome = migrate_with(&mut d, &[rename_port, add_flag], 3).expect("migrates");
        assert_eq!(outcome, MigrationOutcome::Upgraded { from: 1 });
        assert_eq!(d.get("version").and_then(toml::Value::as_integer), Some(3));
        assert_eq!(d.get("port").and_then(toml::Value::as_integer), Some(25470));
        assert!(d.get("prt").is_none());
        assert_eq!(
            d.get("discovery_enabled").and_then(toml::Value::as_bool),
            Some(true)
        );
    }

    #[test]
    fn a_partial_chain_starts_from_the_documents_own_version() {
        let mut d = doc("version = 2\n");
        let outcome = migrate_with(&mut d, &[rename_port, add_flag], 3).expect("migrates");
        assert_eq!(outcome, MigrationOutcome::Upgraded { from: 2 });
        // Step 1 must not have run: the document was already past it.
        assert!(d.get("port").is_none());
        assert_eq!(
            d.get("discovery_enabled").and_then(toml::Value::as_bool),
            Some(true)
        );
    }

    #[test]
    fn a_missing_step_is_an_error_not_a_silent_skip() {
        let mut d = doc("version = 1\n");
        let err = migrate_with(&mut d, &[], 5).expect_err("no steps registered");
        assert!(matches!(err, MigrationError::StepFailed { from: 1, .. }));
    }
}
