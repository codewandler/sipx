//! Local port policy is retained across the public call entry points.
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::similar_names
)]
use sipx_call::{CallConfig, DialOptions, MediaAddress, answer_at, dial};
use sipx_media::{MediaPort, MediaPortRange};
use sipx_sip::{Host, HostName, Uri};
use sipx_transport::{Config, Target, bind};
use std::net::{IpAddr, Ipv4Addr};
fn ip() -> IpAddr {
    IpAddr::V4(Ipv4Addr::LOCALHOST)
}
async fn reservation() -> (MediaPort, MediaPortRange) {
    let port = MediaPort::bind((ip(), 0).into()).await.unwrap();
    assert!(port.has_control_port());
    let n = port.local_addr().port();
    let range = MediaPortRange::new(n, n + 1).unwrap();
    (port, range)
}
#[tokio::test]
async fn exhausted_media_range_sends_no_invite() {
    let (held, range) = reservation().await;
    let (client, _) = bind(Config::new((ip(), 0).into())).await.unwrap();
    let (peer, mut incoming) = bind(Config::new((ip(), 0).into())).await.unwrap();
    let options = DialOptions::new("<sip:caller@example.test>", ip()).with_media_port_range(range);
    let uri = Uri::sip(Host::Name(HostName::new("peer.test").unwrap()));
    let error = dial(&client, Target::udp(peer.local_addr()), &uri, &options)
        .await
        .unwrap_err();
    assert!(matches!(error,sipx_call::Error::Io(e) if e.kind()==std::io::ErrorKind::AddrInUse));
    assert!(incoming.try_recv().is_err());
    drop(held);
    client.shutdown().await;
    peer.shutdown().await;
}
#[tokio::test]
async fn call_config_and_answer_both_use_their_own_range() {
    let (caller_hold, caller_range) = reservation().await;
    let (callee_hold, callee_range) = reservation().await;
    let (client, _) = bind(Config::new((ip(), 0).into())).await.unwrap();
    let (peer, mut incoming) = bind(Config::new((ip(), 0).into())).await.unwrap();
    let options = DialOptions::new("<sip:caller@example.test>", ip()).with_call_config(
        CallConfig::new(MediaAddress::new(ip()).with_port_range(caller_range)),
    );
    let uri = Uri::sip(Host::Name(HostName::new("peer.test").unwrap()));
    drop((caller_hold, callee_hold));
    let (caller, callee) = tokio::join!(
        dial(&client, Target::udp(peer.local_addr()), &uri, &options),
        async {
            let invite = incoming.recv().await.unwrap();
            answer_at(
                &peer,
                &invite,
                MediaAddress::new(ip()).with_port_range(callee_range),
            )
            .await
        }
    );
    let caller = caller.unwrap();
    let callee = callee.unwrap();
    assert!(caller_range.contains(caller.media().local_addr().port()));
    assert!(callee_range.contains(callee.media().local_addr().port()));
    caller.media().shutdown().await;
    callee.media().shutdown().await;
    drop((caller, callee));
    client.shutdown().await;
    peer.shutdown().await;
}
