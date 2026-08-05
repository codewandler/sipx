---
title: Register against a PBX
description: Register an endpoint, handle digest authentication, and refresh the granted lease before it expires.
---

# Register against a PBX

A registrar maps an address of record such as `sip:alice@example.com` to the endpoint where
calls should arrive. The binding expires unless the endpoint refreshes it.

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

**Treat registration as a lease.** The registrar decides the granted lifetime, which can differ
from the requested value. `Lease::refresh_after` is deliberately earlier than `Lease::granted`,
because refreshing at expiry races the network. The example performs one registration; a
long-running application should call `UserAgent::keep_registered` or schedule refreshes from the
returned lease.

**Digest selection is deterministic.** sipx supports MD5 and the 256-bit and 512/256-bit SHA-2
variants, including their `-sess` forms. It selects the strongest offered algorithm as required
by RFC 8760, with the registrar's order breaking equal-strength ties. Authentication failures
are returned as typed errors rather than accepted as a registration.

**Advertise a reachable contact.** The example binds an ephemeral local socket and uses its
address in `Contact`, which is suitable for a controlled local setup. A deployed endpoint must
advertise an address the registrar can route back to. RFC 5626 Outbound can keep requests on a
client-opened flow when direct inbound reachability is unavailable.

**Keep path observation separate from reachability policy.** Before the first success,
`UserAgent::registration_observation` is `NotRegistered`. Afterwards it reports what the registrar
put in the final response's top `Via`: `Observed(address)`, `Absent`, or `Invalid(reason)`. The
convenience `observed_registration_address` returns an address only for the first case. This is
useful for diagnostics and NAT visibility, but it is not proof that an inbound path exists. sipx
never copies the value into a later `Contact`, route set, GRUU, Outbound or push state, SDP, ICE
candidate, or media destination. Missing or malformed observation data also does not invalidate the
registrar's lease. See the [registration observation specification](https://github.com/codewandler/sipx/blob/main/docs/specs/registration-observation.md)
for the complete typed outcome table.

## From the command line

```bash
SIPX_PASSWORD='your-password' \
  sipx register sip:alice@example.com --keep-alive
```

Prefer `SIPX_PASSWORD` to `--password`, because command-line arguments may be visible to other
local processes. Useful options include `--target host:port` to bypass discovery,
`--transport tls` or `--transport wss` for protected signalling, and `--outbound` for an RFC 5626
flow. Private authorities are added with `--tls-ca`; certificate verification cannot be disabled.

See the [CLI reference](../reference/cli.md) for push-refresh options, capture handling, output
fields, and exit codes.
