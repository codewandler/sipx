//! `sipx load` — finite, reproducible call admission with joined cleanup.

use std::collections::BTreeMap;
use std::net::IpAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use sipx_call::{Credentials, DialOptions};
use sipx_sip::{Address, Uri};
use sipx_testkit::load::{AdmissionEnd, BoundedPlan, Cause, Stop, run_bounded};
use sipx_transport::{Config as TransportConfig, bind};

use crate::output::{Exit, Format, fail};

pub(crate) const HELP: &str = "\
sipx load — place a finite, reproducible call load

USAGE:
    sipx load <URI> --rate <CALLS/S> --concurrency <N> (--calls <N> | --duration <S>) [OPTIONS]

ARGS:
    <URI>             Target called by every admitted call

REQUIRED OPTIONS:
    --rate <CALLS/S>  Positive finite arrival rate
    --concurrency <N> Positive maximum active calls

BOUNDS:
    --calls <N>       Stop after admitting this many calls
    --duration <S>    Stop admission after this many seconds
    --call-duration <S> End each answered call after this many seconds (default 0)
    --timeout <S>     Bound each call setup (default 20)

REPRODUCIBILITY:
    --seed <N>        Arrival-jitter and media seed (default 0)

CALL OPTIONS:
    --from <URI>      Our own address (default sip:sipx@<local>)
    --password <P>    Password. Prefer SIPX_PASSWORD, since argv is world-readable
    --local <ADDR>    Local address to bind (default 0.0.0.0:0)
    --transport <T>   Signalling: udp, tcp, tls, ws or wss (default udp)
    --tcp             Legacy alias for --transport tcp
    --tls-server-name <N>  Certificate identity to verify (default URI host)
    --tls-ca <FILE>   Add PEM trust roots to the platform store
    --tls-cert <FILE> Client certificate chain for mutual TLS (with --tls-key)
    --tls-key <FILE>  Client private key for mutual TLS (with --tls-cert)
    --json            Emit the stable sipx.load.v1 summary as one JSON line
";

const CLEANUP: Duration = Duration::from_secs(40);

#[derive(Debug, Clone, Copy)]
struct Limits {
    rate: f64,
    concurrency: usize,
    calls: Option<usize>,
    duration: Option<Duration>,
    call_duration: Duration,
    setup_timeout: Duration,
    seed: u64,
}

impl Limits {
    fn parse(args: &crate::Args<'_>) -> Result<Self, String> {
        let rate = parse_positive_f64(args.value("rate"), "--rate")?;
        let interval = Duration::try_from_secs_f64(1.0 / rate)
            .map_err(|_| "--rate cannot be represented by the scheduler clock".to_owned())?;
        if interval.is_zero() {
            return Err("--rate is faster than the scheduler clock can represent".to_owned());
        }
        let concurrency = parse_positive_usize(args.value("concurrency"), "--concurrency")?;
        if concurrency > tokio::sync::Semaphore::MAX_PERMITS {
            return Err(format!(
                "--concurrency must not exceed {}",
                tokio::sync::Semaphore::MAX_PERMITS
            ));
        }
        let calls = args
            .value("calls")
            .map(|value| parse_positive_usize(Some(value), "--calls"))
            .transpose()?;
        let duration = args.number("duration").map(Duration::from_secs);
        if duration.is_some_and(|value| value.is_zero()) {
            return Err("--duration must be greater than zero for load admission".to_owned());
        }
        if duration.is_some_and(|value| tokio::time::Instant::now().checked_add(value).is_none()) {
            return Err("--duration exceeds the scheduler clock's range".to_owned());
        }
        if calls.is_none() && duration.is_none() {
            return Err(
                "load requires at least one finite bound: --calls or --duration".to_owned(),
            );
        }
        let seed = parse_u64(args.value("seed").unwrap_or("0"), "--seed")?;
        let call_duration = Duration::from_secs(args.number("call-duration").unwrap_or(0));
        let setup_timeout = Duration::from_secs(args.number("timeout").unwrap_or(20));

        Ok(Self {
            rate,
            concurrency,
            calls,
            duration,
            call_duration,
            setup_timeout,
            seed,
        })
    }
}

fn parse_positive_f64(value: Option<&str>, flag: &str) -> Result<f64, String> {
    let Some(value) = value else {
        return Err(format!("{flag} is required"));
    };
    let parsed = value
        .parse::<f64>()
        .map_err(|_| format!("{flag} must be a positive finite number, not {value:?}"))?;
    if !parsed.is_finite() || parsed <= 0.0 {
        return Err(format!(
            "{flag} must be a positive finite number, not {value:?}"
        ));
    }
    Ok(parsed)
}

fn parse_positive_usize(value: Option<&str>, flag: &str) -> Result<usize, String> {
    let Some(value) = value else {
        return Err(format!("{flag} is required"));
    };
    let parsed = value
        .parse::<usize>()
        .map_err(|_| format!("{flag} must be a positive whole number, not {value:?}"))?;
    if parsed == 0 {
        return Err(format!("{flag} must be greater than zero"));
    }
    Ok(parsed)
}

fn parse_u64(value: &str, flag: &str) -> Result<u64, String> {
    value.parse::<u64>().map_err(|_| {
        format!(
            "{flag} must be a whole number from 0 through {}, not {value:?}",
            u64::MAX
        )
    })
}

#[derive(Debug, Clone, Copy)]
struct Measurement {
    setup: Duration,
    status: u16,
    quality: sipx_rtp::Quality,
}

#[allow(
    clippy::too_many_lines,
    reason = "validation, endpoint construction, owned execution and final reporting stay in lifecycle order"
)]
pub(crate) async fn run(raw: &[String], format: Format) -> Exit {
    let args = match crate::arguments(raw, HELP, format) {
        Ok(args) => args,
        Err(exit) => return exit,
    };
    let Some(uri_text) = args.positional() else {
        eprint!("{HELP}");
        return fail(format, Exit::Usage, "a target URI is required");
    };
    let limits = match Limits::parse(&args) {
        Ok(limits) => limits,
        Err(message) => return fail(format, Exit::Usage, &message),
    };
    let Ok(to) = Uri::parse(bytes::Bytes::from(uri_text.to_owned())) else {
        return fail(format, Exit::Usage, &format!("not a SIP URI: {uri_text}"));
    };
    let transport = match crate::signalling::Selection::from_args(&args, to.scheme().is_secure()) {
        Ok(transport) => transport,
        Err(message) => return fail(format, Exit::Usage, &message),
    };
    let Some((target_addr, server_name)) = crate::dial::target_of(&to, transport.kind()) else {
        return fail(
            format,
            Exit::Usage,
            &format!("{uri_text} must name an address and port"),
        );
    };
    let target = match transport.target(&args, target_addr, &server_name) {
        Ok(target) => target,
        Err(message) => return fail(format, Exit::Usage, &message),
    };
    let Ok(local) = args.value("local").unwrap_or("0.0.0.0:0").parse() else {
        return fail(format, Exit::Usage, "--local must be host:port");
    };
    let media_address: IpAddr = crate::advertise::reachable_ip(local, target_addr.ip());
    let from = args
        .value("from")
        .map_or_else(|| format!("<sip:sipx@{media_address}>"), str::to_owned);
    let credentials = match credentials(&args, &from) {
        Ok(credentials) => credentials,
        Err(message) => return fail(format, Exit::Usage, &message),
    };

    let mut config = TransportConfig::new(local);
    config.sent_by = media_address.to_string();
    if let Err(message) = transport.configure_client(&args, &mut config) {
        return fail(format, Exit::Usage, &message);
    }
    let (handle, _incoming) = match bind(config).await {
        Ok(bound) => bound,
        Err(error) => return fail(format, Exit::Failed, &format!("bind: {error}")),
    };

    let mut options = DialOptions::new(from, media_address);
    if !limits.setup_timeout.is_zero() {
        options = options.with_timeout(limits.setup_timeout);
    }
    if let Some(credentials) = credentials {
        options = options.with_credentials(credentials);
    }

    let stop = Stop::new();
    let interrupt = stop.clone();
    let signal_task = tokio::spawn(async move {
        if tokio::signal::ctrl_c().await.is_ok() {
            interrupt.request();
        }
    });
    let measurements = Arc::new(Mutex::new(Vec::<Measurement>::new()));
    let observed = Arc::clone(&measurements);
    let handle = Arc::new(handle);
    let to = Arc::new(to);
    let options = Arc::new(options);
    let target = Arc::new(target);

    let bounded = run_bounded(
        BoundedPlan {
            calls: limits.calls,
            duration: limits.duration,
            rate: limits.rate,
            seed: limits.seed,
            most_in_flight: limits.concurrency,
            cleanup: CLEANUP,
        },
        stop,
        move |index, stop| {
            let handle = Arc::clone(&handle);
            let to = Arc::clone(&to);
            let options = Arc::clone(&options);
            let target = Arc::clone(&target);
            let measurements = Arc::clone(&observed);
            async move {
                let started = tokio::time::Instant::now();
                let mut call = sipx_call::dial_until(
                    &handle,
                    (*target).clone(),
                    &to,
                    &options,
                    stop.requested(),
                )
                .await
                .map_err(|error| {
                    let cause = classify(error);
                    if matches!(&cause, Cause::Other(_)) {
                        stop.request();
                    }
                    cause
                })?;
                let setup = started.elapsed();
                let status = call.initial_status();
                // One bounded packet is enough to make media deterministic and observable without
                // allocating in proportion to an operator-supplied call duration.
                let frame = deterministic_frame(limits.seed, index);
                let played = call.media().play(&frame, frame.len()).await;
                if !limits.call_duration.is_zero() {
                    tokio::select! {
                        () = stop.requested() => {}
                        () = tokio::time::sleep(limits.call_duration) => {}
                    }
                }
                let quality = call.media().quality().await;
                let hung_up = call.hang_up().await;
                let measurement = Measurement {
                    setup,
                    status,
                    quality,
                };
                let Ok(mut measurements) = measurements.lock() else {
                    stop.request();
                    return Err(Cause::Other("measurement store poisoned".to_owned()));
                };
                measurements.push(measurement);
                drop(measurements);
                if let Err(error) = hung_up {
                    stop.request();
                    return Err(Cause::Other(format!("hang up failed: {error}")));
                }
                if !played {
                    stop.request();
                    return Err(Cause::Other("media playback failed".to_owned()));
                }
                Ok(())
            }
        },
    )
    .await;

    signal_task.abort();
    let _ = signal_task.await;
    let measurements = match measurements.lock() {
        Ok(values) => values.clone(),
        Err(_) => return fail(format, Exit::Failed, "measurement store poisoned"),
    };
    emit_summary(format, uri_text, limits, &bounded, &measurements);

    if !bounded.cleanup_complete || has_internal_failure(&bounded.outcome.failures) {
        Exit::Failed
    } else {
        Exit::Success
    }
}

fn deterministic_frame(seed: u64, index: usize) -> [i16; 160] {
    let mut state = seed ^ u64::try_from(index).unwrap_or(u64::MAX).rotate_left(17);
    let mut frame = [0i16; 160];
    for sample in &mut frame {
        state = state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        let [high, low, ..] = state.to_be_bytes();
        *sample = i16::from_be_bytes([high, low]);
    }
    frame
}

fn credentials(args: &crate::Args<'_>, from: &str) -> Result<Option<Credentials>, String> {
    let password = args
        .value("password")
        .map(str::to_owned)
        .or_else(|| std::env::var("SIPX_PASSWORD").ok());
    let Some(password) = password else {
        return Ok(None);
    };
    let username = Address::parse(from.as_bytes(), "From")
        .ok()
        .and_then(|address| address.uri.decoded_user())
        .map(|user| String::from_utf8_lossy(&user).into_owned())
        .ok_or_else(|| "--password requires --from to contain a SIP username".to_owned())?;
    Ok(Some(Credentials::new(username, password)))
}

fn classify(error: sipx_call::Error) -> Cause {
    match error {
        sipx_call::Error::Rejected { status, .. } => Cause::Rejected(status),
        sipx_call::Error::Cancelled(_) | sipx_call::Error::NoResponse => Cause::Timeout,
        sipx_call::Error::Transport(_) | sipx_call::Error::Io(_) => Cause::Transport,
        other => Cause::Other(other.to_string()),
    }
}

fn has_internal_failure(failures: &BTreeMap<Cause, usize>) -> bool {
    failures.keys().any(|cause| {
        matches!(
            cause,
            Cause::Other(message)
                if message == "panicked"
                    || message == "cancelled"
                    || message == "cleanup budget exhausted"
                    || message == "measurement store poisoned"
                    || message == "media playback failed"
                    || message.starts_with("hang up failed:")
        )
    })
}

fn response_counts(
    outcome: &sipx_testkit::load::Outcome,
    measurements: &[Measurement],
) -> BTreeMap<String, usize> {
    let mut responses = BTreeMap::<String, usize>::new();
    for measurement in measurements {
        *responses.entry(measurement.status.to_string()).or_default() += 1;
    }
    for (cause, count) in &outcome.failures {
        if let Cause::Rejected(status) = cause {
            *responses.entry(status.to_string()).or_default() += count;
        }
    }
    responses
}

fn percentile(values: &[Duration], numerator: usize, denominator: usize) -> Option<u64> {
    if values.is_empty() {
        return None;
    }
    let mut sorted = values.to_vec();
    sorted.sort_unstable();
    let rank = sorted
        .len()
        .saturating_mul(numerator)
        .saturating_add(denominator.saturating_sub(1))
        / denominator.max(1);
    sorted
        .get(rank.saturating_sub(1).min(sorted.len() - 1))
        .map(|duration| u64::try_from(duration.as_millis()).unwrap_or(u64::MAX))
}

#[allow(
    clippy::cast_precision_loss,
    reason = "a process cannot collect enough media snapshots to exceed f64's exact integer range"
)]
fn emit_summary(
    format: Format,
    target: &str,
    limits: Limits,
    bounded: &sipx_testkit::load::BoundedOutcome,
    measurements: &[Measurement],
) {
    let outcome = &bounded.outcome;
    let rejected: usize = outcome
        .failures
        .iter()
        .filter_map(|(cause, count)| matches!(cause, Cause::Rejected(_)).then_some(*count))
        .sum();
    let timed_out = outcome.failures.get(&Cause::Timeout).copied().unwrap_or(0);
    let failed = outcome.failed().saturating_sub(rejected + timed_out);
    let connected = measurements.len();
    let responses = response_counts(outcome, measurements);
    let setup: Vec<_> = measurements.iter().map(|value| value.setup).collect();
    let snapshots = measurements.len();
    let packets_lost: i64 = measurements
        .iter()
        .map(|value| value.quality.cumulative_lost)
        .sum();
    let divisor = snapshots as f64;
    let mean = |value: fn(&Measurement) -> f64| {
        (snapshots > 0).then(|| measurements.iter().map(value).sum::<f64>() / divisor)
    };
    let status = if !bounded.cleanup_complete || has_internal_failure(&outcome.failures) {
        "failed"
    } else if bounded.admission_end == AdmissionEnd::Requested {
        "interrupted"
    } else {
        "completed"
    };
    let summary = serde_json::json!({
        "schema": "sipx.load.v1",
        "status": status,
        "seed": limits.seed,
        "target": target,
        "limits": {
            "rate": limits.rate,
            "concurrency": limits.concurrency,
            "calls": limits.calls,
            "duration_ms": limits.duration.map(|value| u64::try_from(value.as_millis()).unwrap_or(u64::MAX)),
            "call_duration_ms": u64::try_from(limits.call_duration.as_millis()).unwrap_or(u64::MAX),
            "setup_timeout_ms": u64::try_from(limits.setup_timeout.as_millis()).unwrap_or(u64::MAX),
            "cleanup_ms": u64::try_from(CLEANUP.as_millis()).unwrap_or(u64::MAX),
        },
        "outcomes": {
            "attempted": outcome.attempted,
            "connected": connected,
            "rejected": rejected,
            "timed_out": timed_out,
            "failed": failed,
            "peak_concurrency": bounded.peak_in_flight,
        },
        "response_codes": responses,
        "setup_ms": {
            "p50": percentile(&setup, 50, 100),
            "p95": percentile(&setup, 95, 100),
            "p99": percentile(&setup, 99, 100),
        },
        "media": {
            "snapshots": snapshots,
            "packets_lost": packets_lost,
            "mean_loss": mean(|value| value.quality.loss),
            "mean_jitter_ms": mean(|value| value.quality.jitter.as_secs_f64() * 1000.0),
            "mean_mos": mean(|value| value.quality.mos),
        }
    });

    match format {
        Format::Json => println!("{summary}"),
        Format::Text => {
            println!("status             {status}");
            println!("target             {target}");
            println!("seed               {}", limits.seed);
            println!("attempted          {}", outcome.attempted);
            println!("connected          {connected}");
            println!("rejected           {rejected}");
            println!("timed_out          {timed_out}");
            println!("failed             {failed}");
            println!("peak_concurrency   {}", bounded.peak_in_flight);
            println!("summary_json       {summary}");
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    fn raw(items: &[&str]) -> Vec<String> {
        items.iter().map(|item| (*item).to_owned()).collect()
    }

    #[test]
    fn every_load_plan_has_finite_admission_and_cleanup_bounds() {
        let unbounded = raw(&[
            "load",
            "sip:a@127.0.0.1",
            "--rate",
            "2",
            "--concurrency",
            "3",
        ]);
        let args = crate::Args::new(&unbounded).expect("argument shape");
        assert!(Limits::parse(&args).is_err());

        let bounded = raw(&[
            "load",
            "sip:a@127.0.0.1",
            "--rate",
            "2",
            "--concurrency",
            "3",
            "--calls",
            "4",
        ]);
        let args = crate::Args::new(&bounded).expect("argument shape");
        let limits = Limits::parse(&args).expect("finite plan");
        assert_eq!(limits.calls, Some(4));
        assert_eq!(CLEANUP, Duration::from_secs(40));
    }

    #[test]
    fn unsafe_or_nonsensical_rates_and_limits_are_refused_before_io() {
        for values in [
            raw(&[
                "load",
                "sip:a@127.0.0.1",
                "--rate",
                "0",
                "--concurrency",
                "3",
                "--calls",
                "4",
            ]),
            raw(&[
                "load",
                "sip:a@127.0.0.1",
                "--rate",
                "NaN",
                "--concurrency",
                "3",
                "--calls",
                "4",
            ]),
            raw(&[
                "load",
                "sip:a@127.0.0.1",
                "--rate",
                "inf",
                "--concurrency",
                "3",
                "--calls",
                "4",
            ]),
            raw(&[
                "load",
                "sip:a@127.0.0.1",
                "--rate",
                "1e-300",
                "--concurrency",
                "3",
                "--calls",
                "4",
            ]),
            raw(&[
                "load",
                "sip:a@127.0.0.1",
                "--rate",
                "1e300",
                "--concurrency",
                "3",
                "--calls",
                "4",
            ]),
            raw(&[
                "load",
                "sip:a@127.0.0.1",
                "--rate",
                "2",
                "--concurrency",
                "0",
                "--calls",
                "4",
            ]),
            raw(&[
                "load",
                "sip:a@127.0.0.1",
                "--rate",
                "2",
                "--concurrency",
                "3",
                "--calls",
                "0",
            ]),
        ] {
            let args = crate::Args::new(&values).expect("argument shape");
            assert!(Limits::parse(&args).is_err(), "{values:?}");
        }

        let excessive = raw(&[
            "load",
            "sip:a@127.0.0.1",
            "--rate",
            "2",
            "--concurrency",
            &tokio::sync::Semaphore::MAX_PERMITS
                .saturating_add(1)
                .to_string(),
            "--calls",
            "4",
        ]);
        let args = crate::Args::new(&excessive).expect("argument shape");
        assert!(Limits::parse(&args).is_err(), "{excessive:?}");
    }

    #[test]
    fn response_counts_use_the_success_status_that_arrived() {
        let measurements = [Measurement {
            setup: Duration::from_millis(2),
            status: 202,
            quality: sipx_rtp::Quality {
                loss: 0.0,
                cumulative_lost: 0,
                jitter: Duration::ZERO,
                round_trip: None,
                mos: 4.4,
            },
        }];
        let mut outcome = sipx_testkit::load::Outcome::default();
        outcome.failures.insert(Cause::Rejected(486), 2);

        let responses = response_counts(&outcome, &measurements);

        assert_eq!(responses.get("202"), Some(&1));
        assert_eq!(responses.get("486"), Some(&2));
        assert!(!responses.contains_key("200"));
    }

    #[test]
    fn seed_and_call_index_reproduce_media_without_repeating_every_call() {
        assert_eq!(deterministic_frame(41, 2), deterministic_frame(41, 2));
        assert_ne!(deterministic_frame(41, 2), deterministic_frame(42, 2));
        assert_ne!(deterministic_frame(41, 2), deterministic_frame(41, 3));
    }
}
