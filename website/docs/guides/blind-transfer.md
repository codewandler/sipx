---
title: Blind transfer
description: Hand a call over with REFER and read the NOTIFYs that report what became of it — a 202 is not success.
---

# Blind transfer

A transfer has three parties: the **transferor** who hands the call over, the **transferee** who
is asked to call somewhere new, and the **target** who is called as a result. A blind transfer
says only "call this number" — the transferor does not stay to introduce anybody.

The example below is the transferor's side, a real file that CI compiles
([`crates/sipx-call/examples/blind_transfer.rs`](https://github.com/codewandler/sipx/blob/main/crates/sipx-call/examples/blind_transfer.rs)):

<!-- BEGIN generated:example crates/sipx-call/examples/blind_transfer.rs -->
```rust
//! Place a call, hand it over to somebody else, and learn what became of it.
//!
//! ```text
//! cargo run --example blind_transfer -- 192.0.2.1:5060 sip:carol@192.0.2.7:5060
//! ```

// These samples are read by people before they are run by machines, so they are written for
// readability where the workspace lints would prefer something terser. `clone_into` over
// `to_owned` teaches nothing in a five-line example, and a sine wave has to become an `i16`
// somewhere.
#![allow(clippy::assigning_clones, clippy::cast_possible_truncation)]

use std::net::SocketAddr;
use std::time::Duration;

use bytes::Bytes;
use sipx_call::{DialOptions, TransferState, dial};
use sipx_sip::{Host, HostName, Uri};
use sipx_transport::{Config, Target, bind};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let far_end: SocketAddr = args
        .next()
        .unwrap_or_else(|| "127.0.0.1:5060".to_owned())
        .parse()?;
    let target = args
        .next()
        .unwrap_or_else(|| "sip:carol@192.0.2.7:5060".to_owned());
    let target = Uri::parse(Bytes::from(target))?;

    let (endpoint, mut incoming) = bind(Config::new("0.0.0.0:0".parse()?)).await?;
    let to = Uri::sip(Host::Name(HostName::new("callee.example")?));
    let options = DialOptions::new("<sip:alice@example.net>", "127.0.0.1".parse()?)
        .with_timeout(Duration::from_secs(20));

    let mut call = dial(&endpoint, Target::udp(far_end), &to, &options).await?;
    println!("connected");

    // REFER asks the far end to call the target instead (RFC 3515). Success here means only
    // that the request was accepted — a 202 is "I will try", not "it worked". What actually
    // became of the attempt arrives later, as NOTIFY.
    call.refer(&target).await?;
    println!("the far end took the transfer on; waiting for the outcome");

    // Keep feeding the call until the transferee reports a final outcome and the implicit
    // subscription the REFER created has ended. The NOTIFYs arrive on the same channel as
    // everything else in the dialog.
    while !call.transfer().is_some_and(|transfer| transfer.finished) {
        let Some(request) = incoming.recv().await else {
            break;
        };
        call.handle(&request).await?;
    }

    match call.transfer().map(|transfer| transfer.state.clone()) {
        Some(TransferState::Succeeded) => {
            // The target answered. The handover is complete; ending this call is a policy
            // decision, and for a blind transfer the usual policy is to end it.
            println!("transferred; this leg is no longer needed");
            call.hang_up().await?;
        }
        Some(TransferState::Failed { status, reason }) => {
            // The call with the transferee is still up — the far end is still there, and the
            // application decides what to try next.
            println!("the transfer failed ({status} {reason}); the call is still up");
            call.hang_up().await?;
        }
        other => println!("the transfer ended unresolved: {other:?}"),
    }
    Ok(())
}
```
<!-- END generated:example -->

Run it against something that answers and honours REFER:

```bash
cargo run --example blind_transfer -- 192.0.2.1:5060 sip:carol@192.0.2.7:5060
```

## What is worth noticing

**A 202 means "I will try", and nothing more.** This is where transfer implementations go wrong.
`refer` returns once the transferee has accepted the *request* (RFC 3515); whether the resulting
call was answered, refused or rang out arrives afterwards, as NOTIFY, and shows up in
[`Call::transfer`](https://codewandler.github.io/sipx/api/sipx_call/call/struct.Call.html#method.transfer).
Reporting success at the 202 would tell a user their call was handed over when it may have gone
nowhere.

**The outcome is a state, not a boolean.** `TransferState::Failed` carries the status and reason
the target gave — 486 from a busy target and 603 from one that declined lead to different next
steps, and the transferor still holds a live call with the transferee either way.

**The subscription must end.** A REFER creates an implicit subscription (RFC 3515 §2.4.4), and
`Transfer::finished` is distinct from the state being final on purpose: a transferee may report a
final status and still owe the terminating NOTIFY that closes the subscription. The example waits
for `finished`, not merely for a final state.

**The transferee's half is three calls.** An incoming REFER surfaces as
[`Call::referral`](https://codewandler.github.io/sipx/api/sipx_call/call/struct.Call.html#method.referral)
and as `CallEvent::TransferRequested`;
[`accept_referral`](https://codewandler.github.io/sipx/api/sipx_call/call/struct.Call.html#method.accept_referral)
answers 202, places the call and sends the NOTIFYs, and
[`refuse_referral`](https://codewandler.github.io/sipx/api/sipx_call/call/struct.Call.html#method.refuse_referral)
answers with a status the transferor can act on. `Referral::referred_by` says who asked
(RFC 3892) — the only basis the transferee has for deciding whether to call a stranger on
somebody's say-so.

**Hanging up afterwards is policy, not protocol.** A transferor usually ends its call once the
transfer succeeds, but nothing in RFC 3515 requires it — the example makes that an explicit
decision rather than a side effect.

## From the command line

The diagnostic actor sends the same REFER:

```bash
sipx scenario <<'EOF'
{"id":"1","command":"dial","uri":"sip:bob@192.0.2.1:5060"}
{"id":"2","command":"transfer","target":"sip:carol@192.0.2.7:5060"}
{"id":"3","command":"hangup"}
{"id":"4","command":"shutdown"}
EOF
```

See the [CLI reference](../reference/cli.md) for the command set and event envelope. For the
consultation-first variant, see [Attended transfer](attended-transfer.md).
