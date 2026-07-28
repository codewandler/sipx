---
title: Register against a PBX
description: Registration as a lease — refresh before expiry, answer digest with the strongest algorithm offered, and the refusal cases that make it trustworthy.
---

# Register against a PBX

The example below is a real file that CI compiles
([`crates/sipx-ua/examples/register.rs`](https://github.com/codewandler/sipx/blob/main/crates/sipx-ua/examples/register.rs)):

<!-- BEGIN generated:example crates/sipx-ua/examples/register.rs -->
```rust
//! Register against a PBX, and keep the registration alive.
//!
//! ```text
//! cargo run --example register -- sip:alice@example.com 192.0.2.1:5060 secret
//! ```

// These samples are read by people before they are run by machines, so they are written for
// readability where the workspace lints would prefer something terser. `clone_into` over
// `to_owned` teaches nothing in a five-line example, and a sine wave has to become an `i16`
// somewhere.
#![allow(clippy::assigning_clones, clippy::cast_possible_truncation)]

use std::net::SocketAddr;

use sipx_sip::{Host, HostName, Uri};
use sipx_transport::{Config as TransportConfig, Target, bind};
use sipx_ua::{Config, Credentials, UserAgent};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut args = std::env::args().skip(1);
    let user = args
        .next()
        .unwrap_or_else(|| "sip:alice@example.com".to_owned());
    let server: SocketAddr = args
        .next()
        .unwrap_or_else(|| "127.0.0.1:5060".to_owned())
        .parse()?;
    let password = args.next().unwrap_or_else(|| "secret".to_owned());

    let (endpoint, _incoming) = bind(TransportConfig::new("0.0.0.0:0".parse()?)).await?;

    let registrar = Uri::sip(Host::Name(HostName::new("example.com")?));
    let config = Config::new(
        format!("<{user}>"),
        format!("<sip:alice@{}>", endpoint.local_addr()),
        registrar,
        Target::udp(server),
    )
    .with_credentials(Credentials::new("alice", password));

    let mut agent = UserAgent::new(endpoint, config);

    // A registration is a *lease*, not a request: the server decides how long it lasts, which is
    // not always what was asked for, and it has to be refreshed before it expires.
    let lease = agent.register().await?;
    println!(
        "registered for {:?}; refresh after {:?}",
        lease.granted, lease.refresh_after
    );
    Ok(())
}
```
<!-- END generated:example -->

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

```bash
sipx register sip:alice@example.com --password '…'
```
