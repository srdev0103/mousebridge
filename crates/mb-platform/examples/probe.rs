//! Prints what the platform backend sees on this machine.
//!
//! Run with `cargo run -p mb-platform --example probe`. This is the first step of
//! the manual checklist in `docs/platform-validation.md`: the values printed here
//! must match what the OS itself reports.

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let platform = mb_platform::current()?;

    let host = platform.host()?;
    println!("host name  : {}", host.name);
    println!("host system: {}", host.summary());

    let layout = platform.displays()?;
    println!("\ndisplays   : {}", layout.len());
    for d in layout.displays() {
        println!(
            "  {} {} scale={:.2}x{}{}",
            d.id,
            d.bounds,
            d.scale.get(),
            if d.is_primary { " [primary]" } else { "" },
            d.name
                .as_deref()
                .map_or(String::new(), |n| format!(" \"{n}\"")),
        );
    }
    if let Some(bb) = layout.bounding_box() {
        println!("  bounding box: {bb}");
    }

    println!("\npermissions:");
    for p in platform.required_permissions() {
        let status = platform.permission_status(*p);
        println!(
            "  {:<18} {status:?} (usable={})",
            p.to_string(),
            status.is_usable()
        );
    }
    let missing = platform.missing_permissions();
    println!(
        "\nsharing ready: {}",
        if missing.is_empty() {
            "yes".to_owned()
        } else {
            format!("NO - missing {missing:?}")
        }
    );
    Ok(())
}
