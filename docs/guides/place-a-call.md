# Place a call

```rust
{{#include ../../crates/sipx-call/examples/place_a_call.rs}}
```

Run it against something that answers:

```sh
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

```sh
sipx dial sip:bob@192.0.2.1:5060 --play hello.wav --record reply.wav --timeout 20
```

Every command speaks `--json` and returns a distinct exit code per outcome.

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
*no* traffic arrived. [`serve`](../api/sipx_call/call/fn.serve.html) is the loop that handles both:

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
