//! Workspace task runner. See [`xtask`] for the individual checks.

fn main() -> std::io::Result<()> {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap_or_else(|| std::path::Path::new("."))
        .join("crates");

    let (report, findings) = xtask::unsafe_audit::audit(&root)?;
    println!("unsafe blocks        : {}", report.blocks);
    println!("unsafe extern blocks : {}", report.extern_blocks);
    println!("unjustified          : {}", findings.len());
    for f in &findings {
        println!("  {}:{}  {}", f.file.display(), f.line, f.source);
    }
    if findings.is_empty() {
        Ok(())
    } else {
        std::process::exit(1);
    }
}
