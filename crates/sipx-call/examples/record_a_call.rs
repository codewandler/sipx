//! Answer a call, record the caller until they go quiet, and write a WAV file.
//!
//! ```text
//! cargo run --example record_a_call -- caller.wav
//! ```

// These samples are read by people before they are run by machines, so they are written for
// readability where the workspace lints would prefer something terser. `clone_into` over
// `to_owned` teaches nothing in a five-line example, and a sine wave has to become an `i16`
// somewhere.
#![allow(clippy::assigning_clones, clippy::cast_possible_truncation)]

use std::time::Duration;

use sipx_audio::{Wav, write_wav};
use sipx_call::{answer, serve};
use sipx_sip::Method;
use sipx_transport::{Config, bind};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let path = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "caller.wav".to_owned());

    let (endpoint, mut incoming) = bind(Config::new("0.0.0.0:5060".parse()?)).await?;
    println!("listening on {}", endpoint.local_addr());

    while let Some(request) = incoming.recv().await {
        if request.request.method != Method::Invite {
            continue;
        }

        // A local media address, for a local run. A deployed endpoint advertises an address the
        // caller can actually reach — the answer-a-call guide covers that.
        let mut call = answer(&endpoint, &request, "127.0.0.1".parse()?).await?;
        println!("answered; recording");

        // Record until the caller goes quiet: the half second defines silence — how long a hole
        // has to be before "they stopped talking" is true. The trailing silence that detected
        // the end is not part of what comes back. For "record this much, however long it takes",
        // `record_at_least` is the counted twin, whose duration is a bound on failure instead.
        let heard = call
            .media()
            .record_until_idle(Duration::from_millis(500))
            .await;

        // Stamp the WAV with the negotiated clock rate, so the file plays back at the speed the
        // caller spoke — the samples are whatever rate the codec agreed, not always 8 kHz.
        let wav = Wav {
            sample_rate: call.negotiated_clock_rate(),
            samples: heard,
        };
        println!("recorded {:?} to {path}", wav.duration());
        write_wav(std::fs::File::create(&path)?, &wav)?;

        // The recording resolving did not end the call. Keep serving it: the BYE arrives on the
        // signalling channel, and answering it is what stops the media.
        serve(&mut call, &mut incoming).await?;
        println!("the call ended");
        break;
    }
    Ok(())
}
