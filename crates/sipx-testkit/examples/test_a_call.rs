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
