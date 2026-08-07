---
title: Attended transfer
description: Consult first, then hand the caller over by replacing the consultation call — REFER with Replaces, and why the dialog tags are the security.
---

# Attended transfer

Where a [blind transfer](blind-transfer.md) says "call this number", an attended transfer says
"call this number, and when you get through, take the place of the call I already have with
them". The transferor speaks to the target first — checks they are there, warns them who is
coming — and the handover is seamless rather than a second ring.

The example below is the transferor's side, a real file that CI compiles
([`crates/sipx-call/examples/attended_transfer.rs`](https://github.com/codewandler/sipx/blob/main/crates/sipx-call/examples/attended_transfer.rs)):

<!-- BEGIN generated:example crates/sipx-call/examples/attended_transfer.rs -->
```rust
//! Consult a colleague first, then hand the caller over by replacing the consultation call.
//!
//! ```text
//! cargo run --example attended_transfer -- 192.0.2.1:5060 192.0.2.7:5060
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
    let mut args = std::env::args().skip(1);
    let customer_addr: SocketAddr = args
        .next()
        .unwrap_or_else(|| "127.0.0.1:5060".to_owned())
        .parse()?;
    let colleague_addr: SocketAddr = args
        .next()
        .unwrap_or_else(|| "127.0.0.1:5062".to_owned())
        .parse()?;

    // One endpoint, two calls: both dialogs' requests arrive on the same channel, and
    // `Call::handle` says which call each one belongs to.
    let (endpoint, mut incoming) = bind(Config::new("0.0.0.0:0".parse()?)).await?;
    let to = Uri::sip(Host::Name(HostName::new("callee.example")?));
    let options = DialOptions::new("<sip:alice@example.net>", "127.0.0.1".parse()?)
        .with_timeout(Duration::from_secs(20));

    let mut customer = dial(&endpoint, Target::udp(customer_addr), &to, &options).await?;
    println!("talking to the customer");

    // Park the customer while consulting — hold is a direction (see the hold-and-resume guide).
    customer.reinvite(Direction::SendOnly).await?;
    println!("customer on hold; consulting");

    let mut colleague = dial(&endpoint, Target::udp(colleague_addr), &to, &options).await?;
    println!("consulting the colleague");

    // The handover. The REFER goes to the *customer* — the call being transferred — and names
    // the colleague's dialog in a Replaces header (RFC 3891): "call where my other call goes,
    // and take that call's place". The customer's new INVITE replaces the consultation call
    // rather than ringing a second time.
    customer.refer_attended(&colleague).await?;
    println!("handover requested; waiting for the outcome");

    // Drive both calls until the customer's transfer resolves. The colleague leg sees its own
    // traffic during this: when the replacement call arrives there, the colleague ends the
    // consultation call with a BYE, which `handle` answers.
    while !customer
        .transfer()
        .is_some_and(|transfer| transfer.finished)
    {
        let Some(request) = incoming.recv().await else {
            break;
        };
        if customer.handle(&request).await? {
            continue;
        }
        colleague.handle(&request).await?;
    }

    if customer
        .transfer()
        .is_some_and(|transfer| matches!(transfer.state, sipx_call::TransferState::Succeeded))
    {
        // The customer and the colleague are talking. Only the customer leg is still ours to
        // end; the consultation call was replaced and ended by the colleague's BYE above.
        println!("transferred");
        customer.hang_up().await?;
    } else {
        // The handover did not happen. Everything is as it was: take the customer off hold and
        // decide what to try next.
        println!("the transfer did not complete; resuming the customer");
        customer.reinvite(Direction::SendRecv).await?;
        customer.hang_up().await?;
    }
    if !colleague.is_ended() {
        colleague.hang_up().await?;
    }
    Ok(())
}
```
<!-- END generated:example -->

Run it against two far ends — the caller being transferred and the colleague being consulted:

```bash
cargo run --example attended_transfer -- 192.0.2.1:5060 192.0.2.7:5060
```

## What is worth noticing

**Which call the method belongs to.** `customer.refer_attended(&colleague)` asks the *customer*
to move: the call the REFER is sent on is the one being transferred, and the other call names
where to go. Getting the two backwards asks your colleague to call your customer's phone.

**The tags are the security.** The `Replaces` header (RFC 3891) names the consultation dialog by
its Call-ID *and both tags*. The Call-ID is visible to every element on the signalling path, but
the tags are random and known only to the two parties — matching on Call-ID alone would let anyone
who ever saw one message of a call ask to be put in the middle of it. RFC 3891 §5 says as much,
and it is why `Replaces` has no form with fewer than three fields.

**The consultation call ends by being replaced.** When the customer's new INVITE reaches the
colleague carrying `Replaces`, the colleague answers it and sends BYE on the dialog it replaced —
the example's loop sees that BYE arrive on its own. Only the customer leg is the transferor's to
hang up, and only once the NOTIFYs report the transfer succeeded.

**Failure leaves everything as it was.** Both calls are still up: take the customer off hold, tell
them the colleague is unavailable, and try something else. The transfer having failed is a state
on the customer call, not an exception that tore anything down.

**Two calls share one endpoint.** Both dialogs' requests arrive on the same channel, and
[`Call::handle`](https://codewandler.github.io/sipx/api/sipx_call/call/struct.Call.html#method.handle)
returns whether the call claimed the message — which is all the routing a two-call program needs.
An application juggling many calls wants the
[`Dispatcher`](https://codewandler.github.io/sipx/api/sipx_call/dispatch/struct.Dispatcher.html)
instead.

## From the command line

The diagnostic actor's `transfer` command sends a blind REFER; composing an attended handover —
two calls, a hold, and a `Replaces` — is the library's job, and the example above is the shape of
it. See [Blind transfer](blind-transfer.md) for the scenario-driven form.
