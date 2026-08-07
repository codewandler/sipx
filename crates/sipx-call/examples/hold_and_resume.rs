//! Place a call, put it on hold, play something into the hold, and resume.
//!
//! ```text
//! cargo run --example hold_and_resume -- 192.0.2.1:5060
//! ```

// These samples are read by people before they are run by machines, so they are written for
// readability where the workspace lints would prefer something terser. `clone_into` over
// `to_owned` teaches nothing in a five-line example, and a sine wave has to become an `i16`
// somewhere.
#![allow(clippy::assigning_clones, clippy::cast_possible_truncation)]

use std::net::SocketAddr;
use std::time::Duration;

use sipx_call::{DialOptions, dial};
use sipx_sdp::Direction;
use sipx_sip::{Host, HostName, Uri};
use sipx_transport::{Config, Target, bind};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let far_end: SocketAddr = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "127.0.0.1:5060".to_owned())
        .parse()?;

    let (endpoint, _incoming) = bind(Config::new("0.0.0.0:0".parse()?)).await?;
    let to = Uri::sip(Host::Name(HostName::new("callee.example")?));
    // A local media address, for a local run. A deployed endpoint advertises an address the far
    // end can actually reach — the place-a-call guide covers that.
    let options = DialOptions::new("<sip:alice@example.net>", "127.0.0.1".parse()?)
        .with_timeout(Duration::from_secs(20));

    let mut call = dial(&endpoint, Target::udp(far_end), &to, &options).await?;
    println!("connected");

    // Hold is a direction, not a disconnection (RFC 3264): `sendonly` asks the far end to stop
    // sending while this side may continue. The far end is told, and its side reports being on
    // hold; the dialog and the media session stay up throughout.
    call.reinvite(Direction::SendOnly).await?;
    println!("on hold");

    // Whatever is sent now plays into the hold — this tone is the hold music. 440 Hz for a
    // second, at the 8 kHz G.711 uses.
    let tone: Vec<i16> = (0..8_000)
        .map(|i| {
            let t = f64::from(i) / 8000.0;
            (t * 440.0 * std::f64::consts::TAU)
                .sin()
                .mul_add(12_000.0, 0.0) as i16
        })
        .collect();
    call.play(&tone).await;

    // Resume is the same renegotiation with the direction restored.
    call.reinvite(Direction::SendRecv).await?;
    println!("resumed; audio flows both ways again");

    call.hang_up().await?;
    Ok(())
}
