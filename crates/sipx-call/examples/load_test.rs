//! Run a load test: a sipx client placing calls against a sipx server, in one process.
//!
//! Both ends are real. The server binds an endpoint, answers every INVITE it receives and serves
//! the dialog until the caller hangs up; the client drives [`sipx_call::load`]'s scheduler, which
//! places calls at a **stated arrival rate** and reports what happened to each.
//!
//! ```text
//! cargo run --example load_test
//! cargo run --example load_test -- --calls 200 --rate 50 --in-flight 40
//! cargo run --example load_test -- --calls 200 --rate 50 --media none
//! ```
//!
//! `--media none` places and answers calls that negotiate no session at all — no SDP offer, no RTP
//! socket, on both ends — so the setup latency it reports is the signalling round trip without the
//! media stack underneath it. `full`, the default, is a negotiated session as before. There is no
//! `idle` here: this example holds a call for [`HOLD`] and measures setup, and a session that is
//! negotiated but silent costs the same to set up as one that is not.
//!
//! To point the same client at somebody else's SIP server instead of the one this example starts,
//! give it an address and no server is started:
//!
//! ```text
//! cargo run --example load_test -- --calls 100 --rate 20 --server 192.0.2.10:5060
//! ```
//!
//! # What the numbers mean, and what they do not
//!
//! **Rate, not concurrency.** `--rate` is how many calls are *started* per second. A harness that
//! instead keeps N calls in flight speeds up when the system under test slows down, which is the
//! opposite of what a load test is for. `--in-flight` is only a backstop, so a server that has
//! stopped answering entirely cannot make this process run out of sockets before it does.
//!
//! **Read the latency figures as the whole call, not as signalling alone.** The scheduler times the
//! work it was given, and the work this example gives it is dial → hold → hang up. So the printed
//! `setup` percentiles include [`HOLD`]: subtract it to get the signalling round trip, or pass
//! `--hold 0` to measure setup by itself. The percentiles are nearest-rank, so every figure printed
//! is a call that actually happened rather than an interpolation between two that did.
//!
//! **Failures are never summed.** `4 × timeout` and `4 × rejected 486` are different facts about a
//! server and one number would hide which you have.
//!
//! **This is not a benchmark of sipx against anything.** With both ends in one process on one
//! machine, what you are measuring includes this process's own scheduler. It is useful for
//! exercising a server, for watching a change move a percentile, and for producing load on demand
//! — not for a published throughput figure.

// These samples are read by people before they are run by machines, so they are written for
// readability where the workspace lints would prefer something terser.
#![allow(clippy::cast_possible_truncation, clippy::cast_precision_loss)]

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use sipx_call::load::{Cause, Plan, run};
use sipx_call::{
    DialOptions, Dispatched, Dispatcher, SignallingDialOptions, SignallingIdentity, dial,
    dial_signalling, serve,
};
use sipx_sip::{Host, HostName, Uri};
use sipx_transport::{Config, Target, bind};

/// Whether a call negotiates a media session at all.
///
/// `None` is not "a session that sends nothing": there is no SDP offer, so no port is named, so no
/// RTP socket is bound and no jitter buffer exists on either end. It is the shape a deployment that
/// runs its media elsewhere actually has, and it reaches it through
/// [`sipx_call::dial_signalling`] — the same path `sipx load --mode signalling` uses (`T-46`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Media {
    /// A negotiated session, as every call here had before `T-46`.
    Full,
    /// No session: no SDP, no RTP socket.
    None,
}

/// How long a placed call stays up before the client hangs it up, unless `--hold` says otherwise.
///
/// Short, because what this example exercises is call *setup* — the signalling round trip and the
/// dialog it establishes. A longer hold measures how many concurrent dialogs the machine can carry,
/// which is a different question and one `--in-flight` is the knob for.
const HOLD: Duration = Duration::from_millis(200);

/// How long a call may take to set up before it counts as a timeout rather than a slow success.
///
/// Without a bound the attempt runs until the transaction layer gives up at 32 seconds, and a run
/// against an unresponsive server would take longer to report than it did to fail.
const SETUP_BOUND: Duration = Duration::from_secs(5);

/// The address both ends use. Written once so the client and the server cannot disagree about it.
const LOOPBACK: std::net::IpAddr = std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST);

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let options = Options::parse(std::env::args().skip(1))?;

    // The server. Started here unless one was named, so the example runs with no arguments and no
    // other process — which is the difference between a sample somebody reads and one they run.
    let server = if let Some(address) = options.server {
        println!("client only: placing calls against {address}");
        address
    } else {
        let address = start_server(options.media).await?;
        println!("server listening on {address}");
        address
    };

    // The client. One endpoint, shared by every call the plan places: a load generator that bound a
    // socket per call would be measuring the operating system as much as the server.
    let (endpoint, _incoming) = bind(Config::new("127.0.0.1:0".parse()?)).await?;
    let endpoint = Arc::new(endpoint);

    let plan = Plan {
        calls: options.calls,
        rate: options.rate,
        most_in_flight: options.in_flight,
    };
    println!(
        "placing {} calls at {}/s, at most {} in flight",
        plan.calls, plan.rate, plan.most_in_flight
    );

    let hold = options.hold;
    let media = options.media;
    let outcome = run(plan, move |_index| {
        let endpoint = Arc::clone(&endpoint);
        async move { place_one(&endpoint, server, hold, media).await }
    })
    .await;

    // `report` prints the attempt count, the successes, every failure cause on its own line, and
    // the setup-latency percentiles.
    print!("\n{}", outcome.report());
    println!("{:.1} calls/s completed", outcome.calls_per_second());

    // A run whose calls all failed is not a successful run, however cleanly the harness finished.
    // Exiting non-zero is what lets this be used from a script.
    if outcome.succeeded == 0 && outcome.attempted > 0 {
        return Err("every call failed".into());
    }
    Ok(())
}

/// One call: dial, hold briefly, hang up. The `Result` is what the scheduler records.
///
/// Every failure is mapped to a [`Cause`] rather than to a message, because the point of the run is
/// which *kind* of failure a server produces under load — a refusal, a timeout and a dead socket
/// are three different problems and only the first is the server working as designed.
async fn place_one(
    endpoint: &sipx_transport::Handle,
    server: SocketAddr,
    hold: Duration,
    media: Media,
) -> Result<(), Cause> {
    let to = Uri::sip(Host::Name(
        HostName::new("callee.example").map_err(|error| Cause::Other(error.to_string()))?,
    ));
    if media == Media::None {
        let from = Uri::parse(Bytes::from_static(b"sip:load@example.net"))
            .map_err(|error| Cause::Other(error.to_string()))?;
        let options = SignallingDialOptions::new(SignallingIdentity::fresh(endpoint, &to, &from))
            .with_timeout(SETUP_BOUND);
        let mut call = dial_signalling(endpoint, Target::udp(server), &to, &options)
            .await
            .map_err(|error| classify(&error))?;
        tokio::time::sleep(hold).await;
        // The same failure bound the media half's `hang_up` carries, stated here because the
        // signalling one takes it as an argument.
        call.hang_up(SETUP_BOUND)
            .await
            .map_err(|error| classify(&error))?;
        return Ok(());
    }
    let dial_options =
        DialOptions::new("<sip:load@example.net>", LOOPBACK).with_timeout(SETUP_BOUND);

    let mut call = dial(endpoint, Target::udp(server), &to, &dial_options)
        .await
        .map_err(|error| classify(&error))?;

    // Hold the dialog open, then end it properly. A load test that abandoned its calls would leave
    // the server carrying dialogs that no BYE ever closes, and the next run would measure that
    // rather than the server.
    tokio::time::sleep(hold).await;
    call.hang_up().await.map_err(|error| classify(&error))?;
    Ok(())
}

/// Turn a dial failure into the cause the scheduler counts.
fn classify(error: &sipx_call::Error) -> Cause {
    match error {
        sipx_call::Error::Rejected { status, .. } => Cause::Rejected(*status),
        // The far end said nothing at all, which is a different fact about a server than a
        // refusal: one of them is the server working. A BYE nobody answered inside its bound is
        // the same fact about the same server, one request later.
        sipx_call::Error::NoResponse | sipx_call::Error::SignallingTeardownTimeout(_) => {
            Cause::Timeout
        }
        sipx_call::Error::Transport(_) | sipx_call::Error::Io(_) => Cause::Transport,
        other => Cause::Other(other.to_string()),
    }
}

/// Bind a server and answer every call it is offered, until the process ends.
///
/// Returns the address it bound. Each call is served on its own task so a slow one cannot hold up
/// the next INVITE — a server that answered serially would make this example measure a queue.
async fn start_server(media: Media) -> Result<SocketAddr, Box<dyn std::error::Error>> {
    let (endpoint, incoming) = bind(Config::new("127.0.0.1:0".parse()?)).await?;
    let address = endpoint.local_addr();

    tokio::spawn(async move {
        // The dispatcher routes each dialog's own requests to that dialog. Without it a server
        // reads one queue for every call at once, and answering them serially would make this
        // example measure a queue rather than a server.
        let mut dispatcher = Dispatcher::new(endpoint.clone(), incoming);
        let calls = dispatcher.calls();
        while let Some(event) = dispatcher.next().await {
            let Dispatched::Invitation(invitation) = event else {
                continue;
            };
            if media == Media::None {
                // Answers the caller's shape: a 2xx with no SDP and no media session behind it, so
                // the run has no media stack at *either* end rather than only at the client's.
                let contact = format!("<sip:load@{}>", endpoint.advertised());
                match invitation.answer_signalling(&endpoint, contact).await {
                    Ok(mut call) => {
                        let calls = calls.clone();
                        tokio::spawn(async move {
                            while call.next().await.is_some() {}
                            calls.forget(call.dialog());
                        });
                    }
                    Err(error) => eprintln!("server could not answer: {error}"),
                }
                continue;
            }
            let answered = invitation.answer(&endpoint, LOOPBACK).await;
            match answered {
                Ok(mut call) => {
                    let (_, mut requests) = invitation.into_parts();
                    // `serve` answers the BYE and stops the media when the caller hangs up. One
                    // task per call, so a slow teardown cannot hold up the next INVITE.
                    tokio::spawn(async move {
                        let _ = serve(&mut call, &mut requests).await;
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
    calls: usize,
    hold: Duration,
    rate: f64,
    in_flight: usize,
    media: Media,
    server: Option<SocketAddr>,
}

impl Options {
    /// Defaults chosen to finish in a couple of seconds on any machine, so the no-argument run is
    /// worth watching rather than worth interrupting.
    fn parse(args: impl Iterator<Item = String>) -> Result<Self, Box<dyn std::error::Error>> {
        let mut options = Self {
            calls: 50,
            hold: HOLD,
            rate: 25.0,
            in_flight: 16,
            media: Media::Full,
            server: None,
        };
        let mut args = args.peekable();
        while let Some(flag) = args.next() {
            let mut value = || -> Result<String, String> {
                args.next().ok_or_else(|| format!("{flag} needs a value"))
            };
            match flag.as_str() {
                "--calls" => options.calls = value()?.parse()?,
                "--rate" => options.rate = value()?.parse()?,
                "--in-flight" => options.in_flight = value()?.parse()?,
                "--hold" => options.hold = Duration::from_millis(value()?.parse()?),
                "--media" => {
                    options.media = match value()?.as_str() {
                        "full" => Media::Full,
                        "none" => Media::None,
                        other => {
                            return Err(format!("--media takes full or none, not {other}").into());
                        }
                    };
                }
                "--server" => options.server = Some(value()?.parse()?),
                "--help" | "-h" => {
                    println!(
                        "usage: cargo run --example load_test -- \
                         [--calls N] [--rate PER_SECOND] [--in-flight N] \
                         [--hold MILLISECONDS] [--media full|none] [--server ADDR]"
                    );
                    std::process::exit(0);
                }
                other => return Err(format!("unknown option {other}").into()),
            }
        }
        Ok(options)
    }
}
