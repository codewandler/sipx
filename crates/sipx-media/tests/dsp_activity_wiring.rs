//! `M-119`: an activity hint reaches a reducer because the call carried it, not because an
//! application assembled the path by hand.
//!
//! `M-114` shipped the policy, its vectors and the corpus measurement that decides whether the hint
//! is worth setting at all. `M-67` shipped the door a parameter can be moved through mid-call. Every
//! piece of the path exists; what does not exist is the join, and its absence is not a gap an
//! application notices — it is a gap an application *fills*, by draining the analyser, driving the
//! hint and calling `configure` itself. That is four correct decisions per call, and the fourth one
//! is where a wrong direction becomes silence rather than a refusal.
//!
//! So these tests are about the join and its refusals, not about the policy: what the hint decides
//! is `M-114`'s and is asserted there.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::cast_possible_truncation
)]

use std::net::SocketAddr;

use bytes::Bytes;
use sipx_audio::analysis::{AnalysisProfile, AudioAnalyzer, AudioDirection};
use sipx_audio::dsp::noise::{
    ActivityHint, HintPolicy, NoiseReducer, NoiseReduction, SubbandSuppressor,
};
use sipx_audio::{PcmEncoding, PcmFormat};
use sipx_media::dsp::{ActivityWiring, BuiltIn, GraphBounds, GraphPlan, WiringError};
use sipx_media::{Codec, Config, MediaPort, MediaSession, Processing};
use sipx_rtp::Packet;
use tokio::net::UdpSocket;

const SAMPLES_PER_PACKET: usize = 160;

/// Acceptance row 1: the observations one call's analyser produces reach that call's reducer as a
/// declared parameter set, through `M-67`'s door and no other.
#[tokio::test]
async fn an_analysers_observations_reach_the_reducer_through_the_control_surface() {
    let (session, peer, session_addr) = session_and_peer().await;

    let graph = session
        .attach_dsp(
            GraphPlan::new(AudioDirection::Inbound, GraphBounds::new())
                .with_built_in(BuiltIn::SubbandSuppressor, &[])
                .expect("the shipped suppressor is a registered stage"),
        )
        .expect("a validated inbound graph activates");

    let frames = session
        .attach_processor(Processing::new(
            AudioDirection::Inbound,
            PcmFormat::new(8_000, PcmEncoding::Signed16).expect("a valid format"),
        ))
        .expect("the seam accepts one attachment per direction");

    let wiring = ActivityWiring::new(
        frames,
        AudioAnalyzer::new(AnalysisProfile::new(AudioDirection::Inbound, 8_000))
            .expect("the reference profile is valid"),
        ActivityHint::new(AudioDirection::Inbound, reducer(), HintPolicy::default()),
        graph,
        0,
    )
    .expect("hint, graph and seam all follow the inbound direction");

    // Speech-shaped audio, then silence: the hint must be set while the analyser calls it voice and
    // released after it stops, and both must arrive as parameter updates rather than as a flag the
    // wiring kept to itself.
    feed_active(&peer, session_addr, 40, 0).await;
    feed(&peer, session_addr, 40, 0xff).await;

    let outcome = wiring.run_until_idle();

    assert!(
        outcome.updates_applied() > 0,
        "no parameter update reached the reducer: {outcome:?}"
    );
    assert_eq!(
        outcome.updates_refused(),
        0,
        "the wiring drove a parameter the reducer's declaration does not accept: {outcome:?}"
    );

    session.shutdown().await;
}

/// Acceptance row 3: an analyser bound to one direction cannot be wired to the other direction's
/// graph, and saying so is a typed refusal rather than a hint that is quietly about the wrong audio.
#[tokio::test]
async fn a_hint_for_one_direction_cannot_be_wired_to_the_others_graph() {
    let (session, _peer, _session_addr) = session_and_peer().await;

    let outbound = session
        .attach_dsp(
            GraphPlan::new(AudioDirection::Outbound, GraphBounds::new())
                .with_built_in(BuiltIn::SubbandSuppressor, &[])
                .expect("the shipped suppressor is a registered stage"),
        )
        .expect("a validated outbound graph activates");

    let frames = session
        .attach_processor(Processing::new(
            AudioDirection::Inbound,
            PcmFormat::new(8_000, PcmEncoding::Signed16).expect("a valid format"),
        ))
        .expect("the seam accepts one attachment per direction");

    let refused = ActivityWiring::new(
        frames,
        AudioAnalyzer::new(AnalysisProfile::new(AudioDirection::Inbound, 8_000))
            .expect("the reference profile is valid"),
        ActivityHint::new(AudioDirection::Inbound, reducer(), HintPolicy::default()),
        outbound,
        0,
    );

    assert!(
        matches!(
            refused,
            Err(WiringError::DirectionMismatch {
                hint: AudioDirection::Inbound,
                graph: AudioDirection::Outbound,
                ..
            })
        ),
        "an inbound hint on an outbound graph must be refused by name, got {:?}",
        refused.map(|_| "a wiring"),
    );

    session.shutdown().await;
}

/// Acceptance row 2's second half: a call without the wiring behaves exactly as it does today.
///
/// Asserted on samples rather than by inspection, because "it only runs when you ask for it" is a
/// claim about a code path and this is a claim about audio.
#[tokio::test]
async fn a_call_without_the_wiring_carries_identical_samples() {
    let unwired = carry_and_collect(false).await;
    let wired_but_silent = carry_and_collect(true).await;

    assert_eq!(
        unwired, wired_but_silent,
        "attaching the wiring changed audio the detector never called voice"
    );
}

/// Acceptance row 4: the per-frame parameter set puts no work on the media worker.
///
/// The bound is asserted, not argued: the wiring is driven from the side that already owns the
/// frames, so the graph's own counters must show no deadline miss and no bypass attributable to it.
#[tokio::test]
async fn the_wiring_puts_no_work_on_the_media_worker() {
    let (session, peer, session_addr) = session_and_peer().await;

    let graph = session
        .attach_dsp(
            GraphPlan::new(AudioDirection::Inbound, GraphBounds::new())
                .with_built_in(BuiltIn::SubbandSuppressor, &[])
                .expect("the shipped suppressor is a registered stage"),
        )
        .expect("a validated inbound graph activates");
    let counters_before = graph.counters();

    let frames = session
        .attach_processor(Processing::new(
            AudioDirection::Inbound,
            PcmFormat::new(8_000, PcmEncoding::Signed16).expect("a valid format"),
        ))
        .expect("the seam accepts one attachment per direction");
    let wiring = ActivityWiring::new(
        frames,
        AudioAnalyzer::new(AnalysisProfile::new(AudioDirection::Inbound, 8_000))
            .expect("the reference profile is valid"),
        ActivityHint::new(AudioDirection::Inbound, reducer(), HintPolicy::default()),
        graph,
        0,
    )
    .expect("hint, graph and seam all follow the inbound direction");

    feed_active(&peer, session_addr, 60, 0).await;
    let outcome = wiring.run_until_idle();

    let after = outcome.graph().counters();
    assert_eq!(
        after.deadline_misses(),
        counters_before.deadline_misses(),
        "the wiring cost the media worker a deadline"
    );
    assert_eq!(
        after.bypasses(),
        counters_before.bypasses(),
        "the wiring bypassed a stage it was supposed to be configuring"
    );

    session.shutdown().await;
}

async fn carry_and_collect(wire: bool) -> Vec<i16> {
    let (session, peer, session_addr) = session_and_peer().await;
    let graph = session
        .attach_dsp(
            GraphPlan::new(AudioDirection::Inbound, GraphBounds::new())
                .with_built_in(BuiltIn::SubbandSuppressor, &[])
                .expect("the shipped suppressor is a registered stage"),
        )
        .expect("a validated inbound graph activates");

    let mut frames = session
        .attach_processor(Processing::new(
            AudioDirection::Inbound,
            PcmFormat::new(8_000, PcmEncoding::Signed16).expect("a valid format"),
        ))
        .expect("the seam accepts one attachment per direction");

    // Silence: a stream the detector never calls voice, which is where "identical" has to hold.
    feed(&peer, session_addr, 30, 0xff).await;

    let collected = if wire {
        let wiring = ActivityWiring::new(
            frames,
            AudioAnalyzer::new(AnalysisProfile::new(AudioDirection::Inbound, 8_000))
                .expect("the reference profile is valid"),
            ActivityHint::new(AudioDirection::Inbound, reducer(), HintPolicy::default()),
            graph,
            0,
        )
        .expect("hint, graph and seam all follow the inbound direction");
        wiring.run_until_idle().into_samples()
    } else {
        let mut samples = Vec::new();
        while let Some(frame) = frames.try_recv() {
            samples.extend_from_slice(
                &frame
                    .pcm()
                    .to_i16(8_000)
                    .expect("the seam hands back its own format"),
            );
        }
        samples
    };

    session.shutdown().await;
    collected
}

async fn session_and_peer() -> (MediaSession, UdpSocket, SocketAddr) {
    let peer = UdpSocket::bind("127.0.0.1:0").await.expect("binds");
    let peer_addr = peer.local_addr().expect("has an address");

    let port = MediaPort::bind("127.0.0.1:0".parse().expect("valid"))
        .await
        .expect("binds");
    let session_addr = port.local_addr();

    let mut config = Config::new(peer_addr, Codec::Pcmu);
    config.rtcp_interval = None;
    (
        port.start(config).expect("valid media setup"),
        peer,
        session_addr,
    )
}

async fn feed(peer: &UdpSocket, to: SocketAddr, packets: u16, level: u8) {
    for sequence in 0..packets {
        let encoded = Packet::new(
            Codec::Pcmu.payload_type(),
            sequence,
            u32::from(sequence) * SAMPLES_PER_PACKET as u32,
            0x2c54_0001,
            Bytes::from(vec![level; SAMPLES_PER_PACKET]),
        )
        .encode();
        peer.send_to(&encoded, to).await.expect("sends");
    }
    settle().await;
}

/// Feed audio that a detector can actually call voice.
///
/// A constant sample level is not speech and no threshold should treat it as such: what a detector
/// measures is deviation within a window, and a flat line has none however loud it is. So this
/// alternates full-scale, which is the crudest signal with energy in it — the point here is that the
/// analyser produces observations at all, not that they are speech-like. What the policy does with
/// them is `M-114`'s, and asserted there.
async fn feed_active(peer: &UdpSocket, to: SocketAddr, packets: u16, first_sequence: u16) {
    for offset in 0..packets {
        let sequence = first_sequence + offset;
        let samples: Vec<u8> = (0..SAMPLES_PER_PACKET)
            .map(|i| {
                let sample: i16 = if i % 2 == 0 { 24_000 } else { -24_000 };
                sipx_audio::ulaw_encode(sample)
            })
            .collect();
        let encoded = Packet::new(
            Codec::Pcmu.payload_type(),
            sequence,
            u32::from(sequence) * SAMPLES_PER_PACKET as u32,
            0x2c54_0001,
            Bytes::from(samples),
        )
        .encode();
        peer.send_to(&encoded, to).await.expect("sends");
    }
    settle().await;
}

/// Let the session decode what was just sent and queue it at the seam.
///
/// The wiring drains what is already there rather than awaiting more, which is what keeps it off
/// the media worker — so a test has to give the worker its turn before it looks.
async fn settle() {
    tokio::time::sleep(std::time::Duration::from_millis(120)).await;
}

/// The declaration the shipped suppressor actually makes.
///
/// Taken from the reducer rather than hand-built, and that is not fussiness: a `NoiseReduction`
/// assembled in a test declares no activity input, and a reducer that consumes nothing is wired to
/// nothing. The hint would then be correct, silent, and indistinguishable from a broken join.
fn reducer() -> NoiseReduction {
    SubbandSuppressor::new().noise_reduction()
}
