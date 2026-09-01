//! Turning a peer-supplied filename into a path that is safe to write.
//!
//! # Policy
//!
//! Two different responses, for two different kinds of problem:
//!
//! * **Rejected** — names with no safe interpretation at all: empty, `.`, `..`,
//!   or nothing but separators. There is no file the user could have meant.
//! * **Sanitised** — names that are merely unusable somewhere: characters
//!   Windows forbids, reserved device names, trailing dots. The user's intent is
//!   clear, and refusing the transfer would be less helpful than adjusting the
//!   name and saying so.
//!
//! Traversal is handled by neither: it is removed by construction, because only
//! the final path component is ever considered.
//!
//! # Both platforms' rules, always
//!
//! A macOS peer can legitimately send `a:b|c.txt`, which Windows cannot store. A
//! Windows peer can send `CON`. Applying only the local platform's rules would
//! mean a file that transfers cleanly in one direction and fails in the other,
//! so both sets are applied on both platforms.

use std::path::{Path, PathBuf};

/// Longest accepted file name, in bytes.
///
/// Below the 255-byte limit both platforms impose, with room for the collision
/// suffix that may be appended afterwards.
pub const MAX_NAME_LEN: usize = 200;

/// Character substituted for one that cannot appear in a file name.
const REPLACEMENT: char = '_';

/// Names Windows reserves for devices, regardless of extension.
///
/// `CON.txt` is still `CON`. A file with one of these names cannot be created on
/// Windows, and on macOS it would create a file that silently fails to transfer
/// back.
const RESERVED: &[&str] = &[
    "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
    "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
];

/// Characters Windows forbids in a file name.
const FORBIDDEN: &[char] = &['<', '>', ':', '"', '/', '\\', '|', '?', '*'];

/// Why a peer-supplied name could not be used.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PathError {
    /// The name was empty, or became empty once separators were removed.
    #[error("the file name is empty")]
    Empty,
    /// The name was `.` or `..`.
    #[error("'{found}' is a directory reference, not a file name")]
    DirectoryReference {
        /// What was received.
        found: String,
    },
    /// The destination could not be resolved, or escaped the target folder.
    #[error("the destination path escapes the download folder")]
    Escape,
    /// The filesystem refused the operation.
    #[error("cannot write to {path}: {detail}")]
    Io {
        /// Path involved.
        path: String,
        /// Underlying cause.
        detail: String,
    },
}

/// Reduces a peer-supplied name to a safe single file name.
///
/// # Errors
///
/// [`PathError::Empty`] or [`PathError::DirectoryReference`] when the name has
/// no safe interpretation. Everything else is sanitised in place.
pub fn safe_file_name(raw: &str) -> Result<String, PathError> {
    // Only the final component is ever considered, and both separators are
    // treated as separators regardless of platform: a Windows peer sends
    // backslashes, a Unix peer sends slashes, and a hostile peer sends whichever
    // the receiver ignores.
    let leaf = raw.rsplit(['/', '\\']).next().unwrap_or("").trim();

    if leaf.is_empty() {
        return Err(PathError::Empty);
    }
    if leaf == "." || leaf == ".." {
        return Err(PathError::DirectoryReference {
            found: leaf.to_owned(),
        });
    }

    // Control characters and forbidden punctuation become underscores. A null
    // byte in particular can truncate a path in any C API downstream.
    let mut name: String = leaf
        .chars()
        .map(|c| {
            if c.is_control() || FORBIDDEN.contains(&c) {
                REPLACEMENT
            } else {
                c
            }
        })
        .collect();

    // Windows silently strips trailing dots and spaces, so `report.txt. ` would
    // become `report.txt` — a different file than the one that was checked.
    while name.ends_with('.') || name.ends_with(' ') {
        name.pop();
    }
    if name.is_empty() {
        return Err(PathError::Empty);
    }

    // Reserved device names, extension or not.
    let stem = name.split('.').next().unwrap_or(&name).to_ascii_uppercase();
    if RESERVED.contains(&stem.as_str()) {
        name.insert(0, REPLACEMENT);
    }

    // Truncate on a character boundary so a multi-byte name is never cut in half.
    if name.len() > MAX_NAME_LEN {
        let mut cut = MAX_NAME_LEN;
        while cut > 0 && !name.is_char_boundary(cut) {
            cut -= 1;
        }
        name.truncate(cut);
        while name.ends_with('.') || name.ends_with(' ') {
            name.pop();
        }
        if name.is_empty() {
            return Err(PathError::Empty);
        }
    }

    Ok(name)
}

/// Builds a destination path inside `folder` that does not already exist.
///
/// A peer sending `notes.txt` must not be able to destroy the user's
/// `notes.txt`, so a colliding name gains a numeric suffix.
///
/// # Errors
///
/// [`PathError::Escape`] if the result is not inside `folder`, which is a
/// belt-and-braces check on top of the name sanitising, and
/// [`PathError::Io`] if the folder cannot be inspected.
pub fn unique_destination(folder: &Path, raw_name: &str) -> Result<PathBuf, PathError> {
    let name = safe_file_name(raw_name)?;

    let candidate = folder.join(&name);
    // The sanitiser should already make this impossible. Checking anyway costs
    // nothing and means a future change to the sanitiser cannot silently open a
    // traversal.
    if !is_inside(folder, &candidate) {
        return Err(PathError::Escape);
    }

    if !candidate.exists() {
        return Ok(candidate);
    }

    let (stem, extension) = split_extension(&name);
    for attempt in 2..10_000u32 {
        let alternative = match extension {
            Some(ext) => format!("{stem} ({attempt}).{ext}"),
            None => format!("{stem} ({attempt})"),
        };
        let path = folder.join(&alternative);
        if !is_inside(folder, &path) {
            return Err(PathError::Escape);
        }
        if !path.exists() {
            return Ok(path);
        }
    }

    Err(PathError::Io {
        path: folder.display().to_string(),
        detail: "too many files with this name already exist".to_owned(),
    })
}

/// Splits a name into stem and extension, without treating a leading dot as one.
///
/// `.gitignore` is a name, not an extension.
fn split_extension(name: &str) -> (&str, Option<&str>) {
    match name.rfind('.') {
        Some(index) if index > 0 => (&name[..index], Some(&name[index + 1..])),
        _ => (name, None),
    }
}

/// True when `candidate` is inside `folder`, comparing lexically.
///
/// Deliberately lexical rather than canonicalised: the destination does not
/// exist yet, so it cannot be canonicalised, and canonicalising the *folder*
/// while joining an unresolved name would not catch anything the component check
/// misses. The name has already been reduced to a single component with no
/// separators, so this is a second line of defence rather than the first.
fn is_inside(folder: &Path, candidate: &Path) -> bool {
    let Ok(relative) = candidate.strip_prefix(folder) else {
        return false;
    };
    let mut components = relative.components();
    let Some(std::path::Component::Normal(_)) = components.next() else {
        return false;
    };
    components.next().is_none()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_ordinary_name_passes_through() {
        assert_eq!(safe_file_name("report.pdf").expect("valid"), "report.pdf");
        assert_eq!(
            safe_file_name("My Document.docx").expect("valid"),
            "My Document.docx"
        );
    }

    #[test]
    fn unicode_names_are_preserved() {
        for name in ["日本語.txt", "café.pdf", "emoji 🎉.png"] {
            assert_eq!(safe_file_name(name).expect("valid"), name);
        }
    }

    #[test]
    fn traversal_is_removed_by_taking_the_leaf() {
        // The canonical attack. Only the final component is ever considered, so
        // there is nothing left to escape with.
        assert_eq!(
            safe_file_name("../../../.ssh/authorized_keys").expect("valid"),
            "authorized_keys"
        );
        assert_eq!(
            safe_file_name("..\\..\\Windows\\System32\\evil.dll").expect("valid"),
            "evil.dll"
        );
        assert_eq!(safe_file_name("/etc/passwd").expect("valid"), "passwd");
        assert_eq!(
            safe_file_name("C:\\Windows\\notepad.exe").expect("valid"),
            "notepad.exe"
        );
    }

    #[test]
    fn both_separators_are_treated_as_separators_on_every_platform() {
        // A hostile peer sends whichever separator the receiver ignores.
        assert_eq!(safe_file_name("a/b\\c/d.txt").expect("valid"), "d.txt");
    }

    #[test]
    fn directory_references_are_refused() {
        // No file the user could have meant.
        assert!(matches!(
            safe_file_name(".."),
            Err(PathError::DirectoryReference { .. })
        ));
        assert!(matches!(
            safe_file_name("."),
            Err(PathError::DirectoryReference { .. })
        ));
        assert!(matches!(safe_file_name("../"), Err(PathError::Empty)));
    }

    #[test]
    fn empty_and_separator_only_names_are_refused() {
        assert_eq!(safe_file_name(""), Err(PathError::Empty));
        assert_eq!(safe_file_name("   "), Err(PathError::Empty));
        assert_eq!(safe_file_name("///"), Err(PathError::Empty));
    }

    #[test]
    fn a_null_byte_cannot_survive() {
        // A null can truncate a path in any C API downstream, turning
        // `safe.txt\0../../evil` into something else entirely.
        let sanitised = safe_file_name("safe.txt\u{0}extra").expect("valid");
        assert!(!sanitised.contains('\u{0}'), "{sanitised}");
    }

    #[test]
    fn control_characters_are_replaced() {
        let sanitised = safe_file_name("bad\nname\ttext.txt").expect("valid");
        assert_eq!(sanitised, "bad_name_text.txt");
    }

    #[test]
    fn characters_windows_forbids_are_replaced_on_every_platform() {
        // A macOS peer can legitimately send these. Applying only the local
        // platform's rules would make transfers work one way and fail the other.
        let sanitised = safe_file_name("a<b>c:d\"e|f?g*h.txt").expect("valid");
        assert_eq!(sanitised, "a_b_c_d_e_f_g_h.txt");
    }

    #[test]
    fn trailing_dots_and_spaces_are_stripped() {
        // Windows silently strips these, producing a different file than the one
        // that was checked.
        assert_eq!(safe_file_name("report.txt. ").expect("valid"), "report.txt");
        assert_eq!(safe_file_name("name...").expect("valid"), "name");
        assert_eq!(safe_file_name("trailing   ").expect("valid"), "trailing");
    }

    #[test]
    fn a_name_that_sanitises_to_nothing_is_refused() {
        assert_eq!(safe_file_name("..."), Err(PathError::Empty));
    }

    #[test]
    fn reserved_device_names_are_defused() {
        // `CON.txt` is still `CON` on Windows, and cannot be created.
        for name in ["CON", "con", "PRN.txt", "aux.tar.gz", "COM1", "LPT9.dat"] {
            let sanitised = safe_file_name(name).expect("valid");
            let stem = sanitised.split('.').next().unwrap().to_ascii_uppercase();
            assert!(
                !RESERVED.contains(&stem.as_str()),
                "{name} sanitised to {sanitised}, still reserved"
            );
        }
    }

    #[test]
    fn a_name_merely_starting_with_a_reserved_word_is_left_alone() {
        // `CONTRACT.pdf` is not a device name and must not be mangled.
        assert_eq!(
            safe_file_name("CONTRACT.pdf").expect("valid"),
            "CONTRACT.pdf"
        );
        assert_eq!(safe_file_name("console.log").expect("valid"), "console.log");
    }

    #[test]
    fn over_long_names_are_truncated_on_a_character_boundary() {
        // A multi-byte name cut mid-character would not be valid UTF-8.
        let long = format!("{}.txt", "日".repeat(300));
        let sanitised = safe_file_name(&long).expect("valid");
        assert!(sanitised.len() <= MAX_NAME_LEN);
        assert!(std::str::from_utf8(sanitised.as_bytes()).is_ok());
    }

    #[test]
    fn a_leading_dot_is_a_name_not_an_extension() {
        assert_eq!(split_extension(".gitignore"), (".gitignore", None));
        assert_eq!(
            split_extension("archive.tar.gz"),
            ("archive.tar", Some("gz"))
        );
        assert_eq!(split_extension("noextension"), ("noextension", None));
    }

    #[test]
    fn a_destination_lands_inside_the_folder() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = unique_destination(dir.path(), "report.pdf").expect("valid");
        assert_eq!(path, dir.path().join("report.pdf"));
        assert!(path.starts_with(dir.path()));
    }

    #[test]
    fn traversal_cannot_escape_the_download_folder() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = unique_destination(dir.path(), "../../../etc/passwd").expect("valid");
        assert_eq!(path, dir.path().join("passwd"));
        assert!(path.starts_with(dir.path()), "escaped to {path:?}");
    }

    #[test]
    fn an_existing_file_is_never_overwritten() {
        // A peer sending `notes.txt` must not be able to destroy the user's.
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("notes.txt"), b"the user's file").expect("writes");

        let path = unique_destination(dir.path(), "notes.txt").expect("valid");
        assert_ne!(path, dir.path().join("notes.txt"));
        assert_eq!(path.file_name().unwrap(), "notes (2).txt");

        // The original is untouched.
        assert_eq!(
            std::fs::read(dir.path().join("notes.txt")).expect("reads"),
            b"the user's file"
        );
    }

    #[test]
    fn repeated_collisions_keep_counting() {
        let dir = tempfile::tempdir().expect("tempdir");
        for name in ["notes.txt", "notes (2).txt", "notes (3).txt"] {
            std::fs::write(dir.path().join(name), b"x").expect("writes");
        }
        let path = unique_destination(dir.path(), "notes.txt").expect("valid");
        assert_eq!(path.file_name().unwrap(), "notes (4).txt");
    }

    #[test]
    fn an_extensionless_collision_is_suffixed_sensibly() {
        let dir = tempfile::tempdir().expect("tempdir");
        std::fs::write(dir.path().join("README"), b"x").expect("writes");
        let path = unique_destination(dir.path(), "README").expect("valid");
        assert_eq!(path.file_name().unwrap(), "README (2)");
    }

    #[test]
    fn the_containment_check_rejects_anything_with_a_separator() {
        // The second line of defence, tested directly rather than through the
        // sanitiser that should make it unreachable.
        let folder = Path::new("/downloads");
        assert!(is_inside(folder, Path::new("/downloads/file.txt")));
        assert!(!is_inside(folder, Path::new("/downloads/sub/file.txt")));
        assert!(!is_inside(folder, Path::new("/elsewhere/file.txt")));
        assert!(!is_inside(folder, Path::new("/downloads")));
    }
}
