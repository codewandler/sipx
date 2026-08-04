//! The deployment-address choice is reachable through the shipped command.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::process::Stdio;
use std::time::Duration;

use sipx_sip::HeaderName;
use sipx_transport::{Config, bind};
use tokio::process::Command;

#[tokio::test]
async fn dial_advertise_reaches_via_contact_and_sdp_without_binding_the_public_ip() {
    let (peer, mut incoming) = bind(Config::new("127.0.0.1:0".parse().unwrap()))
        .await
        .expect("binds peer");
    let uri = format!("sip:bob@{}", peer.local_addr());

    let child = Command::new(env!("CARGO_BIN_EXE_sipx"))
        .args([
            "dial",
            &uri,
            "--local",
            "127.0.0.1:0",
            "--advertise",
            "198.51.100.44",
            "--timeout",
            "1",
            "--json",
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .expect("starts sipx");

    let invite = tokio::time::timeout(Duration::from_secs(5), incoming.recv())
        .await
        .expect("the command sends promptly")
        .expect("an INVITE")
        .request;
    let contact =
        String::from_utf8_lossy(&invite.headers.value(&HeaderName::Contact).unwrap()).into_owned();
    let via =
        String::from_utf8_lossy(&invite.headers.value(&HeaderName::Via).unwrap()).into_owned();
    let sdp = String::from_utf8_lossy(invite.body());

    assert!(contact.contains("198.51.100.44"), "{contact}");
    assert!(via.contains("198.51.100.44"), "{via}");
    assert!(sdp.contains("c=IN IP4 198.51.100.44"), "{sdp}");

    let output = tokio::time::timeout(Duration::from_secs(5), child.wait_with_output())
        .await
        .expect("the one-second attempt exits")
        .expect("waits for sipx");
    assert_eq!(output.status.code(), Some(5), "{output:?}");
}
