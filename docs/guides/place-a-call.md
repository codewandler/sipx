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
