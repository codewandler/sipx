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
    // The setup bound the run states for each of its calls is the ceiling over the one lookup they
    // all share: a run whose calls may take one second to set up must not spend `T-38`'s eight
    // resolving before the first of them is even placed.
    let resolver = crate::destination::Resolver::within(
        (!limits.setup_timeout.is_zero()).then_some(limits.setup_timeout),
    );
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
            async move {
                let measurement = run_attempt(
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
                .inspect_err(|cause| {
                    if matches!(cause, Cause::Other(_)) {
                        stop.request();
                    }
                })?;
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
    emit_summary(
        format,
        uri_text,
        limits,
        &bounded,
        &measurements,
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
) -> Result<Measurement, Cause> {
    match limits.mode {
        WorkloadMode::Signalling => {
            let identity = SignallingIdentity::new(handle, to, from, limits.seed, index)?;
            let mut last_transport = None;
            for target in candidates.iter().take(crate::destination::MAX_ATTEMPTS) {
                match run_signalling_attempt(
                    handle,
                    target.clone(),
                    to,
                    &identity,
                    credentials,
                    limits,
                    stop,
                )
                .await
                {
                    Err(Cause::Transport) => last_transport = Some(Cause::Transport),
                    result => return result,
                }
            }
            Err(last_transport.unwrap_or(Cause::Transport))
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
) -> Result<Measurement, Cause> {
    let started = tokio::time::Instant::now();
    let mut last_transport = None;
    let mut connected = None;
    for target in candidates.iter().take(crate::destination::MAX_ATTEMPTS) {
        match sipx_call::dial_until(handle, target.clone(), to, options, stop.requested()).await {
            Ok(call) => {
                connected = Some(call);
                break;
            }
            Err(error @ sipx_call::Error::Transport(_)) => last_transport = Some(error),
            Err(error) => {
                last_transport = Some(error);
                break;
            }
        }
    }
    let mut call = connected.ok_or_else(|| {
        let error = last_transport.unwrap_or(sipx_call::Error::NoResponse);
        classify(error)
    })?;
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
        .map_err(|error| Cause::Other(format!("hang up failed: {error}")))?;
    if !played {
        return Err(Cause::Other("media playback failed".to_owned()));
    }
    Ok(Measurement {
        setup,
        status,
        quality: Some(quality),
    })
}

#[derive(Debug)]
struct SignallingIdentity {
    to: Bytes,
    from: Bytes,
    call_id: Bytes,
    contact: Bytes,
}

impl SignallingIdentity {
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
        let run_id = format!("{seed:016x}{run_tail:016x}");
        let index = u64::try_from(index).unwrap_or(u64::MAX);
        Ok(Self {
            to: Bytes::from(format!("<{}>", String::from_utf8_lossy(&to.to_bytes()))),
            from: Bytes::from(format!(
                "<{}>;tag=f-{seed:016x}-{index:x}",
                String::from_utf8_lossy(&from.uri.to_bytes())
            )),
            call_id: Bytes::from(format!("cl-{run_id}-{index}@driver.invalid")),
            contact: Bytes::from(format!("<sip:load@{}>", handle.advertised())),
        })
    }
}

#[allow(clippy::too_many_arguments)]
async fn run_signalling_attempt(
    handle: &sipx_transport::Handle,
    target: sipx_transport::Target,
    to: &Uri,
    identity: &SignallingIdentity,
    credentials: Option<&Credentials>,
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
        let response = wait_for_invite(handle, &mut responses, limits.setup_timeout, stop).await?;
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
    within: Duration,
    stop: &Stop,
) -> Result<Response, Cause> {
    let deadline = (!within.is_zero()).then(|| tokio::time::Instant::now() + within);
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
fn emit_summary(
    format: Format,
    target: &str,
    limits: Limits,
    bounded: &sipx_call::load::BoundedOutcome,
    measurements: &[Measurement],
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
            quality: Some(sipx_rtp::Quality {
                loss: 0.0,
                cumulative_lost: 0,
                jitter: Duration::ZERO,
                round_trip: None,
                mos: 4.4,
            }),
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
}
