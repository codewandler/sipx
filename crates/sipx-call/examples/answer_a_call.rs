//! Wait for a call, answer it, record what the caller says, and hang up.
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

use sipx_call::answer;
use sipx_sip::Method;
use sipx_transport::{Config, bind};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let mut config = Config::new("0.0.0.0:5060".parse()?);
    // Bound to every address, so there is nothing sensible to advertise; say so explicitly
    // rather than letting the far end be told to reply to 0.0.0.0.
    config.sent_by = "127.0.0.1".to_owned();

    let (endpoint, mut incoming) = bind(config).await?;
    println!("listening on {}", endpoint.local_addr());

    while let Some(request) = incoming.recv().await {
        if request.request.method != Method::Invite {
            continue;
        }

        let call = answer(&endpoint, &request, "127.0.0.1".parse()?).await?;
        println!("answered over {:?}", request.transport);

        // Record until the caller goes quiet for half a second.
        let heard = call
            .media()
            .record_until_idle(Duration::from_millis(500))
            .await;
        println!("heard {} samples", heard.len());
        break;
    }
    Ok(())
}
