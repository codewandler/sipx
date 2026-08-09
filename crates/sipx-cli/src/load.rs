//! `sipx load` — finite, reproducible call admission with joined cleanup.

use std::collections::BTreeMap;
use std::net::IpAddr;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use bytes::Bytes;
use sipx_call::load::{AdmissionEnd, BoundedPlan, Cause, Stop, run_bounded};
use sipx_call::{Credentials, DialOptions};
use sipx_sip::build::RequestBuilder;
use sipx_sip::headers::CSeq;
use sipx_sip::{Address, HeaderName, Method, Request, Response, Uri};
use sipx_transport::{Config as TransportConfig, bind};

use crate::cli::{LoadOptions, WorkloadMode};
use crate::output::{Exit, Format, fail};

const CLEANUP: Duration = Duration::from_secs(40);
const WORKLOAD_MODE_FIELD: &[u8] = b"X-Sipx-Workload-Mode";
pub(crate) const MODE_MISMATCH_REASON: &str = "Workload Mode Mismatch";

pub(crate) fn workload_mode_name() -> HeaderName {
    HeaderName::Other(Bytes::from_static(WORKLOAD_MODE_FIELD))
}

pub(crate) fn requested_mode(request: &Request) -> Result<Option<WorkloadMode>, String> {
    let name = workload_mode_name();
    match request.headers.count(&name) {
        0 => Ok(None),
        1 => {
            let value = request
                .headers
                .value(&name)
                .ok_or_else(|| "workload mode field disappeared".to_owned())?;
            match value.as_ref() {
                b"signalling" => Ok(Some(WorkloadMode::Signalling)),
                b"generated-media" => Ok(Some(WorkloadMode::GeneratedMedia)),
                _ => Err(format!(
                    "invalid X-Sipx-Workload-Mode value {:?}",
                    String::from_utf8_lossy(&value)
                )),
            }
        }
        _ => Err("repeated X-Sipx-Workload-Mode field".to_owned()),
    }
}

#[derive(Debug, Clone, Copy)]
struct Limits {
    rate: f64,
    concurrency: usize,
    calls: Option<usize>,
    duration: Option<Duration>,
    call_duration: Duration,
    setup_timeout: Duration,
    seed: u64,
    mode: WorkloadMode,
}

impl Limits {
    fn parse(options: &LoadOptions) -> Result<Self, String> {
        let rate = positive_f64(options.rate, "--rate")?;
        let interval = Duration::try_from_secs_f64(1.0 / rate)
            .map_err(|_| "--rate cannot be represented by the scheduler clock".to_owned())?;
        if interval.is_zero() {
            return Err("--rate is faster than the scheduler clock can represent".to_owned());
        }
        let concurrency = positive_usize(options.concurrency, "--concurrency")?;
        if concurrency > tokio::sync::Semaphore::MAX_PERMITS {
            return Err(format!(
                "--concurrency must not exceed {}",
                tokio::sync::Semaphore::MAX_PERMITS
            ));
        }
        let calls = options
            .calls
            .map(|value| positive_usize(value, "--calls"))
            .transpose()?;
        let duration = options.duration.map(Duration::from_secs);
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
        let seed = options.seed;
        let call_duration = Duration::from_secs(options.call_duration);
        let setup_timeout = Duration::from_secs(options.timeout);

        Ok(Self {
            rate,
            concurrency,
            calls,
            duration,
            call_duration,
            setup_timeout,
            seed,
            mode: options.mode,
        })
    }

    /// The ceiling over the one lookup this run's calls share: the lower of the two bounds the
    /// run actually stated, or neither when it stated neither.
    ///
    /// `P-29`: `--timeout` alone is the wrong ceiling, because it bounds a *call* and `--duration`
    /// bounds the *run*. A run given `--duration 2 --timeout 20` could spend `T-38`'s whole eight
    /// seconds resolving — four times its own bound — before placing the first call, and the flag
    /// that could have seen it was the one nobody consults.
    fn resolution_bound(&self) -> Option<Duration> {
        let setup = (!self.setup_timeout.is_zero()).then_some(self.setup_timeout);
        match (setup, self.duration) {
            (Some(setup), Some(duration)) => Some(setup.min(duration)),
            (stated, None) | (None, stated) => stated,
        }
    }
}

fn positive_f64(value: f64, flag: &str) -> Result<f64, String> {
    if !value.is_finite() || value <= 0.0 {
        return Err(format!(
            "{flag} must be a positive finite number, not {value:?}"
        ));
    }
    Ok(value)
}

fn positive_usize(value: usize, flag: &str) -> Result<usize, String> {
    if value == 0 {
        return Err(format!("{flag} must be greater than zero"));
    }
    Ok(value)
}

#[derive(Debug, Clone, Copy)]
struct Measurement {
    setup: Duration,
    status: u16,
    quality: Option<sipx_rtp::Quality>,
}

#[allow(
    clippy::too_many_lines,
    reason = "validation, endpoint construction, owned execution and final reporting stay in lifecycle order"
)]
pub(crate) async fn run(command: LoadOptions, format: Format) -> Exit {
    let uri_text = command.uri.as_str();
    let limits = match Limits::parse(&command) {
        Ok(limits) => limits,
        Err(message) => return fail(format, Exit::Usage, &message),
    };
    let Ok(to) = Uri::parse(bytes::Bytes::from(uri_text.to_owned())) else {
        return fail(format, Exit::Usage, &format!("not a SIP URI: {uri_text}"));
    };
    let mut transport = match crate::signalling::Selection::from_options(
        &command.signalling,
        to.scheme().is_secure(),
    ) {
        Ok(transport) => transport,
        Err(message) => return fail(format, Exit::Usage, &message),
    };
    // Every bound the run stated is a ceiling over the one lookup its calls all share: a run whose
    // calls may take one second to set up must not spend `T-38`'s eight resolving before the first
    // of them is placed, and neither must one that may only last one second at all.
    let resolver = crate::destination::Resolver::within(limits.resolution_bound());
    let candidates = match resolver
        .resolve(&to, None, transport, &command.signalling)
        .await
    {
        Ok(candidates) => candidates,
        Err(error) => {
            return fail(format, crate::destination::exit(&error), &error.to_string());
        }
    };
    let target = match crate::destination::first(&candidates) {
        Ok(target) => target.clone(),
        Err(error) => {
            return fail(format, crate::destination::exit(&error), &error.to_string());
        }
    };
    let target_addr = target.addr;
    transport = transport.negotiated(target.transport);
    let local = command.local;
    let media_address: IpAddr = crate::advertise::reachable_ip(local, target_addr.ip());
    let from = command
        .from
        .as_deref()
        .map_or_else(|| format!("<sip:sipx@{media_address}>"), str::to_owned);
    let credentials = match credentials(&command, &from) {
        Ok(credentials) => credentials,
        Err(message) => return fail(format, Exit::Usage, &message),
    };

    let mut config = TransportConfig::new(local);
    config.sent_by = media_address.to_string();
    if let Err(message) = transport.configure_client(&command.signalling, &mut config) {
        return fail(format, Exit::Usage, &message);
    }
    let (handle, _incoming) = match bind(config).await {
        Ok(bound) => bound,
        Err(error) => return fail(format, Exit::Failed, &format!("bind: {error}")),
    };

    let signalling_from = from.clone();
    let mut options = DialOptions::new(from, media_address);
    if !limits.setup_timeout.is_zero() {
        options = options.with_timeout(limits.setup_timeout);
    }
    let mode_header = match sipx_sip::Header::build(
        workload_mode_name(),
        Bytes::from_static(limits.mode.as_str().as_bytes()),
    ) {
        Ok(header) => header,
        Err(error) => return fail(format, Exit::Failed, &error.to_string()),
    };
    options = options.with_header(mode_header);
    if let Some(credentials) = credentials.clone() {
        options = options.with_credentials(credentials);
    }

    let stop = Stop::new();
    let interrupt = stop.clone();
    let process_stop = crate::stop::Stop::new();
    let signal_listener = process_stop.clone();
    let signal_task = tokio::spawn(async move {
        signal_listener.wait().await;
        interrupt.request();
    });
    let measurements = Arc::new(Mutex::new(Vec::<Measurement>::new()));
    let observed = Arc::clone(&measurements);
    // How far the deepest failed candidate pass got. Every admitted call walks the same resolved
    // list, so the deepest one is what answers the operator's question — was every address behind
    // this name tried, or only the head of the list? A shallower pass says only that *that* call
    // stopped sooner.
    let walked = Arc::new(Mutex::new(None::<crate::destination::Attempts>));
    let observed_passes = Arc::clone(&walked);
    let handle = Arc::new(handle);
    // Held back from the workload closure, which owns every clone the admitted calls are placed
    // through: without one reference kept outside it there is nothing left to shut the endpoint
    // down with once the plan finishes, and the summary below would be a record of work that had
    // only been dropped rather than joined.
    let endpoint = Arc::clone(&handle);
    let to = Arc::new(to);
    let signalling_from = Arc::new(signalling_from);
    let options = Arc::new(options);
    let credentials = Arc::new(credentials);
    let candidates = Arc::new(candidates);

    crate::progress::LoadStart {
        target: uri_text,
        mode: limits.mode.as_str(),
        rate: limits.rate,
        concurrency: limits.concurrency,
        calls: limits.calls,
        duration: limits.duration,
    }
    .emit();
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
            let signalling_from = Arc::clone(&signalling_from);
            let options = Arc::clone(&options);
            let credentials = Arc::clone(&credentials);
            let candidates = Arc::clone(&candidates);
            let measurements = Arc::clone(&observed);
            let passes = Arc::clone(&walked);
            async move {
                let measurement = match run_attempt(
                    index,
                    limits,
                    &handle,
                    &candidates,
                    &to,
                    &signalling_from,
                    &options,
                    credentials.as_ref().as_ref(),
                    &stop,
                )
                .await
                {
                    Ok(measurement) => measurement,
                    Err(failed) => {
                        record_pass(&passes, failed.attempts);
                        if matches!(failed.cause, Cause::Other(_)) {
                            stop.request();
                        }
                        return Err(failed.cause);
                    }
                };
                let Ok(mut measurements) = measurements.lock() else {
                    stop.request();
                    return Err(Cause::Other("measurement store poisoned".to_owned()));
                };
                measurements.push(measurement);
                Ok(())
            }
        },
    )
    .await;

    signal_task.abort();
    let _ = signal_task.await;
    // The summary is this command's terminal record, so nothing it started may still be running
    // when a harness reads it. `Handle::shutdown` is the endpoint's own cleanup barrier: every
    // transaction and timer the admitted calls left behind is cancelled and waited on, which is a
    // causal join rather than the incidental teardown that dropping the last handle would be.
    endpoint.shutdown().await;
    let signal_failure = process_stop.failure();
    let measurements = match measurements.lock() {
        Ok(values) => values.clone(),
        Err(_) => return fail(format, Exit::Failed, "measurement store poisoned"),
    };
    let walked = match observed_passes.lock() {
        Ok(pass) => *pass,
        Err(_) => return fail(format, Exit::Failed, "measurement store poisoned"),
    };
    emit_summary(
        format,
        uri_text,
        limits,
        &bounded,
        &measurements,
        walked,
        process_stop.signal(),
        signal_failure.as_deref(),
    );

    if signal_failure.is_some()
        || !bounded.cleanup_complete
        || has_internal_failure(&bounded.outcome.failures)
    {
        Exit::Failed
    } else {
        Exit::Success
    }
}

/// One admitted call that produced no measurement, and how far its candidate pass got.
///
/// The count travels beside the cause rather than inside it: `Cause` is the harness's per-call
/// classification and knows nothing of the address list this command walked, and the pass is the
/// command's. `T-42` — before it, both of this module's passes counted nothing at all, so a run
/// against a name with four addresses reported the same failures whether one host refused or every
/// address behind the name did.
#[derive(Debug)]
struct Failed {
    /// What the harness counts this call as.
    cause: Cause,
    /// How far the serial pass got, when the failure came from one.
    attempts: Option<crate::destination::Attempts>,
}

impl Failed {
    /// A failure no candidate pass produced — building the call's identity, or its own teardown.
    fn stated(cause: Cause) -> Self {
        Self {
            cause,
            attempts: None,
        }
    }
}

/// Turn a pass that reached nothing into the cause the harness counts and the depth it reports.
///
/// The classification each variant keeps is the one the hand-rolled loops had: a list walked to its
/// end is the last transport failure, and an answer is the far end's. `Nothing` is a caller that
/// resolved no candidate at all, which `run` already refuses before admitting a call, and is
/// counted as a transport failure because that is what an address list with nothing in it leaves.
/// `Expired` is new only because the budget it names is: a pass funded from one deadline can now
/// run out of it, which is a timeout and not a statement about the addresses left untried.
fn unreached(outcome: crate::destination::Unreached<Cause>) -> Failed {
    let attempts = outcome.attempts();
    let cause = match outcome {
        crate::destination::Unreached::Nothing => Cause::Transport,
        crate::destination::Unreached::Expired { .. } => Cause::Timeout,
        crate::destination::Unreached::Unreachable { last, .. }
        | crate::destination::Unreached::Answered(last) => last,
    };
    Failed { cause, attempts }
}

#[allow(clippy::too_many_arguments)]
async fn run_attempt(
    index: usize,
    limits: Limits,
    handle: &sipx_transport::Handle,
    candidates: &[sipx_transport::Target],
    to: &Uri,
    from: &str,
    options: &DialOptions,
    credentials: Option<&Credentials>,
    stop: &Stop,
) -> Result<Measurement, Failed> {
    match limits.mode {
        WorkloadMode::Signalling => {
            let call =
                CallIdentity::new(handle, to, from, limits.seed, index).map_err(Failed::stated)?;
            // `P-31`'s serial pass, and `P-29`'s accounting with it: `--timeout` bounds *a call's
            // setup*, so it funds every address of the target together rather than restarting at
            // each one. A three-address name previously spent three times the number the operator
            // typed before one call gave up.
            //
            // `T-45`: the position in that pass is the third input to the call's identity, and it
            // is counted here because `walk` calls this once per attempted candidate, in order.
            let mut position = 0_usize;
            crate::destination::walk(
                candidates,
                (!limits.setup_timeout.is_zero()).then_some(limits.setup_timeout),
                |target, remaining| {
                    let target = target.clone();
                    let identity = call.at(position);
                    position = position.saturating_add(1);
                    async move {
                        match run_signalling_attempt(
                            handle,
                            target,
                            to,
                            &identity,
                            credentials,
                            remaining,
                            limits,
                            stop,
                        )
                        .await
                        {
                            Ok(measurement) => crate::destination::Attempted::Reached(measurement),
                            // Only a transport failure is this address's. A rejection, a response
                            // deadline or a broken exchange is the far end answering for the name,
                            // and another address repeats a question already answered.
                            Err(Cause::Transport) => {
                                crate::destination::Attempted::Unreachable(Cause::Transport)
                            }
                            Err(cause) => crate::destination::Attempted::Answered(cause),
                        }
                    }
                },
            )
            .await
            .map_err(unreached)
        }
        WorkloadMode::GeneratedMedia => {
            run_generated_media_attempt(handle, candidates, to, options, limits, index, stop).await
        }
    }
}

async fn run_generated_media_attempt(
    handle: &sipx_transport::Handle,
    candidates: &[sipx_transport::Target],
    to: &Uri,
    options: &DialOptions,
    limits: Limits,
    index: usize,
    stop: &Stop,
) -> Result<Measurement, Failed> {
    let started = tokio::time::Instant::now();
    // The same pass and the same budget as the signalling mode above; only what one candidate does
    // with its share differs.
    let mut call = crate::destination::walk(
        candidates,
        (!limits.setup_timeout.is_zero()).then_some(limits.setup_timeout),
        |target, remaining| {
            let funded = crate::budget::funded(options, remaining);
            let target = target.clone();
            async move {
                match sipx_call::dial_until(handle, target, to, &funded, stop.requested()).await {
                    Ok(call) => crate::destination::Attempted::Reached(call),
                    Err(error @ sipx_call::Error::Transport(_)) => {
                        crate::destination::Attempted::Unreachable(classify(error))
                    }
                    Err(error) => crate::destination::Attempted::Answered(classify(error)),
                }
            }
        },
    )
    .await
    .map_err(unreached)?;
    let setup = started.elapsed();
    let status = call.initial_status();
    // One bounded packet is enough to make media deterministic and observable without allocating
    // in proportion to an operator-supplied call duration.
    let frame = deterministic_frame(limits.seed, index);
    let played = call.media().play(&frame, frame.len()).await;
    wait_for_call_end(limits.call_duration, stop).await;
    let quality = call.media().quality().await;
    call.hang_up()
        .await
        .map_err(|error| Failed::stated(Cause::Other(format!("hang up failed: {error}"))))?;
    if !played {
        return Err(Failed::stated(Cause::Other(
            "media playback failed".to_owned(),
        )));
    }
    Ok(Measurement {
        setup,
        status,
        quality: Some(quality),
    })
}

/// One admitted call's signalling identity, before the pass decides which address hears it.
///
/// The split is `T-45`, and the line it draws is RFC 3261 §8.2.2.2's. The `To` header, the `From`
/// URI and the `Contact` are the call and are the same wherever it is placed; the `Call-ID` and the
/// `From` tag are *per candidate*, because two addresses of one name commonly lead to the same
/// server, and a second INVITE carrying the `Call-ID`, `From` tag and `CSeq` of one that server
/// already accepted is a merged request there — answered `482 Loop Detected`, which this command
/// reads as the far end's answer for the whole name. The pass then stops at the address a fresh
/// call would have been accepted at.
///
/// `peers` states the same rule for the same reason one usage over (RFC 6665 §4.1.2.1), and
/// `--mode generated-media` has always had it: `sipx_call::dial_until` builds its own identity, and
/// the pass calls it once per candidate.
#[derive(Debug)]
struct CallIdentity {
    to: Bytes,
    from_uri: String,
    contact: Bytes,
    run_id: String,
    seed: u64,
    index: u64,
}

impl CallIdentity {
    fn new(
        handle: &sipx_transport::Handle,
        to: &Uri,
        from: &str,
        seed: u64,
        index: usize,
    ) -> Result<Self, Cause> {
        let from = Address::parse(from.as_bytes(), "From")
            .map_err(|error| Cause::Other(format!("invalid load From address: {error}")))?;
        let run_tail = seed.rotate_left(29) ^ 0x6c6f_6164_2d72_756e;
        Ok(Self {
            to: Bytes::from(format!("<{}>", String::from_utf8_lossy(&to.to_bytes()))),
            from_uri: String::from_utf8_lossy(&from.uri.to_bytes()).into_owned(),
            contact: Bytes::from(format!("<sip:load@{}>", handle.advertised())),
            run_id: format!("{seed:016x}{run_tail:016x}"),
            seed,
            index: u64::try_from(index).unwrap_or(u64::MAX),
        })
    }

    /// The identity for the candidate at `position` in this call's pass.
    ///
    /// Derived, never random: `--seed` and the call index still decide every byte, with the
    /// candidate position as the only new input, so a run replayed under the same seed puts the
    /// same requests on the wire in the same order.
    fn at(&self, position: usize) -> SignallingIdentity {
        let (seed, index) = (self.seed, self.index);
        let position = u64::try_from(position).unwrap_or(u64::MAX);
        SignallingIdentity {
            to: self.to.clone(),
            from: Bytes::from(format!(
                "<{}>;tag=f-{seed:016x}-{index:x}-{position:x}",
                self.from_uri
            )),
            call_id: Bytes::from(format!(
                "cl-{}-{index}-{position}@driver.invalid",
                self.run_id
            )),
            contact: self.contact.clone(),
        }
    }
}

/// What one candidate of one admitted call is presented to the far end as.
#[derive(Debug)]
struct SignallingIdentity {
    to: Bytes,
    from: Bytes,
    call_id: Bytes,
    contact: Bytes,
}

#[allow(clippy::too_many_arguments)]
/// One candidate's bodyless INVITE/ACK/BYE exchange, bounded by what the pass has left for it.
///
/// `within` is that share and not `--timeout` itself: the pass funds every address of the target
/// together, so an address reached after two dead ones is answered inside what those two left
/// (`P-29`). `None` is a run that stated no setup deadline at all.
async fn run_signalling_attempt(
    handle: &sipx_transport::Handle,
    target: sipx_transport::Target,
    to: &Uri,
    identity: &SignallingIdentity,
    credentials: Option<&Credentials>,
    within: Option<Duration>,
    limits: Limits,
    stop: &Stop,
) -> Result<Measurement, Cause> {
    let started = tokio::time::Instant::now();
    let mut authorization = None;
    let mut invite_cseq = 1_u32;
    let (invite, accepted) = loop {
        let invite = signalling_invite(
            handle,
            &target,
            to,
            identity,
            invite_cseq,
            authorization.take(),
        )?;
        let mut responses = handle
            .send(invite.clone(), target.clone())
            .await
            .map_err(|_| Cause::Transport)?;
        let response = wait_for_invite(handle, &mut responses, within, stop).await?;
        if matches!(response.status.code(), 401 | 407)
            && let Some(credentials) = credentials
            && invite_cseq == 1
            && let Some(header) = authorization_for(&invite, &response, credentials)
        {
            invite_cseq = invite_cseq.saturating_add(1);
            authorization = Some(header);
            continue;
        }
        if !response.status.is_success() {
            return Err(rejection_cause(&response));
        }
        break (invite, response);
    };

    let mut dialog = sipx_call::Dialog::from_response(&invite, &accepted)
        .ok_or_else(|| Cause::Other("signalling answer created no dialog".to_owned()))?;
    let ack = signalling_dialog_request(handle, &target, &dialog, &Method::Ack, invite_cseq)?;
    handle
        .send_directly(ack, target.clone())
        .await
        .map_err(|_| Cause::Transport)?;
    let setup = started.elapsed();
    wait_for_call_end(limits.call_duration, stop).await;

    let bye_cseq = dialog.next_cseq();
    let bye = signalling_dialog_request(handle, &target, &dialog, &Method::Bye, bye_cseq)?;
    let mut responses = handle
        .send(bye, target)
        .await
        .map_err(|_| Cause::Transport)?;
    let response = responses.final_response().await.ok_or(Cause::Timeout)?;
    if !signalling_response_matches(&response, &dialog, bye_cseq) {
        return Err(Cause::Other(
            "signalling BYE received an invalid response".to_owned(),
        ));
    }
    if !response.status.is_success() {
        return Err(Cause::Other(format!(
            "hang up failed: rejected {} {}",
            response.status.code(),
            String::from_utf8_lossy(&response.reason)
        )));
    }
    Ok(Measurement {
        setup,
        status: accepted.status.code(),
        quality: None,
    })
}

fn signalling_invite(
    handle: &sipx_transport::Handle,
    target: &sipx_transport::Target,
    to: &Uri,
    identity: &SignallingIdentity,
    cseq: u32,
    authorization: Option<sipx_sip::Header>,
) -> Result<Request, Cause> {
    let builder = RequestBuilder::new(Method::Invite, to.clone())
        .header(
            HeaderName::Via,
            Bytes::from(format!(
                "SIP/2.0/{} {};rport;branch={}",
                target.transport.as_str(),
                handle.sent_by_for(target.transport),
                sipx_transport::new_branch()
            )),
        )
        .map_err(build_cause)?
        .header(HeaderName::To, identity.to.clone())
        .map_err(build_cause)?
        .header(HeaderName::From, identity.from.clone())
        .map_err(build_cause)?
        .header(HeaderName::CallId, identity.call_id.clone())
        .map_err(build_cause)?
        .cseq(cseq, &Method::Invite)
        .map_err(build_cause)?
        .header(HeaderName::Contact, identity.contact.clone())
        .map_err(build_cause)?
        .header(
            workload_mode_name(),
            Bytes::from_static(WorkloadMode::Signalling.as_str().as_bytes()),
        )
        .map_err(build_cause)?
        .max_forwards(70);
    let mut request = builder.build();
    if let Some(header) = authorization {
        request.headers.push(header);
    }
    Ok(request)
}

fn signalling_dialog_request(
    handle: &sipx_transport::Handle,
    target: &sipx_transport::Target,
    dialog: &sipx_call::Dialog,
    method: &Method,
    cseq: u32,
) -> Result<Request, Cause> {
    let (local, remote) = dialog.local_and_remote();
    let (uri, routes) = dialog.request_target();
    let mut builder = RequestBuilder::new(method.clone(), uri)
        .header(
            HeaderName::Via,
            Bytes::from(format!(
                "SIP/2.0/{} {};rport;branch={}",
                target.transport.as_str(),
                handle.sent_by_for(target.transport),
                sipx_transport::new_branch()
            )),
        )
        .map_err(build_cause)?
        .header(HeaderName::To, Bytes::from(remote))
        .map_err(build_cause)?
        .header(HeaderName::From, Bytes::from(local))
        .map_err(build_cause)?
        .header(HeaderName::CallId, Bytes::from(dialog.id.call_id.clone()))
        .map_err(build_cause)?
        .cseq(cseq, method)
        .map_err(build_cause)?
        .max_forwards(70);
    for route in routes {
        builder = builder
            .header(HeaderName::Route, Bytes::from(route))
            .map_err(build_cause)?;
    }
    Ok(builder.build())
}

#[allow(
    clippy::needless_pass_by_value,
    reason = "the function is passed directly to Result::map_err at each builder step"
)]
fn build_cause(error: sipx_sip::BuildError) -> Cause {
    Cause::Other(format!(
        "could not build signalling workload request: {error}"
    ))
}

async fn wait_for_invite(
    handle: &sipx_transport::Handle,
    responses: &mut sipx_transport::Responses,
    within: Option<Duration>,
    stop: &Stop,
) -> Result<Response, Cause> {
    let deadline = within
        .filter(|within| !within.is_zero())
        .map(|within| tokio::time::Instant::now() + within);
    loop {
        tokio::select! {
            biased;
            () = stop.requested() => {
                return cancel_signalling_invite(handle, responses)
                    .await
                    .ok_or(Cause::Timeout);
            }
            () = wait_until(deadline), if deadline.is_some() => {
                return cancel_signalling_invite(handle, responses)
                    .await
                    .ok_or(Cause::Timeout);
            }
            event = responses.next() => match event {
                Some(sipx_sip::transaction::TuEvent::Response(response)) if response.status.is_final() => {
                    return Ok(*response);
                }
                Some(sipx_sip::transaction::TuEvent::Timeout) => return Err(Cause::Timeout),
                Some(sipx_sip::transaction::TuEvent::TransportError) | None => {
                    return Err(Cause::Transport);
                }
                Some(_) => {}
            }
        }
    }
}

async fn cancel_signalling_invite(
    handle: &sipx_transport::Handle,
    responses: &mut sipx_transport::Responses,
) -> Option<Response> {
    // The load runner's 40-second cleanup cap is the bound on failure. The transport helper waits
    // for the RFC 3261 provisional-response precondition and preserves a crossing final response.
    match handle.cancel_invite(responses, None).await.ok()? {
        sipx_transport::CancelInviteOutcome::FinalResponse { response, .. } => Some(response),
        sipx_transport::CancelInviteOutcome::Sent(mut cancellation) => {
            let _ = cancellation.outcome().await;
            // A successful final response can cross a correctly created CANCEL. It still creates
            // a dialog and therefore must be returned to the ACK/BYE path above.
            responses
                .final_response()
                .await
                .filter(|response| response.status.is_success())
        }
        _ => None,
    }
}

async fn wait_until(deadline: Option<tokio::time::Instant>) {
    match deadline {
        Some(deadline) => tokio::time::sleep_until(deadline).await,
        None => std::future::pending().await,
    }
}

async fn wait_for_call_end(duration: Duration, stop: &Stop) {
    if duration.is_zero() {
        return;
    }
    tokio::select! {
        () = stop.requested() => {}
        () = tokio::time::sleep(duration) => {}
    }
}

fn authorization_for(
    request: &Request,
    response: &Response,
    credentials: &Credentials,
) -> Option<sipx_sip::Header> {
    let from_proxy = response.status.code() == 407;
    let name = if from_proxy {
        HeaderName::ProxyAuthenticate
    } else {
        HeaderName::WwwAuthenticate
    };
    let challenges = response
        .headers
        .get_all(&name)
        .filter_map(|header| sipx_sip::auth::Challenge::parse(&header.value(), from_proxy))
        .collect();
    let challenge = sipx_sip::auth::strongest(challenges)?;
    let uri_bytes = request.uri.to_bytes();
    let uri = String::from_utf8_lossy(&uri_bytes);
    let cnonce = sipx_transport::new_branch();
    let value = sipx_sip::auth::respond(&challenge, credentials, "INVITE", &uri, 1, &cnonce);
    sipx_sip::Header::build(challenge.response_header(), Bytes::from(value)).ok()
}

fn rejection_cause(response: &Response) -> Cause {
    let reason = String::from_utf8_lossy(&response.reason);
    if response.status.code() == 488 && reason == MODE_MISMATCH_REASON {
        Cause::Other(format!(
            "workload mode mismatch: peer refused {}",
            WorkloadMode::Signalling.as_str()
        ))
    } else {
        Cause::Rejected(response.status.code())
    }
}

fn signalling_response_matches(response: &Response, dialog: &sipx_call::Dialog, cseq: u32) -> bool {
    response.headers.count(&HeaderName::CallId) == 1
        && response
            .headers
            .value(&HeaderName::CallId)
            .is_some_and(|value| value.as_ref() == dialog.id.call_id.as_slice())
        && response
            .headers
            .typed::<CSeq>()
            .and_then(Result::ok)
            .is_some_and(|value| value.sequence == cseq && value.method == Method::Bye)
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

fn credentials(options: &LoadOptions, from: &str) -> Result<Option<Credentials>, String> {
    let password = options.password.clone();
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

/// Keep the deepest pass any failed call made.
///
/// A poisoned store is dropped rather than reported: this is a field beside the run's result, and a
/// summary that refused to print because a count could not be recorded would withhold the numbers
/// the run exists to produce. The counts are absent when nothing recorded one, which is exactly
/// what the reader is told.
fn record_pass(
    passes: &Mutex<Option<crate::destination::Attempts>>,
    attempts: Option<crate::destination::Attempts>,
) {
    let Some(attempts) = attempts else {
        return;
    };
    let Ok(mut deepest) = passes.lock() else {
        return;
    };
    if deepest.is_none_or(|previous| previous.attempted() < attempts.attempted()) {
        *deepest = Some(attempts);
    }
}

fn classify(error: sipx_call::Error) -> Cause {
    match error {
        sipx_call::Error::Rejected {
            status: 488,
            reason,
        } if reason == MODE_MISMATCH_REASON => Cause::Other(format!(
            "workload mode mismatch: peer refused {}",
            WorkloadMode::GeneratedMedia.as_str()
        )),
        sipx_call::Error::Rejected { status, .. } => Cause::Rejected(status),
        sipx_call::Error::Cancelled(_) | sipx_call::Error::NoResponse => Cause::Timeout,
        sipx_call::Error::Transport(_) | sipx_call::Error::Io(_) => Cause::Transport,
        other => Cause::Other(other.to_string()),
    }
}

fn has_internal_failure(failures: &BTreeMap<Cause, usize>) -> bool {
    failures
        .keys()
        .any(|cause| matches!(cause, Cause::Other(_)))
}

fn internal_reason(failures: &BTreeMap<Cause, usize>) -> Option<&str> {
    failures.keys().find_map(|cause| match cause {
        Cause::Other(message) => Some(message.as_str()),
        _ => None,
    })
}

fn response_counts(
    outcome: &sipx_call::load::Outcome,
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
#[allow(
    clippy::too_many_lines,
    reason = "one summary calculation feeds JSON, text and bounded INFO from the same facts"
)]
#[allow(
    clippy::too_many_arguments,
    reason = "each is an independently measured fact about the run; bundling them into a struct \
              would name the summary's own shape twice"
)]
fn emit_summary(
    format: Format,
    target: &str,
    limits: Limits,
    bounded: &sipx_call::load::BoundedOutcome,
    measurements: &[Measurement],
    walked: Option<crate::destination::Attempts>,
    stop_signal: Option<&str>,
    signal_failure: Option<&str>,
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
    let quality: Vec<_> = measurements
        .iter()
        .filter_map(|value| value.quality.as_ref())
        .collect();
    let snapshots = quality.len();
    let packets_lost: i64 = quality.iter().map(|value| value.cumulative_lost).sum();
    let divisor = snapshots as f64;
    let mean = |value: fn(&sipx_rtp::Quality) -> f64| {
        (snapshots > 0).then(|| quality.iter().map(|item| value(item)).sum::<f64>() / divisor)
    };
    let reason = if signal_failure.is_some() {
        signal_failure
    } else if bounded.cleanup_complete {
        internal_reason(&outcome.failures)
    } else {
        Some("cleanup budget exhausted")
    };
    let status = if reason.is_some() {
        "failed"
    } else if bounded.admission_end == AdmissionEnd::Requested {
        "interrupted"
    } else {
        "completed"
    };
    let summary = serde_json::json!({
        "schema": "sipx.load.v1",
        "status": status,
        "stop_signal": stop_signal,
        "reason": reason,
        "mode": limits.mode.as_str(),
        "seed": limits.seed,
        "target": target,
        // `T-41`'s pair, under the names every other command reports it with, for the deepest pass
        // a connection failure made. Null — never zero — where no failed call walked a list: zero
        // would describe a pass that ran and got nowhere, and a run whose calls all connected made
        // no such pass.
        "candidates_attempted": walked.map(|walked| walked.attempted()),
        "candidates_resolved": walked.map(|walked| walked.resolved()),
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
            "mean_loss": mean(|value| value.loss),
            "mean_jitter_ms": mean(|value| value.jitter.as_secs_f64() * 1000.0),
            "mean_mos": mean(|value| value.mos),
        }
    });

    crate::progress::LoadSummary {
        status,
        attempted: outcome.attempted,
        connected,
        rejected,
        timed_out,
        failed,
        peak_concurrency: bounded.peak_in_flight,
    }
    .emit();

    match format {
        Format::Json => println!("{summary}"),
        Format::Text => {
            println!("status             {status}");
            if let Some(signal) = stop_signal {
                println!("stop_signal        {signal}");
            }
            println!("mode               {}", limits.mode.as_str());
            if let Some(reason) = reason {
                println!("reason             {reason}");
            }
            println!("target             {target}");
            if let Some(walked) = walked {
                println!("candidates_attempted {}", walked.attempted());
                println!("candidates_resolved  {}", walked.resolved());
            }
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
#[cfg_attr(coverage_nightly, coverage(off))]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;
    use clap::Parser as _;
    use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};

    use crate::cli::{Cli, Command};

    fn raw(items: &[&str]) -> Vec<String> {
        items.iter().map(|item| (*item).to_owned()).collect()
    }

    fn command(raw: &[String]) -> LoadOptions {
        let cli =
            Cli::try_parse_from(std::iter::once("sipx").chain(raw.iter().map(String::as_str)))
                .expect("argument shape");
        let Some(Command::Load(options)) = cli.command else {
            panic!("load command expected");
        };
        options
    }

    /// `P-27`: `load`'s summary is its terminal record, and nothing this invocation started may
    /// still be running when a harness reads it.
    ///
    /// A regression guard rather than the story's failing-first test, and worth saying which:
    /// before `P-27` `run` never shut the endpoint down on any path, but it dropped the last
    /// handle and then awaited the signal task, so the driver had in fact noticed and released
    /// the socket by the time the summary was printed. This probe cannot tell an incidental
    /// teardown from a join, so it passed then and passes now. What `P-27` changed is the
    /// guarantee: the shutdown is ordered before the summary and waits on the endpoint's own
    /// cleanup barrier, instead of depending on an await that happens to be there.
    ///
    /// One admitted call against a black hole is enough: the plan is bounded, the summary is
    /// printed, and the probe asks what is left. `crate::join_probe` explains the timing.
    #[tokio::test]
    async fn the_summary_joins_the_endpoint_before_it_is_printed() {
        let black_hole = tokio::net::UdpSocket::bind("127.0.0.1:0")
            .await
            .expect("binds");
        let peer = black_hole.local_addr().expect("has an address");
        let local = crate::join_probe::free_local();
        let arguments = raw(&[
            "load",
            &format!("sip:load@{peer}"),
            "--rate",
            "1",
            "--concurrency",
            "1",
            "--calls",
            "1",
            "--timeout",
            "1",
            "--local",
            &local.to_string(),
        ]);

        let exit = run(command(&arguments), Format::Json).await;

        crate::join_probe::assert_released(local, "load summary");
        assert_eq!(
            exit.code(),
            Exit::Success.code(),
            "an admitted call that nothing answered is a measured outcome, not an internal failure"
        );
        drop(black_hole);
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
        assert!(Limits::parse(&command(&unbounded)).is_err());

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
        let limits = Limits::parse(&command(&bounded)).expect("finite plan");
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
            assert!(Limits::parse(&command(&values)).is_err(), "{values:?}");
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
        assert!(
            Limits::parse(&command(&excessive)).is_err(),
            "{excessive:?}"
        );
    }

    #[test]
    fn response_counts_use_the_success_status_that_arrived() {
        let measurements = [Measurement {
            setup: Duration::from_millis(2),
            status: 202,
            quality: Some(sipx_rtp::Quality::new(0.0, 0, Duration::ZERO, None)),
        }];
        let mut outcome = sipx_call::load::Outcome::default();
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

    /// `T-45`: the candidate position is the only new input, and nothing about the identity became
    /// random.
    ///
    /// A run replayed under the same `--seed` still puts the same bytes on the wire, which is what
    /// `load` promises a seed for. What the position separates is the `Call-ID` and the `From` tag;
    /// the `To` header and the `Contact` are the call's and are the same at every address.
    #[tokio::test]
    async fn seed_call_index_and_candidate_position_reproduce_the_signalling_identity() {
        let (handle, _incoming) = bind(TransportConfig::new(
            "127.0.0.1:0".parse().expect("local address"),
        ))
        .await
        .expect("endpoint binds");
        let to = Uri::parse(Bytes::from_static(b"sip:load@127.0.0.1")).expect("request URI");
        let call = |seed, index| {
            CallIdentity::new(&handle, &to, "<sip:sipx@127.0.0.1>", seed, index).expect("identity")
        };

        let replayed = (call(41, 2).at(1), call(41, 2).at(1));
        assert_eq!(replayed.0.call_id, replayed.1.call_id);
        assert_eq!(replayed.0.from, replayed.1.from);

        let call_41_2 = call(41, 2);
        for (left, right) in [
            (call_41_2.at(0), call_41_2.at(1)),
            (call(41, 2).at(0), call(41, 3).at(0)),
            (call(41, 2).at(0), call(42, 2).at(0)),
        ] {
            assert_ne!(left.call_id, right.call_id);
            assert_ne!(left.from, right.from);
            assert_eq!(
                left.to, right.to,
                "the callee is the call's, not the address's"
            );
            assert_eq!(left.contact, right.contact, "one endpoint places them all");
        }

        handle.shutdown().await;
    }

    /// `T-45`: the pass presents each address of a name with a call of its own.
    ///
    /// Two addresses of one name commonly lead to one server — an A record pair, or an SRV pair
    /// over one host — and RFC 3261 §8.2.2.2 makes a second INVITE carrying the `Call-ID`, `From`
    /// tag and `CSeq` of one already accepted a *merged request*, which a compliant UAS answers
    /// `482 Loop Detected`. Before this story the pass sent exactly that: one identity built per
    /// admitted call, outside the walk, with only the `Via` branch differing between candidates.
    /// `load` then read the 482 as the far end's answer for the whole name and stopped, so the
    /// fallback the pass exists for could not happen against the one topology it matters most on.
    #[tokio::test]
    async fn a_second_candidate_at_the_same_server_gets_a_call_of_its_own() {
        let server = SameServer::start().await;
        let (handle, _incoming) = bind(TransportConfig::new(
            "127.0.0.1:0".parse().expect("local address"),
        ))
        .await
        .expect("endpoint binds");
        let uri = format!("sip:load@{}", server.answering);
        let arguments = raw(&[
            "load",
            &uri,
            "--rate",
            "1",
            "--concurrency",
            "1",
            "--calls",
            "1",
            // A ceiling over the whole pass, not a wait: both candidates answer on loopback as
            // soon as they are asked, and this only stops a wedged run from hanging the suite.
            "--timeout",
            "10",
        ]);
        let limits = Limits::parse(&command(&arguments)).expect("finite plan");
        let to = Uri::parse(Bytes::from(uri)).expect("request URI");
        let from = "<sip:sipx@127.0.0.1>".to_owned();
        let options = DialOptions::new(from.clone(), IpAddr::V4(std::net::Ipv4Addr::LOCALHOST));
        let candidates = [
            sipx_transport::Target::new(server.broken, sipx_transport::TransportKind::Tcp),
            sipx_transport::Target::new(server.answering, sipx_transport::TransportKind::Tcp),
        ];

        let measured = run_attempt(
            0,
            limits,
            &handle,
            &candidates,
            &to,
            &from,
            &options,
            None,
            &Stop::new(),
        )
        .await;

        handle.shutdown().await;
        let measurement = measured.unwrap_or_else(|failed| {
            panic!(
                "the second address of a name whose first one took the INVITE and dropped must be \
                 a call of its own, not the first one arriving twice: {:?}",
                failed.cause
            )
        });
        assert_eq!(
            measurement.status, 200,
            "the second candidate was answered by the server the first one already had a request \
             from"
        );
        let seen = server.accepted();
        assert_eq!(
            seen.len(),
            2,
            "both addresses were presented with a request the server could accept: {seen:?}"
        );
        assert_ne!(
            seen.first(),
            seen.last(),
            "each candidate needs its own Call-ID and From tag, not a copy of the last one's: \
             {seen:?}"
        );
    }

    /// One server, reachable at two addresses, refusing merged requests as RFC 3261 §8.2.2.2
    /// requires a UAS to.
    ///
    /// Two listeners over one shared record of what has been accepted is what "the same server
    /// behind two addresses" *is*; nothing else about the fixture matters. The first address reads
    /// the request and drops the connection, which is the one classification the pass walks past
    /// (`sipx-transport` fails a transaction the moment its connection goes, rather than leaving
    /// it to Timer B) and which still leaves the server holding the request. The second address
    /// then sees whatever the pass sends next: the same request by another path, or a new call.
    struct SameServer {
        /// Takes the request and drops the connection without answering.
        broken: std::net::SocketAddr,
        /// Answers `482` to a request this server already has, `200` to anything else.
        answering: std::net::SocketAddr,
        accepted: Arc<Mutex<Vec<String>>>,
        listening: Vec<tokio::task::JoinHandle<()>>,
    }

    impl SameServer {
        async fn start() -> Self {
            let dropping = tokio::net::TcpListener::bind("127.0.0.1:0")
                .await
                .expect("binds");
            let answering = tokio::net::TcpListener::bind("127.0.0.1:0")
                .await
                .expect("binds");
            let server = Self {
                broken: dropping.local_addr().expect("has an address"),
                answering: answering.local_addr().expect("has an address"),
                accepted: Arc::new(Mutex::new(Vec::new())),
                listening: Vec::new(),
            };
            let contact = server.answering;
            let taken = Arc::clone(&server.accepted);
            let held = Arc::clone(&server.accepted);
            let mut server = server;
            server.listening.push(tokio::spawn(async move {
                // Aborting this task drops the set, which is what stops the connections it owns.
                let mut connections = tokio::task::JoinSet::new();
                while let Ok((stream, _)) = dropping.accept().await {
                    let taken = Arc::clone(&taken);
                    connections.spawn(async move {
                        let mut stream = stream;
                        let mut buffered = Vec::new();
                        if let Some(request) = read_message(&mut stream, &mut buffered).await
                            && let Ok(mut taken) = taken.lock()
                        {
                            taken.push(merge_identity(&request));
                        }
                    });
                }
            }));
            server.listening.push(tokio::spawn(async move {
                let mut connections = tokio::task::JoinSet::new();
                while let Ok((stream, _)) = answering.accept().await {
                    let held = Arc::clone(&held);
                    connections.spawn(async move {
                        let mut stream = stream;
                        let mut buffered = Vec::new();
                        while let Some(request) = read_message(&mut stream, &mut buffered).await {
                            let Some(reply) = answer(&request, &held, contact) else {
                                continue;
                            };
                            if stream.write_all(reply.as_bytes()).await.is_err() {
                                return;
                            }
                        }
                    });
                }
            }));
            server
        }

        /// Every request this server took, by the three terms §8.2.2.2 merges on.
        fn accepted(&self) -> Vec<String> {
            self.accepted
                .lock()
                .map(|seen| seen.clone())
                .unwrap_or_default()
        }
    }

    impl Drop for SameServer {
        fn drop(&mut self) {
            for listening in &self.listening {
                listening.abort();
            }
        }
    }

    /// Read one bodyless SIP message, keeping whatever arrived behind it.
    async fn read_message(
        stream: &mut tokio::net::TcpStream,
        buffered: &mut Vec<u8>,
    ) -> Option<String> {
        loop {
            if let Some(end) = buffered.windows(4).position(|window| window == b"\r\n\r\n") {
                let message: Vec<u8> = buffered.drain(..end.saturating_add(4)).collect();
                return Some(String::from_utf8_lossy(&message).into_owned());
            }
            let mut chunk = [0u8; 2048];
            match stream.read(&mut chunk).await {
                Ok(0) | Err(_) => return None,
                Ok(read) => buffered.extend_from_slice(chunk.get(..read).unwrap_or_default()),
            }
        }
    }

    /// What this server says to one request, and `None` where a UAS says nothing.
    fn answer(
        request: &str,
        accepted: &Mutex<Vec<String>>,
        contact: std::net::SocketAddr,
    ) -> Option<String> {
        match request.split_whitespace().next()? {
            "ACK" => None,
            "INVITE" => {
                let identity = merge_identity(request);
                let mut accepted = accepted.lock().ok()?;
                if accepted.contains(&identity) {
                    return Some(response(request, "482 Loop Detected", None));
                }
                accepted.push(identity);
                Some(response(
                    request,
                    "200 OK",
                    Some(&format!("<sip:same-server@{contact}>")),
                ))
            }
            _ => Some(response(request, "200 OK", None)),
        }
    }

    /// RFC 3261 §8.2.2.2's three terms, which are what make a second copy a merged request rather
    /// than a new call: the same `Call-ID`, the same `From` tag and the same `CSeq`.
    fn merge_identity(request: &str) -> String {
        let from_tag = header(request, "From")
            .and_then(|value| value.split(";tag=").nth(1))
            .and_then(|tail| tail.split(';').next())
            .unwrap_or_default();
        format!(
            "{}|{from_tag}|{}",
            header(request, "Call-ID").unwrap_or_default(),
            header(request, "CSeq").unwrap_or_default()
        )
    }

    fn response(request: &str, status: &str, contact: Option<&str>) -> String {
        let mut reply = vec![format!("SIP/2.0 {status}")];
        reply.extend(
            ["Via", "From", "Call-ID", "CSeq"]
                .into_iter()
                .filter_map(|name| header(request, name).map(|value| format!("{name}: {value}"))),
        );
        // The caller's dialog needs a remote tag, and this server is the only party that can
        // supply one; a request that already carries one is answered with it unchanged.
        let to = header(request, "To").unwrap_or("<sip:load@127.0.0.1>");
        reply.push(if to.contains(";tag=") {
            format!("To: {to}")
        } else {
            format!("To: {to};tag=same-server")
        });
        reply.extend(contact.map(|contact| format!("Contact: {contact}")));
        reply.push("Content-Length: 0".to_owned());
        format!("{}\r\n\r\n", reply.join("\r\n"))
    }

    fn header<'a>(message: &'a str, name: &str) -> Option<&'a str> {
        message.lines().find_map(|line| {
            let (field, value) = line.split_once(':')?;
            field
                .trim()
                .eq_ignore_ascii_case(name)
                .then_some(value.trim())
        })
    }
}
