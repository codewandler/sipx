//! Wait for a call, answer it, record what the caller says, and serve it until it ends.
//!
//! ```text
//! cargo run --example answer_a_call
//! ```

// These samples are read by people before they are run by machines, so they are written for
// readability where the workspace lints would prefer something terser. `clone_into` over
// `to_owned` teaches nothing in a five-line example, and a sine wave has to become an `i16`
// somewhere.
#![allow(clippy::assigning_clones, clippy::cast_possible_truncation)]

use std::time::Duration;

use sipx_call::{MediaAddress, answer_at, serve};
use sipx_sip::Method;
use sipx_transport::{Config, bind};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut config = Config::new("0.0.0.0:5060".parse()?);
    // Bound to every address, but advertise one reachable deployment address consistently in
    // Via, Contact and SDP rather than asking the far end to reply to 0.0.0.0.
    config.sent_by = "198.51.100.44".to_owned();

    let (endpoint, mut incoming) = bind(config).await?;
    println!("listening on {}", endpoint.local_addr());

    while let Some(request) = incoming.recv().await {
        if request.request.method != Method::Invite {
            continue;
        }

        let media = MediaAddress::new("198.51.100.44".parse()?).with_bind("0.0.0.0".parse()?);
        let mut call = answer_at(&endpoint, &request, media).await?;
        println!("answered over {:?}", request.transport);

        // Record until the caller goes quiet for half a second.
        let heard = call
            .media()
            .record_until_idle(Duration::from_millis(500))
            .await;
        println!("heard {} samples", heard.len());

        // Keep feeding in-dialog requests and timer deadlines to the call. In particular, this
        // answers a BYE and stops the media when the caller hangs up.
        serve(&mut call, &mut incoming).await?;
        println!("the call ended");
        break;
    }
    Ok(())
}
