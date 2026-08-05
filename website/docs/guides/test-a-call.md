---
title: Test a call
description: Establish real sipx Call values over an in-process SIP signalling path.
---

# Test a call

`sipx-testkit` drives the application API you use in production: `DialOptions`, `dial`, `answer`,
`Call::handle`, and `Call::events`. Its two ordinary transport handles are joined by a bounded
in-process SIP signalling path, so no signalling socket or peer process is needed. Call setup still
opens the ordinary RTP/RTCP ports owned by `sipx-call`; bypassing that negotiation would produce a
mock object rather than prove a real `Call`.

Until the next registry release, depend on the public package from `main`:

```toml
[dev-dependencies]
sipx-testkit = { git = "https://github.com/codewandler/sipx", branch = "main" }
sipx-call = { git = "https://github.com/codewandler/sipx", branch = "main" }
sipx-sip = { git = "https://github.com/codewandler/sipx", branch = "main" }
tokio = { version = "1", features = ["macros", "rt"] }
```

The example below is a real file that CI compiles
([`crates/sipx-testkit/examples/test_a_call.rs`](https://github.com/codewandler/sipx/blob/main/crates/sipx-testkit/examples/test_a_call.rs)):

<!-- BEGIN generated:example crates/sipx-testkit/examples/test_a_call.rs -->
```rust
//! Establish two application calls over in-process SIP signalling.
//!
//! ```text
//! cargo run -p sipx-testkit --example test_a_call
//! ```

use std::net::{IpAddr, Ipv4Addr};

use sipx_call::{CallEvent, DialOptions};
use sipx_sip::{Host, HostName, Uri};
use sipx_testkit::call::CallHarness;

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let loopback = IpAddr::V4(Ipv4Addr::LOCALHOST);
    let to = Uri::sip(Host::Name(HostName::new("callee.example")?));
    let options = DialOptions::new("sip:caller@example.net", loopback);
    let mut harness = CallHarness::new()?;

    let pending = harness.dial(to, options).await?;
    let mut established = pending.answer(loopback).await?;
    let mut originating_events = established
        .caller
        .events()
        .ok_or_else(|| std::io::Error::other("caller event stream was already taken"))?;
    let mut answering_events = established
        .callee
        .events()
        .ok_or_else(|| std::io::Error::other("callee event stream was already taken"))?;

    assert!(matches!(
        originating_events.recv().await,
        Some(CallEvent::Answered)
    ));
    assert!(matches!(
        answering_events.recv().await,
        Some(CallEvent::Answered)
    ));
    println!("dialog established; 200 OK and ACK crossed the in-process signalling path");
    Ok(())
}
```
<!-- END generated:example -->

`CallHarness::dial` returns a pending value scoped to that one invitation. Calling `answer` runs the
real answer path concurrently with the dial, waits for the 2xx, delivers the caller's ACK into the
answering `Call`, and returns both call objects. Seeing `CallEvent::Answered` on both event streams
therefore proves an established dialog rather than merely a transaction-level `200`.

For transaction retransmission and loss tests, use `TransactionHarness::new(seed, Faults)` with
`Link<Virtual>`. Virtual instants retain nanosecond precision, and `advance` processes link arrivals
and timer deadlines chronologically, so a single large advance is equivalent to smaller steps.

The harness installs no tracing subscriber. If the host does not install one, sipx library use is
silent. The [logging policy](../reference/logging.md) defines the levels when a host does opt in.
