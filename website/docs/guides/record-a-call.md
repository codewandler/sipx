---
title: Record a call
description: Record until the caller goes quiet or until enough audio arrived, and write a WAV — with durations measured from the samples, not the clock.
---

# Record a call

Recording answers one of two questions: "record until they stop talking", or "record this much,
however long it takes to arrive". sipx keeps them as two verbs, because a single duration asked to
answer both questions is beaten by whichever is slower on the day.

The example below is a real file that CI compiles
([`crates/sipx-call/examples/record_a_call.rs`](https://github.com/codewandler/sipx/blob/main/crates/sipx-call/examples/record_a_call.rs)):

<!-- BEGIN generated:example crates/sipx-call/examples/record_a_call.rs -->
```rust
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
```
<!-- END generated:example -->

Run it, then dial its address from anything that can place a call:

```bash
cargo run --example record_a_call -- caller.wav
```

## What is worth noticing

**Two verbs, two kinds of duration.**
[`record_until_idle`](https://codewandler.github.io/sipx/api/sipx_call/call/struct.Call.html#method.record_until_idle)'s
`idle` defines silence — how long a hole has to be before "they stopped talking" is true.
[`record_at_least`](https://codewandler.github.io/sipx/api/sipx_call/call/struct.Call.html#method.record_at_least)'s
`within` is a bound on failure — how long to wait before concluding the audio is not coming, set
generously rather than close, because it is not a window to measure in. Load stretches how long
arrival takes; it must not change what was recorded.

**The reported duration describes the recording, not the wait.** `CallEvent::RecordingFinished`
carries a duration measured from the samples themselves and the session's clock rate — not from
timing the call. The trailing silence that *detected* the end is not part of it: it is how the end
was found, not something the far end said.

**What you record is what they sent.** Recording captures received audio at the negotiated clock
rate —
[`negotiated_clock_rate`](https://codewandler.github.io/sipx/api/sipx_call/call/struct.Call.html#method.negotiated_clock_rate)
is what to stamp on a WAV so it plays back at the speed it was spoken. For audio in your own
format instead, `Call::media().capture(...)` converts to a rate you select — see
[Use sipx as a library](as-a-library.md).

**Keep serving the call after the recording.** A recording resolving does not end the call; the
BYE still arrives on the signalling channel, and
[`serve`](https://codewandler.github.io/sipx/api/sipx_call/call/fn.serve.html) is what answers it
and stops the media.

## From the command line

Both call commands write recordings as WAV:

```bash
sipx answer --local 0.0.0.0:5060 --record caller.wav --duration 30 --once
sipx dial sip:bob@192.0.2.1:5060 --record reply.wav --timeout 20
```

The scenario actor records mid-call, bracketed by explicit commands:

```bash
sipx scenario <<'EOF'
{"id":"1","command":"dial","uri":"sip:bob@192.0.2.1:5060"}
{"id":"2","command":"start_recording","path":"reply.wav"}
{"id":"3","command":"stop_recording"}
{"id":"4","command":"hangup"}
{"id":"5","command":"shutdown"}
EOF
```

See the [CLI reference](../reference/cli.md) for output fields and exit codes.
