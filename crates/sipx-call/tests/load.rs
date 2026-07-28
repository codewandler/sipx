//! The load harness and the soak assertions, pointed at real sipx calls.
//!
//! The harness's own tests prove it counts correctly. These prove it counts *sipx* correctly —
//! and, in the soak case, that a few hundred real calls leave nothing behind.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
// `caller` and `callee` differ by two letters and are the words the RFCs, the industry and
// everyone reading this already use. Renaming them to satisfy a similarity heuristic would make
// the test harder to read, not easier.
#![allow(clippy::similar_names)]

use std::collections::HashMap;
use std::net::IpAddr;
use std::sync::Arc;
use std::time::Duration;

use sipx_call::{DialOptions, answer, dial};
use sipx_sip::{HeaderName, Host, HostName, Method, Uri};
use sipx_testkit::load::{Cause, Plan, run};
use sipx_testkit::soak::{SETTLE_PAST_TIMERS, Tolerance, soak};
use sipx_transport::{Config, Handle, Incoming, Target, bind};

fn loopback() -> IpAddr {
    "127.0.0.1".parse().expect("valid")
}

/// Place one call and hang up, classified the way the harness wants to hear about it.
///
/// Shared rather than written twice. `Cause::Other(error.to_string())` is the map *key*, so an
/// error whose text embeds a varying detail — an ephemeral port, a status line — produces one
/// entry per failure. Thirty timeouts become thirty singletons, and "which failure is growing"
/// is precisely the question the by-cause report exists to answer.
async fn place_one_call(caller: &Handle, callee: std::net::SocketAddr) -> Result<(), Cause> {
    let to = Uri::sip(Host::Name(HostName::new("callee.example").expect("valid")));
    let options =
        DialOptions::new("<sip:load@example.net>", loopback()).with_timeout(Duration::from_secs(5));

    match dial(caller, Target::udp(callee), &to, &options).await {
        Ok(mut call) => {
            let _ = call.hang_up().await;
            Ok(())
        }
        Err(sipx_call::Error::Rejected { status, .. }) => Err(Cause::Rejected(status)),
        Err(sipx_call::Error::Cancelled(_) | sipx_call::Error::NoResponse) => Err(Cause::Timeout),
        Err(sipx_call::Error::Transport(_) | sipx_call::Error::Io(_)) => Err(Cause::Transport),
        Err(other) => Err(Cause::Other(other.to_string())),
    }
}

/// A callee that answers everything and holds each call until it is hung up on.
///
/// The holding matters twice over. `answer()` spawns a task that retransmits the 2xx until the
/// ACK arrives, and only a live `Call` notifies it — a harness that dropped the `Call` would
/// leave that task resending the 200 OK on the T1 backoff for thirty-two seconds, about eight
/// spurious responses per call, and would never exercise a call with live media at all.
///
/// So in-dialog requests are routed to the call they belong to, by `Call-ID`. That is more
/// machinery than a load generator seems to need, and it is the difference between measuring
/// calls and measuring answer-then-abandon.
async fn answering_endpoint() -> (Handle, tokio::task::JoinHandle<()>) {
    let (endpoint, mut incoming) = bind(Config::new("127.0.0.1:0".parse().expect("valid")))
        .await
        .expect("binds");

    let answering_on = endpoint.clone();
    let task = tokio::spawn(async move {
        let mut calls: HashMap<Vec<u8>, tokio::sync::mpsc::Sender<Incoming>> = HashMap::new();

        while let Some(request) = incoming.recv().await {
            let Some(call_id) = request.request.headers.value(&HeaderName::CallId) else {
                continue;
            };
            let call_id = call_id.into_owned();

            if request.request.method == Method::Invite && !calls.contains_key(&call_id) {
                let (to_call, mut for_call) = tokio::sync::mpsc::channel::<Incoming>(8);
                calls.insert(call_id, to_call);
                let endpoint = answering_on.clone();
                tokio::spawn(async move {
                    let Ok(mut call) = answer(&endpoint, &request, loopback()).await else {
                        return;
                    };
                    // Held until the far end hangs up, feeding it the ACK and the BYE. Without
                    // this the ACK never reaches the call and its 2xx is retransmitted for
                    // thirty-two seconds.
                    while let Some(in_dialog) = for_call.recv().await {
                        let _ = call.handle(&in_dialog).await;
                        if call.is_ended() {
                            return;
                        }
                    }
                });
                continue;
            }

            // Everything else belongs to a call already in progress.
            if let Some(to_call) = calls.get(&call_id)
                && to_call.send(request).await.is_err()
            {
                calls.remove(&call_id);
            }
        }
    });
    (endpoint, task)
}

/// A short run of real calls, reported on.
#[tokio::test]
async fn the_harness_places_real_calls_and_reports_on_them() {
    let (callee, answering) = answering_endpoint().await;
    let callee_addr = callee.local_addr();

    let (caller, _caller_incoming) = bind(Config::new("127.0.0.1:0".parse().expect("valid")))
        .await
        .expect("binds");
    let caller = Arc::new(caller);

    let outcome = run(Plan::new(20, 40.0), move |_| {
        let caller = Arc::clone(&caller);
        async move { place_one_call(&caller, callee_addr).await }
    })
    .await;

    eprintln!("{}", outcome.report());
    assert_eq!(outcome.attempted, 20);
    assert!(
        outcome.succeeded >= 18,
        "loopback should not be losing calls: {}",
        outcome.report()
    );
    assert!(
        outcome.percentile(0.95).is_some(),
        "a run that succeeded must have latencies to report"
    );
    assert!(outcome.calls_per_second() > 0.0);

    answering.abort();
}

/// X-5, against real calls: a few hundred of them must leave nothing behind.
///
/// `#[ignore]`d because it takes minutes rather than seconds, and a test suite people skip is
/// worse than one that runs. CI runs it on a schedule.
#[tokio::test(flavor = "multi_thread")]
#[ignore = "takes minutes; run with --ignored, or on the CI schedule"]
async fn a_soak_run_leaves_nothing_behind() {
    let (callee, answering) = answering_endpoint().await;
    let callee_addr = callee.local_addr();

    let (caller, _caller_incoming) = bind(Config::new("127.0.0.1:0".parse().expect("valid")))
        .await
        .expect("binds");
    let caller = Arc::new(caller);

    // The settling period is forty seconds, not five, and the difference is not caution. A
    // completed SIP transaction sits in `Completed` for Timer J — 64·T1, thirty-two seconds —
    // absorbing retransmissions, exactly as RFC 3261 §17 requires. A five-second settle counts
    // every one of those as a leaked task; the first version of this test did, and reported
    // "tasks grew from 5 to 305" after 300 calls. That is what a one-task-per-call leak looks
    // like, and it was the specification.
    // **Both** endpoints are sampled, and the callee is the one that matters. Server
    // transactions live on whichever side *receives* a request, so a soak that watched only the
    // caller would be blind to the very leak that prompted this — 300 unanswered server
    // transactions, all of them on the side that was being called.
    let watched = (Arc::clone(&caller), callee.clone());
    let outstanding = move || {
        let (caller, callee) = (Arc::clone(&watched.0), watched.1.clone());
        async move { caller.outstanding().await.unwrap_or(0) + callee.outstanding().await.unwrap_or(0) }
    };

    // A **full batch** of warm-up before the baseline, not a token one. A process sampled at
    // start has faulted in almost nothing, and the first calls grow code pages, allocator arenas
    // and the runtime's per-thread caches — all of which reads as a leak.
    //
    // Measured, over four identical 300-call batches with a 40 s settle between each:
    //
    //     batch 1   8544 → 22788 kB   (+14244)
    //     batch 2  22788 → 26124 kB   (+3336)
    //     batch 3  26124 → 28020 kB   (+1896)
    //     batch 4  28020 → 30248 kB   (+2228)
    //
    // So the first batch is overwhelmingly warm-up. **What it does not do is reach zero**: from
    // the second batch on it settles at roughly 2 MB per 300 calls, about 7 kB a call, and that
    // residual is not explained. It is consistent with glibc arena high-water marks under a
    // concurrent workload and it is equally consistent with a small genuine leak; RSS cannot
    // tell those apart, and neither can this test.
    //
    // The honest consequence: **this dimension catches a gross leak and would miss a small
    // one.** It is set from the measurement above rather than tuned until it passed, and the
    // gap is recorded in the story rather than hidden in a comfortable tolerance.
    let warming = Arc::clone(&caller);
    run(Plan::new(300, 60.0), move |_| {
        let caller = Arc::clone(&warming);
        async move { place_one_call(&caller, callee_addr).await }
    })
    .await;
    tokio::time::sleep(SETTLE_PAST_TIMERS).await;

    let tolerance = Tolerance {
        // Three times the measured steady-state growth, which leaves room for a slower CI
        // machine without accepting an order of magnitude more.
        resident_kb: 6 * 1024,
        ..Tolerance::default()
    };

    let placing = Arc::clone(&caller);
    let result = soak(SETTLE_PAST_TIMERS, outstanding, move || async move {
        let outcome = run(Plan::new(300, 30.0), move |_| {
            let caller = Arc::clone(&placing);
            async move { place_one_call(&caller, callee_addr).await }
        })
        .await;
        eprintln!("{}", outcome.report());
        assert!(
            outcome.succeeded * 10 >= outcome.attempted * 9,
            "a soak over a failing stack proves nothing: {}",
            outcome.report()
        );
    })
    .await;

    eprintln!("{}", result.report(tolerance));
    assert!(result.is_flat(tolerance), "{}", result.report(tolerance));

    answering.abort();
}
