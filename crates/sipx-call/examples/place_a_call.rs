//! Place a call, play a tone into it, and hang up.
//!
//! Compiled by CI, which is the point: a documentation sample that no longer builds is worse
//! than no sample, because it is read as working code.
//!
//! ```text
//! cargo run --example place_a_call -- 192.0.2.1:5060
//! ```

// These samples are read by people before they are run by machines, so they are written for
// readability where the workspace lints would prefer something terser. `clone_into` over
// `to_owned` teaches nothing in a five-line example, and a sine wave has to become an `i16`
// somewhere.
#![allow(clippy::assigning_clones, clippy::cast_possible_truncation)]

use std::net::SocketAddr;
use std::time::Duration;

use sipx_call::{DialOptions, dial};
use sipx_sip::{Host, HostName, Uri};
use sipx_transport::{Config, Target, bind};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let far_end: SocketAddr = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "127.0.0.1:5060".to_owned())
        .parse()?;

    // Port 0 asks the operating system for one. The address sipx advertises to the far end is
    // separate from the one it binds, because behind a NAT they differ. Use one advertised host
    // for Via, Contact and SDP so the far end is never given three different ways back.
    let mut config = Config::new("0.0.0.0:0".parse()?);
    config.sent_by = "198.51.100.44".to_owned();
    let (endpoint, _incoming) = bind(config).await?;

    let to = Uri::sip(Host::Name(HostName::new("callee.example")?));
    let options = DialOptions::new("<sip:alice@example.net>", "198.51.100.44".parse()?)
        // The public mapping above belongs in SDP but cannot be bound on this host.
        .with_media_bind_address("0.0.0.0".parse()?)
        // Without this the attempt runs until the transaction layer gives up, which is 32
        // seconds. With it, sipx sends CANCEL — so the far end stops ringing rather than being
        // answered by somebody after the caller has gone.
        .with_timeout(Duration::from_secs(20));

    let mut call = dial(&endpoint, Target::udp(far_end), &to, &options).await?;
    println!("connected; media encrypted: {}", call.is_encrypted());

    // 440 Hz for half a second, at the 8 kHz G.711 uses.
    let tone: Vec<i16> = (0..4000)
        .map(|i| {
            let t = f64::from(i) / 8000.0;
            (t * 440.0 * std::f64::consts::TAU)
                .sin()
                .mul_add(12_000.0, 0.0) as i16
        })
        .collect();
    call.media().play(&tone, 160).await;

    call.hang_up().await?;
    Ok(())
}
