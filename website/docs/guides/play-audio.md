---
title: Play audio
description: Paced playback that reports how it ended, queued clips, and prompts a keypress can cut short.
---

# Play audio

Playing audio into a call sounds like one verb and is really three: play a clip and wait for it,
start a clip and keep working while it plays, and start a prompt the far end can interrupt with a
keypress. sipx keeps all three on the same primitive so they cannot drift apart.

The example below is a real file that CI compiles
([`crates/sipx-call/examples/play_audio.rs`](https://github.com/codewandler/sipx/blob/main/crates/sipx-call/examples/play_audio.rs)):

<!-- BEGIN generated:example crates/sipx-call/examples/play_audio.rs -->
```rust
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
```
<!-- END generated:example -->

Run it against something that answers:

```bash
cargo run --example play_audio -- 192.0.2.1:5060
```

## What is worth noticing

**`play` resolves when the audio has gone out, not when it was queued.** The clip is paced by the
send loop at the session's own packet size, so it plays correctly under a codec whose clock is not
8 kHz without the caller knowing the rate. And it is cancel-on-drop: abandoning the future — a
`timeout` that fires, a lost `select!` — stops the clip rather than leaving it playing out of a
task nobody holds.

**"Finished" and "cut short" lead to different next steps.** Every playback reports
`CallEvent::PlaybackFinished` with `completed` saying which happened. "The announcement finished"
and "the caller hung up during the announcement" are not the same event to an application driving
the call from its stream.

**Clips queue.** A second playback started while one is running begins when that one ends, up to
a finite depth — the queue is bounded because under the application contract the thing starting
playbacks is not always code somebody wrote by hand. Stopping is bounded too:
[`Playback::STOP_BOUND_PACKETS`](https://codewandler.github.io/sipx/api/sipx_media/session/struct.Playback.html)
packets may still reach the wire after a stop, which is a number rather than "promptly" so
barge-in has a latency an application can state.

**A prompt the caller can interrupt is `start_playback` with `Interrupt::OnDigit`.** The handle
comes back immediately; the far end's first keypress stops the clip, and that keypress is not
consumed — it arrives at
[`recv_digit`](https://codewandler.github.io/sipx/api/sipx_call/call/struct.Call.html#method.recv_digit)
like any other, so "play a prompt and collect digits" never eats the first digit of a PIN. See
[Send and collect DTMF](send-and-collect-dtmf.md).

**Your audio does not have to be 8 kHz mono.**
[`play_pcm`](https://codewandler.github.io/sipx/api/sipx_call/call/struct.Call.html#method.play_pcm)
takes explicit linear PCM with its own rate and depth and converts to the negotiated codec clock,
refusing with a typed error before anything is queued when it cannot. Nothing infers a format from
a byte count.

## From the command line

`sipx dial` and `sipx answer` both play WAV files, resampling when the file's rate differs from
the call's:

```bash
sipx dial sip:bob@192.0.2.1:5060 --play hello.wav --timeout 20
```

The scenario actor starts and stops playback mid-call:

```bash
sipx scenario <<'EOF'
{"id":"1","command":"dial","uri":"sip:bob@192.0.2.1:5060"}
{"id":"2","command":"play","path":"hello.wav"}
{"id":"3","command":"stop_playback"}
{"id":"4","command":"hangup"}
{"id":"5","command":"shutdown"}
EOF
```

See the [CLI reference](../reference/cli.md) for the full command set.
