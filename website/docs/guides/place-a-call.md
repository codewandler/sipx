---
title: Place a call
description: Dial from Rust with sipx-call — timeouts that CANCEL, SRTP that reports itself, and session timers that notice a vanished far end.
---

# Place a call

The example below is a real file that CI compiles
([`crates/sipx-call/examples/place_a_call.rs`](https://github.com/codewandler/sipx/blob/main/crates/sipx-call/examples/place_a_call.rs)):

<!-- BEGIN generated:example crates/sipx-call/examples/place_a_call.rs -->
```rust
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
    // separate from the one it binds, because behind a NAT they differ.
    let (endpoint, _incoming) = bind(Config::new("0.0.0.0:0".parse()?)).await?;

    let to = Uri::sip(Host::Name(HostName::new("callee.example")?));
    let options = DialOptions::new("<sip:alice@example.net>", "127.0.0.1".parse()?)
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
```
<!-- END generated:example -->

Run it against something that answers:

```bash
cargo run --example place_a_call -- 192.0.2.1:5060
```

## What is worth noticing

**The timeout is not optional.** Without `with_timeout`, the attempt runs until the transaction
layer gives up — 32 seconds with the default timers. With it, sipx sends a CANCEL, so the far
end stops ringing. Giving up is not the same as ceasing to wait: without the CANCEL somebody can
answer a call the caller has already abandoned.

**`is_encrypted` answers a real question.** A call whose signalling is encrypted and whose media
is not looks identical from the outside to one where both are. sipx negotiates SRTP when the
transport protects the key and not otherwise, and says which happened.

**The advertised address is separate from the bound one.** Behind a NAT they differ, and the
socket's view is the wrong one to put in a `Contact`.

## From the command line

The same thing without writing any Rust:

```bash
sipx dial sip:bob@192.0.2.1:5060 --play hello.wav --record reply.wav --timeout 20
```

Every command speaks `--json` and returns a distinct exit code per outcome — see the
[CLI reference](../reference/cli.md).

## When the far end vanishes

A SIP dialog has no keepalive. If the phone at the other end loses power it closes no socket and
sends no BYE, so the call stays up — on your side — until something notices. Nothing else in SIP
will.

`with_session_timer` is what notices (RFC 4028):

```rust
let options = DialOptions::new("<sip:alice@example.net>", local_ip)
    .with_session_timer(Duration::from_secs(600));
```

The two ends agree an interval, one of them refreshes inside it, and whichever side stops seeing
refreshes hangs up locally and stops the media. Which side refreshes is negotiated, not chosen —
`Call::session_interval()` reports the interval that was agreed and whether this side has the
job.

A timer is a deadline, so a call that only ever wakes on incoming traffic can never notice that
*no* traffic arrived.
[`serve`](https://codewandler.github.io/sipx/api/sipx_call/call/fn.serve.html) is the loop that
handles both:

```rust
match sipx_call::serve(&mut call, &mut incoming).await {
    Ok(()) => println!("the far end hung up"),
    Err(sipx_call::Error::SessionExpired) => println!("the far end stopped answering"),
    Err(error) => return Err(error.into()),
}
```

The interval is floored at ninety seconds, in both directions and regardless of configuration.
A shorter one is not a tighter check, it is a way for the far end to make you send requests as
fast as it likes (RFC 4028 §11.2).
