---
title: Answer a call
description: Accept an incoming call from Rust — what to advertise, who retransmits the 200, and why the BYE needs feeding to the call.
---

# Answer a call

The example below is a real file that CI compiles
([`crates/sipx-call/examples/answer_a_call.rs`](https://github.com/codewandler/sipx/blob/main/crates/sipx-call/examples/answer_a_call.rs)):

<!-- BEGIN generated:example crates/sipx-call/examples/answer_a_call.rs -->
```rust
//! Wait for a call, answer it, record what the caller says, and hang up.
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

use sipx_call::answer;
use sipx_sip::Method;
use sipx_transport::{Config, bind};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut config = Config::new("0.0.0.0:5060".parse()?);
    // Bound to every address, so there is nothing sensible to advertise; say so explicitly
    // rather than letting the far end be told to reply to 0.0.0.0.
    config.sent_by = "127.0.0.1".to_owned();

    let (endpoint, mut incoming) = bind(config).await?;
    println!("listening on {}", endpoint.local_addr());

    while let Some(request) = incoming.recv().await {
        if request.request.method != Method::Invite {
            continue;
        }

        let call = answer(&endpoint, &request, "127.0.0.1".parse()?).await?;
        println!("answered over {:?}", request.transport);

        // Record until the caller goes quiet for half a second.
        let heard = call
            .media()
            .record_until_idle(Duration::from_millis(500))
            .await;
        println!("heard {} samples", heard.len());
        break;
    }
    Ok(())
}
```
<!-- END generated:example -->

## What is worth noticing

**Binding to `0.0.0.0` leaves nothing sensible to advertise.** A far end told to reply to
`0.0.0.0` will not. Set `sent_by` explicitly whenever the bind address is unspecified.

**`answer` handles the retransmission of the 200.** Over UDP a lost 200 means the caller gives
up while this side believes the call is established; sipx resends it until the ACK arrives.

**In-dialog requests need feeding to the call.** A BYE arrives on the same `incoming` channel;
pass it to `Call::handle` or the call will not notice it has ended and the media will keep
flowing.

## From the command line

```bash
sipx answer --play greeting.wav --record caller.wav --duration 30
```
