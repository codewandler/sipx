---
title: Answer a call
description: Accept an incoming call from Rust — what to advertise, who retransmits the 200, and why the BYE needs feeding to the call.
---

# Answer a call

The example below is a real file that CI compiles
([`crates/sipx-call/examples/answer_a_call.rs`](https://github.com/codewandler/sipx/blob/main/crates/sipx-call/examples/answer_a_call.rs)):

Create a binary package with this complete dependency table; every directly imported sipx crate is
pinned to the same exact prerelease:

<!-- BEGIN generated:answer-consumer-dependencies -->
```toml
[dependencies]
sipx-call = "=1.0.0-rc.5"
sipx-sip = "=1.0.0-rc.5"
sipx-transport = "=1.0.0-rc.5"
tokio = { version = "1", features = ["macros", "rt-multi-thread"] }
```
<!-- END generated:answer-consumer-dependencies -->

<!-- BEGIN generated:example crates/sipx-call/examples/answer_a_call.rs -->
```rust
//! Wait for a call, answer it, record what the caller says, and serve it until it ends.
//!
//! ```text
//! cargo run --example answer_a_call
//! ```

// These samples are read by people before they are run by machines, so they are written for
// readability where the workspace lints would prefer something terser. `clone_into` over
// `to_owned` teaches nothing in a five-line example, and a sine wave has to become an `i16`
// somewhere.
#![allow(clippy::assigning_clones, clippy::cast_possible_truncation)]

use std::time::Duration;

use sipx_call::{MediaAddress, answer_at, serve};
use sipx_sip::Method;
use sipx_transport::{Config, bind};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut config = Config::new("0.0.0.0:5060".parse()?);
    // Bound to every address, but advertise one reachable deployment address consistently in
    // Via, Contact and SDP rather than asking the far end to reply to 0.0.0.0.
    config.sent_by = "198.51.100.44".to_owned();

    let (endpoint, mut incoming) = bind(config).await?;
    println!("listening on {}", endpoint.local_addr());

    while let Some(request) = incoming.recv().await {
        if request.request.method != Method::Invite {
            continue;
        }

        let media = MediaAddress::new("198.51.100.44".parse()?).with_bind("0.0.0.0".parse()?);
        let mut call = answer_at(&endpoint, &request, media).await?;
        println!("answered over {:?}", request.transport);

        // Record until the caller goes quiet for half a second.
        let heard = call
            .media()
            .record_until_idle(Duration::from_millis(500))
            .await;
        println!("heard {} samples", heard.len());

        // Keep feeding in-dialog requests and timer deadlines to the call. In particular, this
        // answers a BYE and stops the media when the caller hangs up.
        serve(&mut call, &mut incoming).await?;
        println!("the call ended");
        break;
    }
    Ok(())
}
```
<!-- END generated:example -->

## What is worth noticing

**Binding to `0.0.0.0` leaves nothing sensible to advertise.** A far end told to reply to
`0.0.0.0` will not. Set `sent_by` explicitly whenever the signalling bind is unspecified, and
use `MediaAddress::with_bind` when the SDP address is not locally owned.

**`answer` handles the retransmission of the 200.** Over UDP a lost 200 means the caller gives
up while this side believes the call is established; sipx resends it until the ACK arrives.

**`serve` owns the complete call lifecycle.** A BYE arrives on the same `incoming` channel, and a
session timer also needs to wake when no message arrives. `serve` handles both until the call ends;
without that loop the call would not notice the far end had gone and its media could keep flowing.

For the fail-closed browser-audio composition, call `answer_with_policy_at` with
`MediaPolicy::browser_audio()` after receiving the INVITE on a WSS listener. The complete remote
offer is validated before the media port is bound or gathering starts. A native-browser-shaped
offer may include safe extra formats, one-section BUNDLE/mid, `ice-options:trickle` with its complete
candidate already present, and the conventional muxed port-9 RTCP placeholder; the answer retains
only the five supported required mappings. Incremental candidate trickling is not implemented.

## From the command line

```bash
sipx answer --local 0.0.0.0:5060 --advertise 198.51.100.44 \
  --play greeting.wav --record caller.wav --duration 30 --wait 60 --once
```

The diagnostic equivalent is `sipx answer --transport wss --tls-cert cert.pem --tls-key key.pem
--profile browser-audio --json`. In the terminal JSON, `media_state: "running"` appears only after
ICE nomination, fingerprint-verified DTLS, and atomic protected-media key installation.
