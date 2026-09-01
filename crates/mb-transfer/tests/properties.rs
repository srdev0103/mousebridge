//! Property tests for destination-path safety.
//!
//! The example tests cover the attacks we thought of. These cover arbitrary
//! strings, because the input is a filename chosen by another machine and the
//! interesting cases are the ones nobody anticipated.
//!
//! The invariant: **whatever arrives, the destination is a single file directly
//! inside the folder the user chose.**

#![allow(clippy::unwrap_used, clippy::expect_used)]

use mb_transfer::path::{MAX_NAME_LEN, safe_file_name, unique_destination};
use proptest::prelude::*;
use std::path::{Component, Path};

/// Strings drawn from the alphabet that matters: separators, dots, control
/// characters, forbidden punctuation, and ordinary text.
fn arb_name() -> impl Strategy<Value = String> {
    prop::collection::vec(
        prop_oneof![
            Just('/'),
            Just('\\'),
            Just('.'),
            Just(' '),
            Just('\u{0}'),
            Just('\n'),
            Just(':'),
            Just('*'),
            Just('?'),
            Just('"'),
            Just('<'),
            Just('|'),
            Just('a'),
            Just('Z'),
            Just('9'),
            Just('日'),
            Just('🎉'),
        ],
        0..40,
    )
    .prop_map(|chars| chars.into_iter().collect())
}

proptest! {
    /// A sanitised name is always a single path component.
    ///
    /// If it were not, joining it to the download folder could reach somewhere
    /// the user did not choose.
    #[test]
    fn a_sanitised_name_is_always_one_component(raw in arb_name()) {
        let Ok(name) = safe_file_name(&raw) else {
            // Refusing is always an acceptable outcome.
            return Ok(());
        };

        prop_assert!(!name.is_empty());
        prop_assert!(!name.contains('/'), "contains a separator: {name:?}");
        prop_assert!(!name.contains('\\'), "contains a separator: {name:?}");
        prop_assert_ne!(name.as_str(), ".");
        prop_assert_ne!(name.as_str(), "..");

        let path = Path::new(&name);
        let components: Vec<_> = path.components().collect();
        prop_assert_eq!(components.len(), 1, "not one component: {:?}", components);
        prop_assert!(matches!(components[0], Component::Normal(_)));
    }

    /// A sanitised name never carries a character that would be rewritten or
    /// rejected by either platform.
    #[test]
    fn a_sanitised_name_is_usable_on_both_platforms(raw in arb_name()) {
        let Ok(name) = safe_file_name(&raw) else {
            return Ok(());
        };

        prop_assert!(!name.chars().any(char::is_control), "control character: {name:?}");
        for forbidden in ['<', '>', ':', '"', '|', '?', '*'] {
            prop_assert!(!name.contains(forbidden), "contains {forbidden}: {name:?}");
        }
        // Windows silently strips these, producing a different file than the one
        // that was checked.
        prop_assert!(!name.ends_with('.'), "trailing dot: {name:?}");
        prop_assert!(!name.ends_with(' '), "trailing space: {name:?}");
        prop_assert!(name.len() <= MAX_NAME_LEN);
    }

    /// A destination always lands directly inside the chosen folder.
    ///
    /// The statement of the whole crate, over arbitrary input.
    #[test]
    fn a_destination_never_escapes_the_folder(raw in arb_name()) {
        let dir = tempfile::tempdir().expect("tempdir");
        let Ok(path) = unique_destination(dir.path(), &raw) else {
            return Ok(());
        };

        prop_assert!(
            path.starts_with(dir.path()),
            "escaped to {:?} from {:?}",
            path,
            raw
        );

        let relative = path.strip_prefix(dir.path()).expect("inside");
        let components: Vec<_> = relative.components().collect();
        prop_assert_eq!(
            components.len(),
            1,
            "landed {} levels deep from {:?}",
            components.len(),
            raw
        );
        prop_assert!(matches!(components[0], Component::Normal(_)));
    }

    /// Sanitising is idempotent.
    ///
    /// A name that has already been made safe must survive unchanged, or a
    /// second pass anywhere in the pipeline would silently rename the file.
    #[test]
    fn sanitising_twice_changes_nothing(raw in arb_name()) {
        let Ok(once) = safe_file_name(&raw) else {
            return Ok(());
        };
        let twice = safe_file_name(&once).expect("an already-safe name stays safe");
        prop_assert_eq!(once, twice);
    }

    /// An existing file is never chosen as a destination.
    #[test]
    fn an_existing_file_is_never_targeted(raw in arb_name()) {
        let dir = tempfile::tempdir().expect("tempdir");
        let Ok(first) = unique_destination(dir.path(), &raw) else {
            return Ok(());
        };
        std::fs::write(&first, b"the user's file").expect("writes");

        let second = unique_destination(dir.path(), &raw).expect("still valid");
        prop_assert_ne!(&second, &first, "would overwrite an existing file");
        prop_assert!(!second.exists());
    }
}
