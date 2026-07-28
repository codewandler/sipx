# Answer a call

```rust
{{#include ../../crates/sipx-call/examples/answer_a_call.rs}}
```

## What is worth noticing

**Binding to `0.0.0.0` leaves nothing sensible to advertise.** A far end told to reply to
`0.0.0.0` will not. Set `sent_by` explicitly whenever the bind address is unspecified.

**`answer` handles the retransmission of the 200.** Over UDP a lost 200 means the caller gives
up while this side believes the call is established; sipx resends it until the ACK arrives.

**In-dialog requests need feeding to the call.** A BYE arrives on the same `incoming` channel;
pass it to `Call::handle` or the call will not notice it has ended and the media will keep
flowing.

## From the command line

```sh
sipx answer --play greeting.wav --record caller.wav --duration 30
```
