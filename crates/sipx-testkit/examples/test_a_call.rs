//! Establish two application calls over in-process SIP signalling.
//!
//! ```text
//! cargo run -p sipx-testkit --example test_a_call
//! ```

use std::net::{IpAddr, Ipv4Addr};

use sipx_call::{CallEvent, DialOptions};
use sipx_sip::{Host, HostName, Uri};
use sipx_testkit::call::CallHarness;

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let loopback = IpAddr::V4(Ipv4Addr::LOCALHOST);
    let to = Uri::sip(Host::Name(HostName::new("callee.example")?));
    let options = DialOptions::new("sip:caller@example.net", loopback);
    let mut harness = CallHarness::new()?;

    let pending = harness.dial(to, options).await?;
    let mut established = Box::pin(pending.answer(loopback)).await?;
    let mut originating_events = established
        .caller
        .events()
        .ok_or_else(|| std::io::Error::other("caller event stream was already taken"))?;
    let mut answering_events = established
        .callee
        .events()
        .ok_or_else(|| std::io::Error::other("callee event stream was already taken"))?;

    assert!(matches!(
        originating_events.recv().await,
        Some(CallEvent::Answered)
    ));
    assert!(matches!(
        answering_events.recv().await,
        Some(CallEvent::Answered)
    ));
    println!("dialog established; 200 OK and ACK crossed the in-process signalling path");
    Ok(())
}
