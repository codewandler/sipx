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
