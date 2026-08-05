//! Confirmed-dialog persistence across fresh signalling and media drivers (story `S-43`).
//!
//! These are the public-boundary vectors. Byte-shape and hostile-prefix vectors live beside the
//! codec; this file proves that the restored value can drive the next real in-dialog exchange.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
#![allow(clippy::similar_names)]

use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use sipx_call::{
    Call, DialOptions, DialogNotQuiescent, DialogPersistenceError, DialogRestoreContext, IcePolicy,
    MediaAddress, MediaPolicy, answer, answer_with_policy, dial,
};
use sipx_media::{Config as MediaConfig, MediaSession};
use sipx_sdp::Direction;
use sipx_sip::build::ResponseBuilder;
use sipx_sip::{CSeq, HeaderName, Host, HostName, Method, StatusCode, Uri};
use sipx_transport::{Config, Handle, Incoming, Target, bind};
use tokio::sync::mpsc::Receiver;
use tokio::time::Instant;

fn loopback() -> IpAddr {
    "127.0.0.1".parse().expect("valid loopback")
}

async fn endpoint() -> (Handle, Receiver<Incoming>) {
    bind(Config::new("127.0.0.1:0".parse().expect("valid endpoint")))
        .await
        .expect("binds")
}

fn callee_uri() -> Uri {
    Uri::sip(Host::Name(HostName::new("callee.example").expect("valid")))
}

struct Connected {
    caller: Call,
    callee: Call,
    caller_endpoint: Handle,
    callee_endpoint: Handle,
    callee_incoming: Receiver<Incoming>,
}

async fn connected() -> Connected {
    connected_with(DialOptions::new("<sip:caller@example.net>", loopback())).await
}

async fn connected_with(options: DialOptions) -> Connected {
    let (callee_endpoint, mut callee_incoming) = endpoint().await;
    let (caller_endpoint, _caller_incoming) = endpoint().await;
    let callee_addr = callee_endpoint.local_addr();
    let answer_handle = callee_endpoint.clone();

    let answering = tokio::spawn(async move {
        let incoming = callee_incoming.recv().await.expect("INVITE arrives");
        let call = answer(&answer_handle, &incoming, loopback())
            .await
            .expect("answers");
        (call, callee_incoming)
    });

    let caller = dial(
        &caller_endpoint,
        Target::udp(callee_addr),
        &callee_uri(),
        &options,
    )
    .await
    .expect("connects");
    let (callee, callee_incoming) = answering.await.expect("answer task");
    Connected {
        caller,
        callee,
        caller_endpoint,
        callee_endpoint,
        callee_incoming,
    }
}

async fn connected_with_media(policy: MediaPolicy) -> Connected {
    let (callee_endpoint, mut callee_incoming) = endpoint().await;
    let (caller_endpoint, _caller_incoming) = endpoint().await;
    let callee_addr = callee_endpoint.local_addr();
    let answer_handle = callee_endpoint.clone();
    let answering = tokio::spawn(async move {
        let incoming = callee_incoming.recv().await.expect("INVITE arrives");
        let call = answer_with_policy(&answer_handle, &incoming, loopback(), policy)
            .await
            .expect("answers");
        (call, callee_incoming)
    });
    let caller = dial(
        &caller_endpoint,
        Target::udp(callee_addr),
        &callee_uri(),
        &DialOptions::new("<sip:caller@example.net>", loopback()).with_media_policy(policy),
    )
    .await
    .expect("connects");
    let (callee, callee_incoming) = answering.await.expect("answer task");
    Connected {
        caller,
        callee,
        caller_endpoint,
        callee_endpoint,
        callee_incoming,
    }
}

async fn fresh_media(remote: SocketAddr) -> Arc<MediaSession> {
    let mut config = MediaConfig::new(remote, sipx_media::Codec::Pcmu);
    config.rtcp_mode = sipx_sdp::RtcpMode::Mux;
    Arc::new(
        MediaSession::start(SocketAddr::new(loopback(), 0), config)
            .await
            .expect("fresh media"),
    )
}

/// DP-1 and DP-9's failing-first public proof: exact durable bytes reconstruct a call on fresh
/// drivers and the next request advances the saved local sequence number exactly once.
#[tokio::test]
async fn restored_dialog_drives_the_next_monotonic_in_dialog_exchange() {
    let Connected {
        mut caller,
        mut callee,
        caller_endpoint,
        callee_endpoint,
        mut callee_incoming,
    } = connected().await;

    caller.dialog.route_set = vec![
        "<sip:first.example;lr>".to_owned(),
        "<sip:second.example;lr>".to_owned(),
    ];
    let saved_cseq = caller.dialog.local_cseq;
    let saved_target = caller.dialog.remote_target.to_bytes();
    let captured = caller
        .dialog_snapshot(Instant::now())
        .expect("quiescent call snapshots");
    let bytes = captured.encode();
    let decoded = sipx_call::DialogSnapshot::decode(&bytes).expect("canonical snapshot decodes");
    assert_eq!(decoded.encode(), bytes, "encoding is canonical");
    drop(caller);

    let (fresh_endpoint, _fresh_incoming) = endpoint().await;
    let fresh_media = fresh_media(callee.media().local_addr()).await;
    let context = DialogRestoreContext::new(
        fresh_endpoint.clone(),
        Target::udp(callee_endpoint.local_addr()),
        fresh_media,
        MediaAddress::new(loopback()),
        callee.media().local_addr(),
        MediaPolicy::default().with_keying(sipx_call::Keying::Plain),
        decoded.direction(),
        Duration::ZERO,
        Instant::now(),
    );
    let mut restored = Call::restore_dialog(&decoded, &context).expect("restores on fresh drivers");
    assert!(matches!(
        Call::restore_dialog(&decoded, &context),
        Err(DialogPersistenceError::ContextAlreadyAttached)
    ));
    assert_eq!(restored.dialog.local_cseq, saved_cseq);
    assert_eq!(restored.dialog.remote_target.to_bytes(), saved_target);
    assert_eq!(
        restored.dialog.route_set,
        ["<sip:first.example;lr>", "<sip:second.example;lr>",]
    );

    let peer = tokio::spawn(async move {
        let request = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let request = callee_incoming
                    .recv()
                    .await
                    .expect("in-dialog request arrives");
                if request.request.method == Method::Invite {
                    break request;
                }
                assert!(
                    callee
                        .handle(&request)
                        .await
                        .expect("handles setup residue")
                );
            }
        })
        .await
        .expect("request arrival is bounded");
        assert_eq!(request.request.method, Method::Invite);
        assert_eq!(
            request
                .request
                .headers
                .typed::<CSeq>()
                .expect("CSeq exists")
                .expect("valid CSeq")
                .sequence,
            saved_cseq + 1,
        );
        let routes: Vec<_> = request
            .request
            .headers
            .get_all(&HeaderName::Route)
            .map(|header| String::from_utf8_lossy(&header.value()).into_owned())
            .collect();
        assert_eq!(
            routes,
            ["<sip:first.example;lr>", "<sip:second.example;lr>",]
        );
        assert!(callee.handle(&request).await.expect("handles re-INVITE"));
        callee
    });
    restored
        .reinvite(Direction::SendRecv)
        .await
        .expect("next request completes");
    assert_eq!(restored.dialog.local_cseq, saved_cseq + 1);
    let callee = peer.await.expect("peer task");

    drop(restored);
    drop(callee);
    fresh_endpoint.shutdown().await;
    caller_endpoint.shutdown().await;
    callee_endpoint.shutdown().await;
}

/// DP-5: changing only the durable security fact cannot attach it to clear signalling, and the
/// borrowed fresh context remains alive and transaction-free after refusal.
#[tokio::test]
async fn a_secure_snapshot_refuses_clear_restoration_without_runtime_side_effects() {
    let Connected {
        caller,
        callee,
        caller_endpoint,
        callee_endpoint,
        ..
    } = connected().await;
    let mut bytes = caller
        .dialog_snapshot(Instant::now())
        .expect("snapshots")
        .encode();
    bytes[7] |= 0b0000_0010;
    let protected = sipx_call::DialogSnapshot::decode(&bytes).expect("protected fact is valid");

    let (fresh_endpoint, _fresh_incoming) = endpoint().await;
    let media = fresh_media(callee.media().local_addr()).await;
    let context = DialogRestoreContext::new(
        fresh_endpoint.clone(),
        Target::udp(callee_endpoint.local_addr()),
        Arc::clone(&media),
        MediaAddress::new(loopback()),
        callee.media().local_addr(),
        MediaPolicy::default().with_keying(sipx_call::Keying::Plain),
        protected.direction(),
        Duration::ZERO,
        Instant::now(),
    );
    assert_eq!(fresh_endpoint.outstanding().await.expect("counts"), 0);
    assert!(matches!(
        Call::restore_dialog(&protected, &context),
        Err(DialogPersistenceError::SecurityDowngrade)
    ));
    assert_eq!(fresh_endpoint.outstanding().await.expect("counts"), 0);
    assert_eq!(
        Arc::strong_count(&media),
        2,
        "failed restore did not consume media"
    );

    drop(context);
    drop(media);
    drop(caller);
    drop(callee);
    fresh_endpoint.shutdown().await;
    caller_endpoint.shutdown().await;
    callee_endpoint.shutdown().await;
}

/// DP-6: a fresh host cannot attach a runtime direction that contradicts the durable media
/// contract, and the refusal happens before the one-owner claim.
#[tokio::test]
async fn a_mismatched_injected_direction_is_refused_before_context_claim() {
    let Connected {
        caller,
        callee,
        caller_endpoint,
        callee_endpoint,
        ..
    } = connected().await;
    let snapshot = caller
        .dialog_snapshot(Instant::now())
        .expect("quiescent call snapshots");
    assert_eq!(snapshot.direction(), Direction::SendRecv);

    let (fresh_endpoint, _fresh_incoming) = endpoint().await;
    let media = fresh_media(callee.media().local_addr()).await;
    let context = DialogRestoreContext::new(
        fresh_endpoint.clone(),
        Target::udp(callee_endpoint.local_addr()),
        media,
        MediaAddress::new(loopback()),
        callee.media().local_addr(),
        MediaPolicy::default().with_keying(sipx_call::Keying::Plain),
        Direction::RecvOnly,
        Duration::ZERO,
        Instant::now(),
    );
    for _ in 0..2 {
        assert!(matches!(
            Call::restore_dialog(&snapshot, &context),
            Err(DialogPersistenceError::MediaContractMismatch { field: "direction" })
        ));
    }

    drop(context);
    drop(caller);
    drop(callee);
    fresh_endpoint.shutdown().await;
    caller_endpoint.shutdown().await;
    callee_endpoint.shutdown().await;
}

/// DP-7: only a positive remaining duration becomes a fresh deadline; zero is an explicit fired
/// action and a value above the negotiated interval is contradictory rather than renewed.
#[tokio::test]
async fn session_time_is_rebased_from_explicit_now_and_never_silently_renewed() {
    let Connected {
        caller,
        callee,
        caller_endpoint,
        callee_endpoint,
        ..
    } = connected_with(
        DialOptions::new("<sip:caller@example.net>", loopback())
            .with_session_timer(Duration::from_secs(600)),
    )
    .await;
    let snapshot = caller
        .dialog_snapshot(Instant::now())
        .expect("timed call snapshots");
    let (interval, we_refresh, remaining) = snapshot.session_timer().expect("timer retained");
    assert_eq!(interval, Duration::from_secs(600));
    assert!(remaining > Duration::ZERO && remaining <= interval);

    let (fresh_endpoint, _fresh_incoming) = endpoint().await;
    let media = fresh_media(callee.media().local_addr()).await;
    let now = Instant::now();
    let elapsed_since_capture = Duration::from_secs(30);
    assert!(elapsed_since_capture < remaining);
    let context = DialogRestoreContext::new(
        fresh_endpoint.clone(),
        Target::udp(callee_endpoint.local_addr()),
        media,
        MediaAddress::new(loopback()),
        callee.media().local_addr(),
        MediaPolicy::default().with_keying(sipx_call::Keying::Plain),
        snapshot.direction(),
        elapsed_since_capture,
        now,
    );
    let restored = Call::restore_dialog(&snapshot, &context).expect("positive remainder restores");
    assert_eq!(restored.session_interval(), Some((interval, we_refresh)));
    assert_eq!(
        restored.session_deadline(),
        Some(
            now + remaining
                .checked_sub(elapsed_since_capture)
                .expect("elapsed time is below the remainder"),
        )
    );
    drop(restored);

    for elapsed in [
        remaining,
        remaining
            .checked_add(Duration::from_nanos(1))
            .expect("test duration fits"),
    ] {
        let expired_context = DialogRestoreContext::new(
            fresh_endpoint.clone(),
            Target::udp(callee_endpoint.local_addr()),
            fresh_media(callee.media().local_addr()).await,
            MediaAddress::new(loopback()),
            callee.media().local_addr(),
            MediaPolicy::default().with_keying(sipx_call::Keying::Plain),
            snapshot.direction(),
            elapsed,
            Instant::now(),
        );
        for _ in 0..2 {
            assert!(matches!(
                Call::restore_dialog(&snapshot, &expired_context),
                Err(DialogPersistenceError::SessionActionDue(_))
            ));
        }
    }

    let mut due_bytes = snapshot.encode();
    let remaining_at = due_bytes.len() - 8;
    due_bytes[remaining_at..].copy_from_slice(&0u64.to_be_bytes());
    let due = sipx_call::DialogSnapshot::decode(&due_bytes).expect("zero is a due action");
    assert!(matches!(
        Call::restore_dialog(&due, &context),
        Err(DialogPersistenceError::SessionActionDue(_))
    ));

    let mut contradictory = snapshot.encode();
    let remaining_at = contradictory.len() - 8;
    let too_long =
        u64::try_from((interval + Duration::from_nanos(1)).as_nanos()).expect("test duration fits");
    contradictory[remaining_at..].copy_from_slice(&too_long.to_be_bytes());
    assert!(matches!(
        sipx_call::DialogSnapshot::decode(&contradictory),
        Err(DialogPersistenceError::SessionContradiction)
    ));

    drop(context);
    drop(caller);
    drop(callee);
    fresh_endpoint.shutdown().await;
    caller_endpoint.shutdown().await;
    callee_endpoint.shutdown().await;
}

#[tokio::test]
async fn capture_refuses_every_runtime_owned_non_quiescent_state() {
    let Connected {
        mut caller,
        mut callee,
        caller_endpoint,
        callee_endpoint,
        mut callee_incoming,
    } = connected().await;

    assert!(matches!(
        callee.dialog_snapshot(Instant::now()),
        Err(DialogPersistenceError::NotQuiescent(
            DialogNotQuiescent::AwaitingAck
        ))
    ));
    let ack = callee_incoming.recv().await.expect("setup ACK arrives");
    assert_eq!(ack.request.method, Method::Ack);
    assert!(callee.handle(&ack).await.expect("handles ACK"));

    let mut offering = Box::pin(caller.reinvite(Direction::SendRecv));
    let offered = loop {
        tokio::select! {
            request = callee_incoming.recv() => {
                let request = request.expect("in-dialog request");
                if request.request.method == Method::Invite {
                    break request;
                }
                assert!(callee.handle(&request).await.expect("handles residue"));
            }
            result = &mut offering => panic!("offer completed without a peer response: {result:?}"),
        }
    };
    drop(offering);
    assert!(matches!(
        caller.dialog_snapshot(Instant::now()),
        Err(DialogPersistenceError::NotQuiescent(
            DialogNotQuiescent::OfferAnswer
        ))
    ));
    drop(offered);

    drop(caller);
    drop(callee);
    caller_endpoint.shutdown().await;
    callee_endpoint.shutdown().await;

    let Connected {
        caller,
        callee,
        caller_endpoint,
        callee_endpoint,
        ..
    } = connected_with_media(MediaPolicy::default().with_ice(IcePolicy::Host)).await;
    assert!(matches!(
        caller.dialog_snapshot(Instant::now()),
        Err(DialogPersistenceError::NotQuiescent(
            DialogNotQuiescent::Ice
        ))
    ));
    assert!(matches!(
        callee.dialog_snapshot(Instant::now()),
        Err(DialogPersistenceError::NotQuiescent(_))
    ));
    drop(caller);
    drop(callee);
    caller_endpoint.shutdown().await;
    callee_endpoint.shutdown().await;
}

#[tokio::test]
async fn capture_refuses_both_sides_of_a_live_transfer_usage() {
    let Connected {
        mut caller,
        mut callee,
        caller_endpoint,
        callee_endpoint,
        mut callee_incoming,
    } = connected().await;
    let refer_target =
        Uri::parse(bytes::Bytes::from_static(b"sip:target@example.net")).expect("target URI");

    let referring = caller.refer(&refer_target);
    let accepting = async {
        loop {
            let incoming = callee_incoming.recv().await.expect("in-dialog request");
            let is_refer = incoming.request.method == Method::Refer;
            assert!(callee.handle(&incoming).await.expect("handles request"));
            if is_refer {
                let accepted = ResponseBuilder::to_request(
                    &incoming.request,
                    StatusCode::new(202).expect("valid status"),
                    "Accepted",
                )
                .expect("response builder")
                .build();
                callee_endpoint
                    .respond(&incoming.key, accepted)
                    .await
                    .expect("responds");
                break;
            }
        }
    };
    let (referred, ()) = tokio::join!(referring, accepting);
    referred.expect("REFER accepted");
    assert!(matches!(
        caller.dialog_snapshot(Instant::now()),
        Err(DialogPersistenceError::NotQuiescent(
            DialogNotQuiescent::Transfer
        ))
    ));
    assert!(matches!(
        callee.dialog_snapshot(Instant::now()),
        Err(DialogPersistenceError::NotQuiescent(
            DialogNotQuiescent::Transfer
        ))
    ));

    drop(caller);
    drop(callee);
    caller_endpoint.shutdown().await;
    callee_endpoint.shutdown().await;
}

#[tokio::test]
async fn an_ended_dialog_is_not_revived_as_a_snapshot() {
    let Connected {
        mut caller,
        mut callee,
        caller_endpoint,
        callee_endpoint,
        mut callee_incoming,
    } = connected().await;
    let ending_peer = async {
        loop {
            let incoming = callee_incoming.recv().await.expect("in-dialog request");
            let is_bye = incoming.request.method == Method::Bye;
            assert!(callee.handle(&incoming).await.expect("handles request"));
            if is_bye {
                break;
            }
        }
    };
    let (ended, ()) = tokio::join!(caller.hang_up(), ending_peer);
    ended.expect("hangs up");
    for call in [&caller, &callee] {
        assert!(matches!(
            call.dialog_snapshot(Instant::now()),
            Err(DialogPersistenceError::NotQuiescent(
                DialogNotQuiescent::Ended
            ))
        ));
    }
    drop(caller);
    drop(callee);
    caller_endpoint.shutdown().await;
    callee_endpoint.shutdown().await;
}
