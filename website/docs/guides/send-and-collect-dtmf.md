---
title: Send and collect DTMF
description: Key through a menu and read the keypresses coming back — RFC 4733 telephone events, not audio tones.
---

# Send and collect DTMF

DTMF is how a call talks to machines: menus, PINs, extensions. sipx sends and receives digits as
RFC 4733 telephone events — named events in the RTP stream, not tones mixed into the audio — so a
keypress survives any codec and cannot be mistaken for someone humming near 941 Hz.

The example below is a real file that CI compiles
([`crates/sipx-call/examples/send_and_collect_dtmf.rs`](https://github.com/codewandler/sipx/blob/main/crates/sipx-call/examples/send_and_collect_dtmf.rs)):

<!-- BEGIN generated:example crates/sipx-call/examples/send_and_collect_dtmf.rs -->
```rust
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
```
<!-- END generated:example -->

Run it against something that answers:

```bash
cargo run --example send_and_collect_dtmf -- 192.0.2.1:5060 "2#"
```

## What is worth noticing

**Formatted numbers are fine.** `send_digits` skips characters that are not DTMF digits, so a
caller holding `"1-800 555 0100"` does not have to strip the punctuation first. Each digit is held
for the duration given — machines at the far end have expectations about how long a keypress
lasts.

**Digits arrive over RTP, not signalling.** Nothing on the signalling channel ever carries a
keypress, which is why
[`recv_digit`](https://codewandler.github.io/sipx/api/sipx_call/call/struct.Call.html#method.recv_digit)
reads from the media session. The
[`serve`](https://codewandler.github.io/sipx/api/sipx_call/call/fn.serve.html) loop does that read
for you and surfaces each keypress as `CallEvent::Dtmf` — a program that runs its own loop owes
the same read, or it will never see a digit.

**Collect against a bound, not a schedule.** The far end presses keys when it presses them. The
example bounds each wait with a timeout as its way of concluding no more digits are coming; the
bound is how long to wait before giving up, not a window to measure in.

**A keypress can also cut a prompt short.** Playback started with `Interrupt::OnDigit` stops at
the far end's first keypress — and that keypress is *not* consumed by the interruption: it still
arrives at `recv_digit` like any other, so a menu never eats the first digit of an extension. See
[Play audio](play-audio.md).

## From the command line

`sipx dial` presses keys once the call is up:

```bash
sipx dial sip:ivr@192.0.2.1:5060 --dtmf "2#" --record reply.wav --timeout 20
```

`sipx answer` reports digits the far end pressed in its JSON result under `dtmf`, and the
scenario actor sends them mid-call:

```bash
sipx scenario <<'EOF'
{"id":"1","command":"dial","uri":"sip:ivr@192.0.2.1:5060"}
{"id":"2","command":"send_dtmf","digits":"2#"}
{"id":"3","command":"hangup"}
{"id":"4","command":"shutdown"}
EOF
```

See the [CLI reference](../reference/cli.md) for output fields and exit codes.
