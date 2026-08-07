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
