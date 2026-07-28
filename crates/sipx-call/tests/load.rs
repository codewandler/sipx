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

use std::net::IpAddr;
use std::sync::Arc;
use std::time::Duration;

use sipx_call::{DialOptions, answer, dial};
use sipx_sip::{Host, HostName, Method, Uri};
use sipx_testkit::load::{Cause, Plan, run};
use sipx_testkit::soak::{SETTLE_PAST_TIMERS, Tolerance, soak};
use sipx_transport::{Config, Handle, Target, bind};

fn loopback() -> IpAddr {
    "127.0.0.1".parse().expect("valid")
}

/// A callee that answers everything until it is dropped, and a handle to call it on.
async fn answering_endpoint() -> (Handle, tokio::task::JoinHandle<()>) {
    let (endpoint, mut incoming) = bind(Config::new("127.0.0.1:0".parse().expect("valid")))
        .await
        .expect("binds");

    let answering_on = endpoint.clone();
    let task = tokio::spawn(async move {
        while let Some(request) = incoming.recv().await {
            match request.request.method {
                Method::Invite => {
                    let endpoint = answering_on.clone();
                    tokio::spawn(async move {
                        if let Ok(mut call) = answer(&endpoint, &request, loopback()).await {
                            tokio::time::sleep(Duration::from_millis(80)).await;
                            let _ = call.hang_up().await;
                        }
                    });
                }
                // A BYE nobody answers leaves the caller waiting out its own timeout, which
                // would put two seconds into every latency percentile this harness reports —
                // a measurement of the test rather than of sipx.
                Method::Bye => {
                    if let Some(ok) = sipx_sip::StatusCode::new(200) {
                        if let Ok(builder) =
                            sipx_sip::build::ResponseBuilder::to_request(&request.request, ok, "OK")
                        {
                            let _ = answering_on.respond(&request.key, builder.build()).await;
                        }
                    }
                }
                _ => {}
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
        async move {
            let to = Uri::sip(Host::Name(HostName::new("callee.example").expect("valid")));
            let options = DialOptions::new("<sip:load@example.net>", loopback())
                .with_timeout(Duration::from_secs(5));
            match dial(&caller, Target::udp(callee_addr), &to, &options).await {
                Ok(mut call) => {
                    let _ = call.hang_up().await;
                    Ok(())
                }
                Err(sipx_call::Error::Rejected { status, .. }) => Err(Cause::Rejected(status)),
                Err(sipx_call::Error::Cancelled(_) | sipx_call::Error::NoResponse) => {
                    Err(Cause::Timeout)
                }
                Err(other) => Err(Cause::Other(other.to_string())),
            }
        }
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
    // The transaction store is sampled too, not just tasks and descriptors. A store that
    // leaks is a slow, quiet outage: the stack works for hours and then stops. `outstanding`
    // has a tolerance of zero — one leftover transaction is one whose call is over.
    let counting = Arc::clone(&caller);
    let outstanding = move || {
        let counting = Arc::clone(&counting);
        tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current()
                .block_on(async move { counting.outstanding().await.unwrap_or(0) })
        })
    };

    let placing = Arc::clone(&caller);
    let result = soak(SETTLE_PAST_TIMERS, outstanding, move || async move {
        let outcome = run(Plan::new(300, 30.0), move |_| {
            let caller = Arc::clone(&placing);
            async move {
                let to = Uri::sip(Host::Name(HostName::new("callee.example").expect("valid")));
                let options = DialOptions::new("<sip:soak@example.net>", loopback())
                    .with_timeout(Duration::from_secs(5));
                match dial(&caller, Target::udp(callee_addr), &to, &options).await {
                    Ok(mut call) => {
                        let _ = call.hang_up().await;
                        Ok(())
                    }
                    Err(error) => Err(Cause::Other(error.to_string())),
                }
            }
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

    for extra in [0u64, 20, 40, 60] {
        if extra > 0 {
            tokio::time::sleep(Duration::from_secs(20)).await;
        }
        eprintln!(
            "PROBE +{extra}s outstanding={}",
            caller.outstanding().await.unwrap_or(0)
        );
    }
    eprintln!("{}", result.report(Tolerance::default()));
    assert!(
        result.is_flat(Tolerance::default()),
        "{}",
        result.report(Tolerance::default())
    );

    answering.abort();
}
