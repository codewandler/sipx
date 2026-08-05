---
title: Test a call
description: Place and answer SIP signalling in one process with deterministic virtual time and no sockets.
---

# Test a call

`sipx-testkit` exposes the same seeded in-process link used by the workspace's own transaction
tests. The call harness joins two real transaction layers over that link. It opens no socket, starts
no runtime and sleeps for no wall-clock duration.

Until the next registry release, depend on the public package from `main`:

```toml
[dev-dependencies]
sipx-testkit = { git = "https://github.com/codewandler/sipx", branch = "main" }
sipx-sip = { git = "https://github.com/codewandler/sipx", branch = "main" }
bytes = "1"
```

The example below is a real file that CI compiles
([`crates/sipx-testkit/examples/test_a_call.rs`](https://github.com/codewandler/sipx/blob/main/crates/sipx-testkit/examples/test_a_call.rs)):

<!-- BEGIN generated:example crates/sipx-testkit/examples/test_a_call.rs -->
```rust
//! Place and answer a SIP call in a test process, without sockets or sleeping.
//!
//! ```text
//! cargo run -p sipx-testkit --example test_a_call
//! ```

use bytes::Bytes;
use sipx_sip::build::RequestBuilder;
use sipx_sip::{HeaderName, Host, HostName, Method, Uri};
use sipx_testkit::call::CallHarness;

fn invite() -> Result<sipx_sip::Request, Box<dyn std::error::Error>> {
    let request = RequestBuilder::new(
        Method::Invite,
        Uri::sip(Host::Name(HostName::new("callee.example")?)),
    )
    .header(
        HeaderName::Via,
        Bytes::from_static(b"SIP/2.0/UDP caller.example;branch=z9hG4bK-example"),
    )?
    .header(HeaderName::To, Bytes::from_static(b"<sip:callee.example>"))?
    .header(
        HeaderName::From,
        Bytes::from_static(b"<sip:caller@example.net>;tag=caller"),
    )?
    .header(
        HeaderName::CallId,
        Bytes::from_static(b"example@example.net"),
    )?
    .cseq(1, &Method::Invite)?
    .max_forwards(70)
    .build();
    Ok(request)
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut call = CallHarness::perfect();

    call.place(invite()?)?;
    let received = call
        .invitation()
        .ok_or_else(|| std::io::Error::other("the invitation did not arrive"))?;
    assert_eq!(received.method, Method::Invite);

    call.answer_ok()?;
    let response = call
        .response()
        .ok_or_else(|| std::io::Error::other("the answer did not arrive"))?;
    assert_eq!(response.status.code(), 200);
    assert_eq!(call.now().millis(), 0);

    println!("answered at virtual millisecond {}", call.now().millis());
    Ok(())
}
```
<!-- END generated:example -->

`CallHarness::perfect` is the shortest happy path. Use `CallHarness::new` with `Faults` to make
loss, duplication, latency and jitter reproducible from a seed, then move time explicitly with
`advance`. The harness covers an INVITE reaching the answering transaction user and its response
reaching the caller. It deliberately does not emulate media or claim network interoperability;
keep those as bounded integration tests.

The harness installs no tracing subscriber. If the host does not install one, sipx library use is
silent. The [logging policy](../reference/logging.md) defines the levels when a host does opt in.
