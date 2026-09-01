//! Checks that every `unsafe` block carries a `SAFETY` comment.
//!
//! The workspace rule is that `unsafe` is isolated, minimised, and justified in
//! place. The first two are visible in review; the third erodes quietly, because
//! an added block looks exactly like an existing one. This turns the rule into a
//! failing test.
//!
//! `unsafe extern` blocks are declarations rather than operations — there is no
//! precondition to discharge at the declaration — so they are counted separately
//! and documented at the block instead.

use std::fs;
use std::path::{Path, PathBuf};

/// How many lines above an `unsafe` block a `SAFETY:` comment may appear.
///
/// Eight is enough for a multi-line justification without letting an unrelated
/// comment further up the function count as coverage.
const LOOKBACK_LINES: usize = 8;

/// One `unsafe` block with no justification.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Unjustified {
    /// File the block appears in.
    pub file: PathBuf,
    /// One-based line number.
    pub line: usize,
    /// The offending source line, trimmed.
    pub source: String,
}

/// Totals from a scan.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct AuditReport {
    /// `unsafe { .. }` blocks found.
    pub blocks: usize,
    /// `unsafe extern "C" { .. }` declaration blocks found.
    pub extern_blocks: usize,
}

/// Scans a directory tree for `unsafe` blocks without a `SAFETY` comment.
///
/// # Errors
///
/// Returns an [`std::io::Error`] if the tree cannot be walked or a file cannot
/// be read.
pub fn audit(root: &Path) -> std::io::Result<(AuditReport, Vec<Unjustified>)> {
    let mut report = AuditReport::default();
    let mut findings = Vec::new();
    let mut stack = vec![root.to_path_buf()];

    while let Some(dir) = stack.pop() {
        for entry in fs::read_dir(&dir)? {
            let path = entry?.path();
            if path.is_dir() {
                // `target` holds vendored and generated code that is not ours.
                if path.file_name().is_some_and(|n| n == "target") {
                    continue;
                }
                stack.push(path);
            } else if path.extension().is_some_and(|e| e == "rs") {
                audit_file(&path, &mut report, &mut findings)?;
            }
        }
    }
    findings.sort_by(|a, b| a.file.cmp(&b.file).then(a.line.cmp(&b.line)));
    Ok((report, findings))
}

fn audit_file(
    path: &Path,
    report: &mut AuditReport,
    findings: &mut Vec<Unjustified>,
) -> std::io::Result<()> {
    let text = fs::read_to_string(path)?;
    let lines: Vec<&str> = text.lines().collect();

    for (index, line) in lines.iter().enumerate() {
        let trimmed = line.trim();

        if is_unsafe_extern(trimmed) {
            report.extern_blocks += 1;
            continue;
        }
        if !opens_unsafe_block(trimmed) {
            continue;
        }
        report.blocks += 1;

        let start = index.saturating_sub(LOOKBACK_LINES);
        let justified = lines[start..index].iter().any(|l| l.contains("SAFETY:"));
        if !justified {
            findings.push(Unjustified {
                file: path.to_path_buf(),
                line: index + 1,
                source: trimmed.chars().take(80).collect(),
            });
        }
    }
    Ok(())
}

/// True for a line that opens an `unsafe { ... }` block.
fn opens_unsafe_block(line: &str) -> bool {
    if is_unsafe_extern(line) || line.starts_with("//") {
        return false;
    }
    let Some(position) = line.find("unsafe") else {
        return false;
    };
    // Must be followed by a brace, allowing for whitespace.
    line[position + "unsafe".len()..]
        .trim_start()
        .starts_with('{')
}

/// True for an `unsafe extern` declaration block.
fn is_unsafe_extern(line: &str) -> bool {
    !line.starts_with("//") && line.contains("unsafe extern")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognises_an_unsafe_block() {
        assert!(opens_unsafe_block("unsafe {"));
        assert!(opens_unsafe_block("let x = unsafe { foo() };"));
        assert!(opens_unsafe_block("    unsafe  {"));
    }

    #[test]
    fn ignores_declarations_and_prose() {
        assert!(!opens_unsafe_block("unsafe extern \"C\" {"));
        assert!(!opens_unsafe_block("// unsafe { not really }"));
        assert!(!opens_unsafe_block("#![forbid(unsafe_code)]"));
        assert!(!opens_unsafe_block("pub unsafe fn thing() {"));
        assert!(!opens_unsafe_block(
            "/// mentions unsafe { in a doc comment"
        ));
    }

    #[test]
    fn extern_blocks_are_counted_separately() {
        assert!(is_unsafe_extern("unsafe extern \"C\" {"));
        assert!(!is_unsafe_extern("unsafe {"));
    }

    /// The check itself, run against this workspace.
    ///
    /// This is the point of the module: `cargo test --workspace` fails if an
    /// `unsafe` block is added without a justification, so the rule cannot decay
    /// between reviews.
    #[test]
    fn every_unsafe_block_in_this_workspace_is_justified() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("xtask sits directly under the workspace root")
            .join("crates");

        let (report, findings) = audit(&root).expect("workspace is readable");

        assert!(
            findings.is_empty(),
            "{} unsafe block(s) have no SAFETY comment:\n{}",
            findings.len(),
            findings
                .iter()
                .map(|f| format!("  {}:{}  {}", f.file.display(), f.line, f.source))
                .collect::<Vec<_>>()
                .join("\n")
        );

        // A sanity floor: if the scan silently stopped matching, this catches it
        // rather than reporting a clean bill of health for zero blocks examined.
        assert!(
            report.blocks >= 20,
            "only {} unsafe blocks found; the scanner is probably broken",
            report.blocks
        );
    }
}
