//! Answer a call, dial a second leg, and drive the pair as one call.
//!
//! ```text
//! cargo run --example couple_two_calls -- 192.0.2.7:5060
//! ```
//!
//! Dial the printed listening address from anywhere; the program relays the call onward to the
//! target address and bridges the audio between the two legs.

// These samples are read by people before they are run by machines, so they are written for
// readability where the workspace lints would prefer something terser. `clone_into` over
// `to_owned` teaches nothing in a five-line example, and a sine wave has to become an `i16`
// somewhere.
#![allow(clippy::assigning_clones, clippy::cast_possible_truncation)]

use std::net::SocketAddr;
use std::time::Duration;

use sipx_call::{DialOptions, Dispatched, Dispatcher, EarlyCoupling};
use sipx_sip::{Host, HostName, Uri};
use sipx_transport::{Config, Target, bind};
use tokio::sync::mpsc;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let target: SocketAddr = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "127.0.0.1:5062".to_owned())
        .parse()?;

    let (endpoint, incoming) = bind(Config::new("0.0.0.0:5060".parse()?)).await?;
    println!("listening on {}", endpoint.local_addr());

    // The dispatcher routes every incoming request to the call it belongs to, and surfaces the
    // ones that start something new. Routed inboxes fill only while `next` is being polled, so
    // it runs in a task of its own — the shape any multi-call program has.
    let mut dispatcher = Dispatcher::new(endpoint.clone(), incoming);
    let calls = dispatcher.calls();
    let (invitations, mut surfaced) = mpsc::channel(1);
    tokio::spawn(async move {
        while let Some(dispatched) = dispatcher.next().await {
            if let Dispatched::Invitation(invitation) = dispatched
                && invitations.send(invitation).await.is_err()
            {
                return;
            }
        }
    });

    let Some(invitation) = surfaced.recv().await else {
        return Err("the endpoint shut down before a call arrived".into());
    };
    println!("a call arrived; dialling the target leg");

    // The early coupling owns both legs from the first INVITE: the caller hears ringing once
    // the target rings, a caller that hangs up early becomes a CANCEL on the outbound INVITE,
    // and a target that refuses becomes the same status on the inbound one. Target selection —
    // this address — is the application's policy, not the coupling's.
    let to = Uri::sip(Host::Name(HostName::new("target.example")?));
    let options = DialOptions::new("<sip:edge@example.net>", "127.0.0.1".parse()?)
        .with_timeout(Duration::from_secs(20));
    // Boxed because a coupling holds two complete legs, which makes these futures large ones.
    let early = Box::pin(EarlyCoupling::dial(
        invitation,
        &calls,
        &endpoint,
        Target::udp(target),
        &to,
        &options,
        "127.0.0.1".parse()?,
    ))
    .await?;

    // Both legs confirm — or the whole attempt unwinds, with nobody left ringing at a dead peer.
    let confirmed = Box::pin(early.confirmed()).await?;
    let (mut coupling, mut one_incoming, mut two_incoming) = confirmed.into_parts();

    // Without this the coupling is signalling-only. Nothing is copied between the legs — each
    // has its own addresses, ports and keys — so the bridge says whether it is transcoding,
    // which happens exactly when the two legs negotiated different codecs.
    let transcoding = coupling.bridge_media();
    println!("bridged; transcoding: {transcoding}");

    // Drive both dialogs until either ends. A BYE on one leg is answered there and then mapped
    // to a BYE on the peer, so the two calls end together rather than one of them being left up.
    let end = coupling.run(&mut one_incoming, &mut two_incoming).await?;
    println!("the coupling ended: {end:?}");
    Ok(())
}
