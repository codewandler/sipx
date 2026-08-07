---
title: Hold and resume
description: Put a call on hold with a re-INVITE direction, keep sending into the hold, and resume — plus why mute is a different verb.
---

# Hold and resume

Hold is a direction, not a disconnection. RFC 3264 spells a held call `sendonly`: the far end is
asked to stop sending while this side may continue — which is exactly what hold music is. Resuming
is the same renegotiation with `sendrecv` restored. The call, its dialog and its media session all
stay up throughout.

The example below is a real file that CI compiles
([`crates/sipx-call/examples/hold_and_resume.rs`](https://github.com/codewandler/sipx/blob/main/crates/sipx-call/examples/hold_and_resume.rs)):

<!-- BEGIN generated:example crates/sipx-call/examples/hold_and_resume.rs -->
```rust
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
```
<!-- END generated:example -->

Run it against something that answers:

```bash
cargo run --example hold_and_resume -- 192.0.2.1:5060
```

## What is worth noticing

**Hold is a direction and nothing else.** Some stacks spell hold with a null connection address,
but RFC 8839 §4.4.1.1.1 makes `c=0.0.0.0` imply an ICE restart — a hold spelled that way would
restart ICE on every hold and resume. sipx keeps hold what RFC 3264 made it: `sendonly` or
`inactive` in the direction attribute, with everything else about the session unchanged.

**Mute is not hold.** [`Call::mute`](https://codewandler.github.io/sipx/api/sipx_call/call/struct.Call.html#method.mute)
gates this side's outbound audio locally and tells the far end nothing; hold renegotiates, so the
far end knows and may play its own hold music. They answer different questions —
[`is_muted`](https://codewandler.github.io/sipx/api/sipx_call/call/struct.Call.html#method.is_muted)
reports a local decision,
[`is_on_hold`](https://codewandler.github.io/sipx/api/sipx_call/call/struct.Call.html#method.is_on_hold)
reports what the far end asked. Muting instead of holding leaves the far end listening to silence
with no idea why.

**The far end can hold you.** When it does, the call reports `CallEvent::Hold` on the
[event stream](https://codewandler.github.io/sipx/api/sipx_call/event/enum.CallEvent.html) and
`is_on_hold` flips; `CallEvent::Resumed` reports the way back. A renegotiation that does not change
the direction — a session refresh, say — reports nothing, because nothing happened.

**A refused renegotiation leaves the call running.** The error `reinvite` returns is about the
change, not the call: the audio keeps flowing under the description that was already agreed.

## From the command line

The diagnostic actor speaks hold and resume as commands over newline-delimited JSON:

```bash
sipx scenario <<'EOF'
{"id":"1","command":"dial","uri":"sip:bob@192.0.2.1:5060"}
{"id":"2","command":"hold"}
{"id":"3","command":"resume"}
{"id":"4","command":"hangup"}
{"id":"5","command":"shutdown"}
EOF
```

Each command's completion or refusal echoes its `id` — see the
[CLI reference](../reference/cli.md) for the event envelope.
