# Register against a PBX

```rust
{{#include ../../crates/sipx-ua/examples/register.rs}}
```

## What is worth noticing

**A registration is a lease, not a request.** The server decides how long it lasts, which is not
always what was asked for, and it has to be refreshed before it expires. `Lease::refresh_after`
is deliberately shorter than `granted` — refreshing at the moment of expiry is a race with the
network.

**Digest is answered with the strongest algorithm offered.** sipx does MD5 and SHA-256 and
prefers SHA-256 when the server offers it. The implementation is checked against the worked
example RFC 2617 publishes for itself rather than against sipx's own arithmetic.

**This is verified against Kamailio**, including the case that makes the success meaningful: a
wrong password is refused.

## From the command line

```sh
sipx register sip:alice@example.com --password '…'
```
