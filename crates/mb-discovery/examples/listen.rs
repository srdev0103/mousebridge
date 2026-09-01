//! Prints MouseBridge announcements heard on this network.
//!
//! A diagnostic for the question "is the other computer actually broadcasting":
//! run it on either machine while the app is running on the other.

#![allow(clippy::unwrap_used, clippy::expect_used)]

use mb_discovery::BeaconSocket;
use std::time::Duration;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let socket = BeaconSocket::bind(mb_discovery::BEACON_PORT)?;
    println!("listening on port {} …", mb_discovery::BEACON_PORT);

    let deadline = tokio::time::Instant::now() + Duration::from_secs(15);
    let mut heard = 0;

    while tokio::time::Instant::now() < deadline {
        let remaining = deadline - tokio::time::Instant::now();
        match tokio::time::timeout(remaining, socket.recv()).await {
            Ok(Ok((announcement, from))) => {
                heard += 1;
                println!(
                    "  {} ({}) at {} — protocol {}, {}",
                    announcement.name,
                    announcement.device.short(),
                    announcement.connect_address(from),
                    announcement.versions,
                    if announcement.is_compatible() {
                        "compatible"
                    } else {
                        "NEEDS UPDATING"
                    },
                );
            }
            Ok(Err(e)) => {
                eprintln!("socket failed: {e}");
                break;
            }
            Err(_) => break,
        }
    }

    println!("\nheard {heard} announcement(s)");
    Ok(())
}
