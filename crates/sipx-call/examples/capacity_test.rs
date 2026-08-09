//! Find the limit: hold calls up in growing steps and record what breaks first.
//!
//! This is the **capacity** half of the pair. `load_test` answers "how fast can calls be set up";
//! this one answers "how many can be held at once, and what degrades when there are too many".
//! The difference is the call duration: here every call stays up for the whole run, so the load is
//! cumulative rather than a rate.
//!
//! ```text
//! cargo run --release --example capacity_test
//! cargo run --release --example capacity_test -- --ramp 50,500,500,500 --dwell 10
//! cargo run --release --example capacity_test -- --media none --ramp 100,100,100
//! cargo run --release --example capacity_test -- --ramp 100,100,100 --record docs/measurements/capacity.json
//! ```
//!
//! **Use `--release`.** A debug build measures the debug build; the codec and jitter paths are the
//! hot ones and they are several times slower without optimisation, so a debug ceiling says
//! nothing about a deployed one.
//!
//! # What it does
//!
//! Each step places its calls, waits `--dwell` for them to settle, then samples every call that is
//! still up. Nothing is torn down between steps, so step three is running step one's calls too.
//!
//! `--media` selects one of three shapes, and they are three different questions. `full` and `idle`
//! both negotiate a session — so both bind an RTP socket and carry a jitter buffer — and differ only
//! in whether audio flows, which makes their difference the cost of *carrying* audio. `none`
//! negotiates nothing at all on either end, so its difference from `idle` is the cost of having a
//! media stack. A deployment that runs its media elsewhere should read `none`; see [`Media`].
//!
//! # The ceiling shows up in the clock before it shows up in the failures
//!
//! In a machine with nothing to do, a step costs exactly one `--dwell` and nothing else: placing
//! the calls and reading their counters are free. So the run *should* take `steps × dwell`, and
//! every second above that is time the machine could not absorb inside its idle window. That
//! surplus is reported three ways:
//!
//! * **`over`** — this step's wall clock minus the dwell. The work the step could not hide.
//! * **`drift`** — the whole run's wall clock minus `step × dwell`. Where the run stands against
//!   the schedule: at step two of four, a drift already past half the total dwell budget means the
//!   ceiling is close whatever the failure count says.
//! * **`sample`** — how long reading every live call's counters took. The purest probe of the
//!   three, because it is pure bookkeeping: it stays in the milliseconds until the runtime can no
//!   longer schedule the reads, and then it does not.
//!
//! The run stops on either signal: a step that could not place its calls, or **a step that spent
//! longer working than waiting** (`over > dwell`). The second arrives first, which is the point —
//! by the time calls fail, the limit is behind you.
//!
//! **Drift measures the machine, not sipx.** On a box doing anything else the surplus is charged
//! here regardless of whose work it was, so a run that shares a machine reports a ceiling that
//! belongs to the machine. Run it on a quiet box or the number means nothing.
//!
//! # What is recorded, and what each number is
//!
//! | column | what it is |
//! |---|---|
//! | `target` / `established` | calls asked for, and calls actually up when the step was sampled |
//! | `setup p50/p95/p99` | INVITE to usable dialog, for *this step's* calls only — nearest-rank |
//! | `rtt p50` | RTCP round-trip across live calls: request/response latency under the load |
//! | `loss` / `jitter` | RFC 3550 §6.4.1, reported by the far end about what it received |
//! | `mos` | an **estimate** from the G.107 E-model, not a measurement — see [`sipx_rtp::Quality::mos`] |
//! | `rss` | this process's resident set. With the server in-process it covers **both ends**; use `--server` to measure a client alone |
//! | `rss/call` | resident set divided by established calls, which is an average and hides per-call variance |
//!
//! # What this is not
//!
//! **Not a benchmark against any other stack**, and not a deployment sizing figure. Both ends share
//! one process, one kernel and one scheduler, so the client's cost is charged to the same CPU as
//! the server's and neither is isolated. A ceiling found here is this machine's ceiling for this
//! shape of traffic. What it is good for is a number that can be tracked: run it before and after a
//! change and compare the recorded file.

// Samples are read before they are run, so they are written for readability.
#![allow(clippy::cast_possible_truncation, clippy::cast_precision_loss)]

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use bytes::Bytes;
use sipx_call::{
    Call, DialOptions, Dispatched, Dispatcher, SignallingDial, SignallingDialOptions,
    SignallingIdentity, dial, dial_signalling, serve,
};
use sipx_sip::{Host, HostName, Uri};
use sipx_transport::{Config, Target, bind};

/// How much media each held call carries.
///
/// **`idle` is not "no media".** The session is still negotiated, the RTP socket is still bound and
/// the jitter buffer still exists — what stops is audio being encoded and sent. So the difference
/// between `full` and `idle` is the cost of *carrying* audio, not the cost of having a media stack.
///
/// `none` is the third shape and the one a deployment that runs its media elsewhere should read: no
/// SDP offer, no RTP socket, on **both** ends. It places its calls through
/// [`sipx_call::dial_signalling`] and answers them with
/// [`Invitation::answer_signalling`](sipx_call::Invitation::answer_signalling) — the same pair
/// `sipx load --mode signalling` and `sipx load-responder` use, rather than a copy of it living
/// here (`T-46`).
///
/// A run in `none` reports no audio figures at all. They are withheld and not zeroed: `loss`,
/// `jitter` and `mos` are computed from a receive stream, and over an empty one they read as a
/// perfect score for a call that carried nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Media {
    /// A negotiated session with audio flowing for the whole run.
    Full,
    /// A negotiated session that sends nothing.
    Idle,
    /// No session at all: no SDP, no RTP socket, no jitter buffer.
    None,
}

impl Media {
    /// What the recorded run calls this mode.
    const fn as_str(self) -> &'static str {
        match self {
            Self::Full => "full",
            Self::Idle => "idle",
            Self::None => "none",
        }
    }
}

/// One call held for the rest of the run.
///
/// The media-free mode holds a different thing, and that difference is the measurement: a
/// [`SignallingDial`] owns no media session, so there are no counters to read off it and the audio
/// columns are absent rather than zero.
#[allow(
    clippy::large_enum_variant,
    reason = "boxing the media half would add an allocation per held call and move its bytes off \
              the vector, which is a change to the very thing this harness measures; the committed \
              full and idle runs were taken against the unboxed layout"
)]
enum Held {
    /// A call with a negotiated media session, flowing or idle.
    Media(Call),
    /// A confirmed dialog with no session.
    Signalling(SignallingDial),
}

impl Held {
    /// The media session, when this call has one.
    fn media(&self) -> Option<&sipx_media::MediaSession> {
        match self {
            Self::Media(call) => Some(call.media()),
            Self::Signalling(_) => None,
        }
    }

    /// End the dialog, whichever shape it is.
    async fn hang_up(&mut self) {
        match self {
            Self::Media(call) => {
                let _ = call.hang_up().await;
            }
            Self::Signalling(call) => {
                let _ = call.hang_up(TEARDOWN_BOUND).await;
            }
        }
    }
}

/// Both ends. Written once so the client and the server cannot disagree about it.
const LOOPBACK: IpAddr = IpAddr::V4(Ipv4Addr::LOCALHOST);

/// How long one call may take to set up before it counts as a failure rather than a slow success.
const SETUP_BOUND: Duration = Duration::from_secs(10);

/// How long a media-free BYE may go unanswered before the teardown is called failed.
///
/// A bound on a failure and not a wait: the far end is in this process and answers immediately.
/// `Call::hang_up` carries its own, which is why only the signalling half names one here.
const TEARDOWN_BOUND: Duration = Duration::from_secs(10);

/// The media clock G.711 runs at, and one 20 ms packet's worth of samples at it.
const CLOCK: usize = 8_000;
const FRAME: usize = CLOCK / 50;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let options = Options::parse(std::env::args().skip(1))?;

    let tone = tone(CLOCK);
    let running = Arc::new(AtomicBool::new(true));
    let server = if let Some(address) = options.server {
        println!("client only: holding calls against {address}");
        address
    } else {
        let address = start_server(Arc::clone(&tone), options.media, Arc::clone(&running)).await?;
        println!("server in this process, listening on {address}");
        address
    };

    let (endpoint, _incoming) = bind(Config::new("127.0.0.1:0".parse()?)).await?;
    let endpoint = Arc::new(endpoint);

    println!(
        "ramp {:?}, {} s dwell between steps\n",
        options.ramp,
        options.dwell.as_secs()
    );
    header();

    // Every call placed so far, held for the whole run. Dropping one would end it, which is the
    // difference between this and `load_test`.
    let mut held: Vec<Held> = Vec::new();
    let mut playing: Vec<tokio::task::JoinHandle<()>> = Vec::new();
    let mut steps: Vec<Step> = Vec::new();
    let mut stopped_early = None;

    let run_started = Instant::now();
    for (index, target) in options.ramp.iter().copied().enumerate() {
        let step_started = Instant::now();
        let (placed, tones, setup, failures) =
            place_many(&endpoint, server, target, &tone, options.media, &running).await;
        let established_now = placed.len();
        held.extend(placed);
        playing.extend(tones);

        tokio::time::sleep(options.dwell).await;

        let (mut step, _) =
            sample(index + 1, target, established_now, setup, failures, &held).await;
        step.wall = step_started.elapsed();
        step.overhead = step.wall.saturating_sub(options.dwell);
        // What the schedule said the run should have cost by now: one dwell per step and nothing
        // else. Everything above it is the cost being measured.
        let scheduled = options.dwell * u32::try_from(index + 1).unwrap_or(u32::MAX);
        step.drift = run_started.elapsed().saturating_sub(scheduled);
        row(&step);

        let short = established_now < target;
        // The ceiling, reached before anything fails. A step that spends longer working than it
        // spends waiting has stopped settling and started struggling; the dwell is no longer a
        // settle window, it is the only part of the step the machine kept up with.
        let struggling = step.overhead > options.dwell;
        steps.push(step);

        if short {
            stopped_early = Some(format!(
                "step {} placed {established_now} of {target}",
                index + 1
            ));
            break;
        }
        if struggling {
            stopped_early = Some(format!(
                "step {} spent longer working than waiting ({:.1}s over a {}s dwell)",
                index + 1,
                options.dwell.as_secs_f64() + options.dwell.as_secs_f64(),
                options.dwell.as_secs()
            ));
            break;
        }
    }

    println!();
    match &stopped_early {
        Some(why) => println!("stopped early: {why} — the limit is the last complete step"),
        None => println!(
            "every step completed: {} calls held, no ceiling found. Extend --ramp to look for one.",
            held.len()
        ),
    }

    if options.media == Media::Full && steps.iter().any(|step| !step.audio_observed) {
        println!(
            "warning: some steps saw no RTP at all, so their loss/jitter/mos are withheld rather \
             than reported as perfect"
        );
    }
    if let Some(path) = &options.record {
        std::fs::write(path, record(&steps, &options, stopped_early.as_deref()))?;
        println!("recorded to {path}");
    }

    // Stop the tones **before** hanging up, and stop them by asking. `abort()` works too now
    // (`M-93` made every wait in the media API a suspension point, so an aborted play task is
    // reaped and the clip it was feeding is discarded), and it is measurably the quicker of the
    // two — 5.3 s against 6.2 s for this ramp, because it does not wait out the clip in flight.
    // Asking is still what this example does: the second of those numbers is the honest one, since
    // a tone aborted mid-frame leaves a call that stopped talking a fifth of a second before it
    // was hung up, and the point here is to time a teardown rather than to shorten it.
    running.store(false, Ordering::Relaxed);
    for task in playing {
        let _ = task.await;
    }
    // Hanging up is not required for the measurement, but leaving a thousand dialogs for the
    // process teardown to reap makes the next run measure the last one's leftovers.
    for mut call in held {
        call.hang_up().await;
    }
    Ok(())
}

/// One step's worth of calls, placed concurrently.
///
/// Returns the calls that came up, their setup latencies, and the failures by cause. Placing them
/// concurrently is deliberate: a step is meant to be a step change in load, and placing serially
/// would ramp gently enough to hide the behaviour the step exists to provoke.
async fn place_many(
    endpoint: &Arc<sipx_transport::Handle>,
    server: SocketAddr,
    target: usize,
    tone: &Arc<Vec<i16>>,
    media: Media,
    running: &Arc<AtomicBool>,
) -> (
    Vec<Held>,
    Vec<tokio::task::JoinHandle<()>>,
    Vec<Duration>,
    Vec<String>,
) {
    let mut placing = tokio::task::JoinSet::new();
    for _ in 0..target {
        let endpoint = Arc::clone(endpoint);
        let tone = Arc::clone(tone);
        let running = Arc::clone(running);
        let media_mode = media;
        placing.spawn(async move {
            let started = Instant::now();
            let result = place_one(&endpoint, server, media_mode).await;
            match result {
                Ok(call) => {
                    let elapsed = started.elapsed();
                    let mut tone_task = None;
                    if let Held::Media(call) = &call
                        && media_mode == Media::Full
                    {
                        // Keep audio flowing for the rest of the run, so loss and jitter are
                        // measurements rather than a report about an idle stream.
                        let media = call.media_handle();
                        tone_task = Some(tokio::spawn(async move {
                            // Two ways out, and this needs both. `running` is how the harness
                            // stops the tones before it hangs up, so the teardown it times is not
                            // also draining a clip. `play`'s own answer is how a tone stops when
                            // its call ends first: `false` means the session has gone, and a loop
                            // that played on regardless would be a busy loop rather than a paced
                            // one, because a stopped session has no send queue left to pace it
                            // (`M-93`).
                            while running.load(Ordering::Relaxed) && media.play(&tone, FRAME).await
                            {
                            }
                        }));
                    }
                    Ok((call, tone_task, elapsed))
                }
                Err(error) => Err(error),
            }
        });
    }

    let mut calls = Vec::with_capacity(target);
    let mut tone_tasks = Vec::with_capacity(target);
    let mut setup = Vec::with_capacity(target);
    let mut failures = Vec::new();
    while let Some(joined) = placing.join_next().await {
        match joined {
            Ok(Ok((call, tone_task, elapsed))) => {
                calls.push(call);
                if let Some(tone_task) = tone_task {
                    tone_tasks.push(tone_task);
                }
                setup.push(elapsed);
            }
            Ok(Err(error)) => failures.push(error),
            Err(error) => failures.push(format!("harness task: {error}")),
        }
    }
    setup.sort_unstable();
    (calls, tone_tasks, setup, failures)
}

async fn place_one(
    endpoint: &sipx_transport::Handle,
    server: SocketAddr,
    media: Media,
) -> Result<Held, String> {
    let to = Uri::sip(Host::Name(
        HostName::new("callee.example").map_err(|error| error.to_string())?,
    ));
    if media == Media::None {
        // The library's own media-free path, not a copy of it living in an example: `dial_signalling`
        // is what `sipx load --mode signalling` places its calls through too (`T-46`).
        let from = Uri::parse(Bytes::from_static(b"sip:capacity@example.net"))
            .map_err(|error| error.to_string())?;
        let options = SignallingDialOptions::new(SignallingIdentity::fresh(endpoint, &to, &from))
            .with_timeout(SETUP_BOUND);
        return dial_signalling(endpoint, Target::udp(server), &to, &options)
            .await
            .map(Held::Signalling)
            .map_err(|error| error.to_string());
    }
    let options =
        DialOptions::new("<sip:capacity@example.net>", LOOPBACK).with_timeout(SETUP_BOUND);
    dial(endpoint, Target::udp(server), &to, &options)
        .await
        .map(Held::Media)
        .map_err(|error| error.to_string())
}

/// What one step looked like once its calls had settled.
struct Step {
    index: usize,
    target: usize,
    established: usize,
    held: usize,
    setup: Vec<Duration>,
    failures: Vec<String>,
    rtt: Option<Duration>,
    loss: f64,
    jitter: Duration,
    mos: f64,
    packets: u64,
    discards: u64,
    rss_bytes: Option<u64>,
    /// Wall clock for the whole step, dwell included.
    wall: Duration,
    /// How long sampling every live call took. The purest saturation probe here: reading counters
    /// off N calls is nearly free until the runtime cannot schedule the reads.
    sample_wall: Duration,
    /// Wall clock minus the dwell — the work the machine could not absorb inside its idle window.
    overhead: Duration,
    /// Cumulative wall clock minus what the schedule said it should be by now.
    drift: Duration,
    /// Whether any RTP arrived at all.
    ///
    /// **The audio columns are suppressed when this is false, and that is the point.** `loss`,
    /// `jitter` and `mos` are computed from a receive stream; over an empty one they read 0%, 0 ms
    /// and 4.40 — a perfect score for a call that carried nothing. The first run of this example
    /// printed exactly that for a thousand calls, because the server never talked back. A figure
    /// that cannot be anything but good is not a measurement.
    audio_observed: bool,
}

/// Sample every live call and the process itself.
async fn sample(
    index: usize,
    target: usize,
    established: usize,
    setup: Vec<Duration>,
    failures: Vec<String>,
    held: &[Held],
) -> (Step, Duration) {
    let sampling_started = Instant::now();
    let mut round_trips = Vec::new();
    let (mut loss, mut mos, mut jitter_total) = (0.0_f64, 0.0_f64, Duration::ZERO);
    let (mut packets, mut discards, mut sampled) = (0_u64, 0_u64, 0_usize);

    // A media-free call contributes nothing here — not a zero. It has no session to read counters
    // off, so it is not in the denominator either, and `audio_observed` below stays false.
    for media in held.iter().filter_map(Held::media) {
        let quality = media.quality().await;
        if let Some(round_trip) = quality.round_trip {
            round_trips.push(round_trip);
        }
        loss += quality.loss;
        mos += quality.mos;
        jitter_total += quality.jitter;
        packets += media.packets_received();
        discards += media.discard_counts().total();
        sampled += 1;
    }

    let sample_wall = sampling_started.elapsed();
    let mean = |total: f64| {
        if sampled == 0 {
            0.0
        } else {
            total / sampled as f64
        }
    };
    round_trips.sort_unstable();
    let step = Step {
        index,
        target,
        established,
        held: held.len(),
        setup,
        failures,
        rtt: percentile(&round_trips, 0.5),
        loss: mean(loss),
        jitter: if sampled == 0 {
            Duration::ZERO
        } else {
            jitter_total / sampled as u32
        },
        mos: mean(mos),
        packets,
        discards,
        rss_bytes: resident_set(),
        audio_observed: packets > 0,
        wall: Duration::ZERO,
        sample_wall,
        overhead: Duration::ZERO,
        drift: Duration::ZERO,
    };
    (step, sample_wall)
}

/// Nearest-rank, so every figure names a sample that actually happened.
fn percentile(sorted: &[Duration], fraction: f64) -> Option<Duration> {
    if sorted.is_empty() {
        return None;
    }
    #[allow(
        clippy::cast_sign_loss,
        reason = "fraction is 0..=1 and len is positive, so the product cannot be negative"
    )]
    let rank = (fraction * sorted.len() as f64).ceil().max(1.0) as usize;
    sorted.get(rank.min(sorted.len()) - 1).copied()
}

/// This process's resident set, from the kernel.
///
/// `None` off Linux rather than a guess: a capacity figure invented from a platform that does not
/// publish one is worse than a blank column.
fn resident_set() -> Option<u64> {
    let status = std::fs::read_to_string("/proc/self/status").ok()?;
    let line = status.lines().find(|line| line.starts_with("VmRSS:"))?;
    let kib: u64 = line.split_whitespace().nth(1)?.parse().ok()?;
    Some(kib * 1024)
}

fn header() {
    println!(
        "{:>4}  {:>7}  {:>7}  {:>6}  {:>8} {:>8} {:>8}  {:>7}  {:>6}  {:>8}  {:>4}  {:>8}  {:>9}  {:>7}  {:>7}  {:>7}",
        "step",
        "target",
        "up",
        "held",
        "setup50",
        "setup95",
        "setup99",
        "rtt50",
        "loss",
        "jitter",
        "mos",
        "rss",
        "rss/call",
        "sample",
        "over",
        "drift",
    );
}

fn row(step: &Step) {
    // Two decimals, because on loopback the honest answer is a fraction of a millisecond and
    // rounding it to `0ms` reads as "not measured" rather than as "fast".
    let ms = |d: Option<Duration>| {
        d.map_or_else(
            || "      —".to_owned(),
            |d| format!("{:>5.2}ms", d.as_secs_f64() * 1000.0),
        )
    };
    let mib = |bytes: Option<u64>| {
        bytes.map_or_else(
            || "       —".to_owned(),
            |bytes| format!("{:>6.0}MB", bytes as f64 / 1_048_576.0),
        )
    };
    let (loss, jitter, mos) = if step.audio_observed {
        (
            format!("{:>5.2}%", step.loss * 100.0),
            format!("{:>6.2}ms", step.jitter.as_secs_f64() * 1000.0),
            format!("{:>4.2}", step.mos),
        )
    } else {
        ("    —".to_owned(), "     —".to_owned(), "   —".to_owned())
    };
    println!(
        "{:>4}  {:>7}  {:>7}  {:>6}  {:>8} {:>8} {:>8}  {:>7}  {:>6}  {:>8}  {:>4}  {:>8}  {:>7}  {:>6.2}s  {:>6.2}s  {:>6.2}s",
        step.index,
        step.target,
        step.established,
        step.held,
        ms(percentile(&step.setup, 0.5)),
        ms(percentile(&step.setup, 0.95)),
        ms(percentile(&step.setup, 0.99)),
        ms(step.rtt),
        loss,
        jitter,
        mos,
        mib(step.rss_bytes),
        step.rss_bytes.map_or_else(
            || "      —".to_owned(),
            |bytes| if step.held == 0 {
                "      —".to_owned()
            } else {
                format!("{:>5.0}KB", bytes as f64 / step.held as f64 / 1024.0)
            }
        ),
        step.sample_wall.as_secs_f64(),
        step.overhead.as_secs_f64(),
        step.drift.as_secs_f64(),
    );
}

/// The run as JSON, for committing beside the code and diffing across changes.
#[allow(
    clippy::format_push_string,
    reason = "a JSON record built line by line reads as the shape it produces; `write!` into a \
              String here would add an unwrap per line and teach nothing"
)]
fn record(steps: &[Step], options: &Options, stopped_early: Option<&str>) -> String {
    let mut out = String::from("{\n");
    out.push_str(&format!(
        "  \"ramp\": {:?},\n  \"dwell_seconds\": {},\n  \"media\": \"{}\",\n",
        options.ramp,
        options.dwell.as_secs(),
        options.media.as_str()
    ));
    out.push_str(&format!(
        "  \"stopped_early\": {},\n",
        stopped_early.map_or_else(|| "null".to_owned(), |why| format!("{why:?}"))
    ));
    out.push_str("  \"steps\": [\n");
    for (index, step) in steps.iter().enumerate() {
        let ms = |d: Option<Duration>| {
            d.map_or_else(
                || "null".to_owned(),
                |d| format!("{:.1}", d.as_secs_f64() * 1000.0),
            )
        };
        out.push_str(&format!(
            "    {{\"step\": {}, \"target\": {}, \"established\": {}, \"held\": {}, \
             \"setup_p50_ms\": {}, \"setup_p95_ms\": {}, \"setup_p99_ms\": {}, \
             \"rtt_p50_ms\": {}, \"audio_observed\": {}, \"loss\": {}, \"jitter_ms\": {}, \"mos\": {}, \
             \"packets_received\": {}, \"discards\": {}, \"rss_bytes\": {}, \"failures\": {}, \
             \"wall_s\": {:.2}, \"sample_s\": {:.3}, \"overhead_s\": {:.2}, \"drift_s\": {:.2}}}{}\n",
            step.index,
            step.target,
            step.established,
            step.held,
            ms(percentile(&step.setup, 0.5)),
            ms(percentile(&step.setup, 0.95)),
            ms(percentile(&step.setup, 0.99)),
            ms(step.rtt),
            step.audio_observed,
            if step.audio_observed { format!("{:.5}", step.loss) } else { "null".to_owned() },
            if step.audio_observed {
                format!("{:.2}", step.jitter.as_secs_f64() * 1000.0)
            } else {
                "null".to_owned()
            },
            if step.audio_observed { format!("{:.3}", step.mos) } else { "null".to_owned() },
            step.packets,
            step.discards,
            step.rss_bytes
                .map_or_else(|| "null".to_owned(), |b| b.to_string()),
            step.failures.len(),
            step.wall.as_secs_f64(),
            step.sample_wall.as_secs_f64(),
            step.overhead.as_secs_f64(),
            step.drift.as_secs_f64(),
            if index + 1 == steps.len() { "" } else { "," },
        ));
    }
    out.push_str("  ]\n}\n");
    out
}

/// One second of 440 Hz, so a call carries audio a receiver can measure loss and jitter on.
fn tone(rate: usize) -> Arc<Vec<i16>> {
    Arc::new(
        (0..rate)
            .map(|i| {
                let t = i as f64 / rate as f64;
                ((t * 440.0 * std::f64::consts::TAU).sin() * 8_000.0) as i16
            })
            .collect(),
    )
}

/// A server that answers everything and holds each dialog until the caller ends it.
async fn start_server(
    tone: Arc<Vec<i16>>,
    media_mode: Media,
    running: Arc<AtomicBool>,
) -> Result<SocketAddr, Box<dyn std::error::Error>> {
    let (endpoint, incoming) = bind(Config::new("127.0.0.1:0".parse()?)).await?;
    let address = endpoint.local_addr();

    tokio::spawn(async move {
        let mut dispatcher = Dispatcher::new(endpoint.clone(), incoming);
        let calls = dispatcher.calls();
        while let Some(event) = dispatcher.next().await {
            let Dispatched::Invitation(invitation) = event else {
                continue;
            };
            if media_mode == Media::None {
                // The answering half of the same media-free shape the client places: no SDP in the
                // 2xx and no media session behind it, so *both* ends of this run are without one.
                let contact = format!("<sip:capacity@{}>", endpoint.advertised());
                match invitation.answer_signalling(&endpoint, contact).await {
                    Ok(mut call) => {
                        let calls = calls.clone();
                        tokio::spawn(async move {
                            // Drives the 2xx retransmission until the ACK, validates in-dialog
                            // requests, and answers the caller's BYE. It ends when the dialog does.
                            while call.next().await.is_some() {}
                            calls.forget(call.dialog());
                        });
                    }
                    Err(error) => eprintln!("server could not answer: {error}"),
                }
                continue;
            }
            match invitation.answer(&endpoint, LOOPBACK).await {
                Ok(mut call) => {
                    let (_, mut requests) = invitation.into_parts();
                    // The server talks back. Without this the client's receive stream is empty and
                    // every audio-quality figure is computed from nothing — which is how the first
                    // run of this example reported a perfect MOS over zero packets.
                    let playing = (media_mode == Media::Full).then(|| {
                        let media = call.media_handle();
                        let tone = Arc::clone(&tone);
                        let running = Arc::clone(&running);
                        // The same two ways out as the client's. This side needs no separate
                        // "the call ended" flag: the caller hangs up first, and `play` reports a
                        // stopped session by answering `false`.
                        tokio::spawn(async move {
                            while running.load(Ordering::Relaxed) && media.play(&tone, FRAME).await
                            {
                            }
                        })
                    });
                    tokio::spawn(async move {
                        let _ = serve(&mut call, &mut requests).await;
                        if let Some(playing) = playing {
                            let _ = playing.await;
                        }
                    });
                }
                Err(error) => eprintln!("server could not answer: {error}"),
            }
        }
    });
    Ok(address)
}

/// What the command line asked for.
struct Options {
    ramp: Vec<usize>,
    dwell: Duration,
    media: Media,
    server: Option<SocketAddr>,
    record: Option<String>,
}

impl Options {
    fn parse(args: impl Iterator<Item = String>) -> Result<Self, Box<dyn std::error::Error>> {
        let mut options = Self {
            // 50 first to establish a floor with the machine quiet, then 500 at a time — the shape
            // that finds a ceiling in a few minutes rather than a few hours.
            ramp: vec![50, 500, 500, 500],
            dwell: Duration::from_secs(10),
            media: Media::Full,
            server: None,
            record: None,
        };
        let mut args = args.peekable();
        while let Some(flag) = args.next() {
            let mut value = || -> Result<String, String> {
                args.next().ok_or_else(|| format!("{flag} needs a value"))
            };
            match flag.as_str() {
                "--ramp" => {
                    options.ramp = value()?
                        .split(',')
                        .map(str::parse)
                        .collect::<Result<_, _>>()?;
                }
                "--dwell" => options.dwell = Duration::from_secs(value()?.parse()?),
                "--media" => {
                    options.media = match value()?.as_str() {
                        "full" => Media::Full,
                        "idle" => Media::Idle,
                        "none" => Media::None,
                        other => {
                            return Err(
                                format!("--media takes full, idle or none, not {other}").into()
                            );
                        }
                    };
                }
                "--server" => options.server = Some(value()?.parse()?),
                "--record" => options.record = Some(value()?),
                "--help" | "-h" => {
                    println!(
                        "usage: cargo run --release --example capacity_test -- \\\n  \
                         [--ramp 50,500,500] [--dwell SECONDS] [--media full|idle|none] \\\n  \
                         [--server ADDR] [--record PATH]"
                    );
                    std::process::exit(0);
                }
                other => return Err(format!("unknown option {other}").into()),
            }
        }
        if options.ramp.is_empty() {
            return Err("--ramp needs at least one step".into());
        }
        Ok(options)
    }
}
