//! Dial a menu, key through it, and read the digits the far end presses back.
//!
//! ```text
//! cargo run --example send_and_collect_dtmf -- 192.0.2.1:5060 "2#"
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
    let mut args = std::env::args().skip(1);
    let far_end: SocketAddr = args
        .next()
        .unwrap_or_else(|| "127.0.0.1:5060".to_owned())
        .parse()?;
    let digits = args.next().unwrap_or_else(|| "2#".to_owned());

    let (endpoint, _incoming) = bind(Config::new("0.0.0.0:0".parse()?)).await?;
    let to = Uri::sip(Host::Name(HostName::new("callee.example")?));
    let options = DialOptions::new("<sip:alice@example.net>", "127.0.0.1".parse()?)
        .with_timeout(Duration::from_secs(20));

    let call = dial(&endpoint, Target::udp(far_end), &to, &options).await?;
    println!("connected");

    // Digits go out as RFC 4733 telephone events in the RTP stream, not as tones in the audio,
    // so they survive any codec. Characters that are not DTMF digits are skipped — a formatted
    // number does not need its punctuation stripped first. Each digit is held for the duration
    // given, which is what the machine at the far end times.
    if call.send_digits(&digits, Duration::from_millis(100)).await {
        println!("sent {digits}");
    }

    // Read what the far end presses back. Digits arrive over RTP, so nothing on the signalling
    // channel ever carries one — this read on the call is the only place they surface. The five
    // seconds is a bound on failure: how long to wait before concluding no more are coming, not
    // a window to measure in.
    while let Ok(Some(digit)) =
        tokio::time::timeout(Duration::from_secs(5), call.recv_digit()).await
    {
        println!("the far end pressed {}", digit.as_char());
        if digit == sipx_rtp::Digit::Hash {
            break;
        }
    }

    let mut call = call;
    call.hang_up().await?;
    Ok(())
}
