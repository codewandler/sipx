//! Play an announcement, then an interruptible prompt, into a call.
//!
//! ```text
//! cargo run --example play_audio -- 192.0.2.1:5060
//! ```

// These samples are read by people before they are run by machines, so they are written for
// readability where the workspace lints would prefer something terser. `clone_into` over
// `to_owned` teaches nothing in a five-line example, and a sine wave has to become an `i16`
// somewhere.
#![allow(clippy::assigning_clones, clippy::cast_possible_truncation)]

use std::net::SocketAddr;
use std::time::Duration;

use sipx_call::{DialOptions, dial};
use sipx_media::Interrupt;
use sipx_sip::{Host, HostName, Uri};
use sipx_transport::{Config, Target, bind};

/// A tone at 8 kHz — stand-in for a real announcement.
fn tone(hz: f64, seconds: usize) -> Vec<i16> {
    (0..8_000 * seconds)
        .map(|i| {
            let t = f64::from(i as u32) / 8000.0;
            (t * hz * std::f64::consts::TAU)
                .sin()
                .mul_add(12_000.0, 0.0) as i16
        })
        .collect()
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let far_end: SocketAddr = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "127.0.0.1:5060".to_owned())
        .parse()?;

    let (endpoint, _incoming) = bind(Config::new("0.0.0.0:0".parse()?)).await?;
    let to = Uri::sip(Host::Name(HostName::new("callee.example")?));
    let options = DialOptions::new("<sip:alice@example.net>", "127.0.0.1".parse()?)
        .with_timeout(Duration::from_secs(20));

    let call = dial(&endpoint, Target::udp(far_end), &to, &options).await?;
    println!("connected");

    // The announcement: paced by the send loop, so this resolves when the audio has actually
    // gone out — not when it was queued. The result says whether it ran to the end or something
    // cut it short, which are different next steps to an application.
    let completed = call.play(&tone(440.0, 1)).await;
    println!(
        "announcement {}",
        if completed {
            "completed"
        } else {
            "was cut short"
        }
    );

    // The prompt: the handle comes back immediately, the caller goes on to other work, and the
    // far end's first keypress stops the clip. That keypress is not consumed by interrupting —
    // it still arrives at `recv_digit` like any other, so a menu never eats the first digit of
    // an extension.
    let prompt = call.start_playback(tone(660.0, 4), Interrupt::OnDigit);
    if let Ok(Some(digit)) = tokio::time::timeout(Duration::from_secs(10), call.recv_digit()).await
    {
        println!("the far end pressed {}", digit.as_char());
    }
    // Harmless if the keypress already stopped it; a prompt that plays on after the caller has
    // answered it is just noise.
    prompt.stop();

    let mut call = call;
    call.hang_up().await?;
    Ok(())
}
