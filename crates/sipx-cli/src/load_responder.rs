//! `sipx load-responder` — a finite, machine-driven answering endpoint.

use std::collections::BTreeMap;
use std::io::Write as _;
use std::net::SocketAddr;
use std::time::Duration;

use bytes::Bytes;
use sha2::{Digest, Sha256};
use sipx_call::{Calls, Dispatched, Invitation, SignallingEvent};
use sipx_sip::build::ResponseBuilder;
use sipx_sip::headers::CSeq;
use sipx_sip::{HeaderName, Method, StatusCode};
use sipx_transport::{Config as TransportConfig, Handle, Incoming, bind};
use tokio::task::JoinSet;
use tokio::time::Instant;
use tokio_util::sync::CancellationToken;

use crate::output::{Exit, Format, fail};

pub(crate) const HELP: &str = "\
sipx load-responder — answer a finite, bounded signalling load

USAGE:
    sipx load-responder --max-active <N> --cleanup <S> (--calls <N> | --duration <S>) [OPTIONS]

REQUIRED BOUNDS:
    --max-active <N>       Positive maximum simultaneously owned dialogs
    --calls <N>            Close admission after this many surfaced INVITEs
    --duration <S>         Close admission after this many seconds
    --cleanup <S>          Positive deadline for dialog, task and transaction drain

POLICY:
    --seed <N>                 Deterministic policy seed (default 0)
    --provisional-percent <P>  Percent receiving one 100 Trying (default 0)
    --answer-percent <P>       Percent receiving 200 rather than rejection (default 100)
    --reject-status <CODE>     Final 4xx-6xx for policy rejection (default 486)
    --dialog-duration <S>      Positive maximum accepted-dialog lifetime (default 40)
    --mode <M>                 signalling or generated-media (default signalling)

ENDPOINT:
    --local <ADDR>         UDP address to bind (default 127.0.0.1:0)
    --transport <T>        Must be udp; other profiles are separate (default udp)
    --json                 Emit terminal sipx.load-responder.v1 JSON after JSON readiness
";

const DEFAULT_DIALOG_DURATION: Duration = Duration::from_secs(40);
const PER_DIALOG_QUEUE: usize = 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    Signalling,
    GeneratedMedia,
}

impl Mode {
    fn parse(value: Option<&str>) -> Result<Self, String> {
        match value.unwrap_or("signalling") {
            "signalling" => Ok(Self::Signalling),
            "generated-media" => Ok(Self::GeneratedMedia),
            other => Err(format!(
                "--mode must be signalling or generated-media, not {other:?}"
            )),
        }
    }

    const fn as_str(self) -> &'static str {
        match self {
            Self::Signalling => "signalling",
            Self::GeneratedMedia => "generated-media",
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct Limits {
    calls: Option<usize>,
    duration: Option<Duration>,
    max_active: usize,
    cleanup: Duration,
    dialog_duration: Duration,
    seed: u64,
    provisional_percent: u8,
    answer_percent: u8,
    reject_status: u16,
    mode: Mode,
}

impl Limits {
    fn parse(args: &crate::Args<'_>) -> Result<Self, String> {
        if args.flag("tcp") || args.value("transport").is_some_and(|value| value != "udp") {
            return Err("load-responder v1 supports only --transport udp".to_owned());
        }
        let max_active = positive_usize(args.value("max-active"), "--max-active")?;
        if max_active > tokio::sync::Semaphore::MAX_PERMITS {
            return Err(format!(
                "--max-active must not exceed {}",
                tokio::sync::Semaphore::MAX_PERMITS
            ));
        }
        let calls = args
            .value("calls")
            .map(|value| positive_usize(Some(value), "--calls"))
            .transpose()?;
        let duration = args.number("duration").map(Duration::from_secs);
        if duration.is_some_and(|value| value.is_zero()) {
            return Err("--duration must be greater than zero".to_owned());
        }
        if calls.is_none() && duration.is_none() {
            return Err("load-responder requires --calls or --duration".to_owned());
        }
        let cleanup = required_positive_duration(args, "cleanup")?;
        let dialog_duration = match args.number("dialog-duration") {
            Some(0) => return Err("--dialog-duration must be greater than zero".to_owned()),
            Some(seconds) => Duration::from_secs(seconds),
            None => DEFAULT_DIALOG_DURATION,
        };
        let seed = args
            .value("seed")
            .unwrap_or("0")
            .parse::<u64>()
            .map_err(|_| "--seed must be an unsigned 64-bit integer".to_owned())?;
        let provisional_percent = percent(
            args.value("provisional-percent"),
            0,
            "--provisional-percent",
        )?;
        let answer_percent = percent(args.value("answer-percent"), 100, "--answer-percent")?;
        let reject_status = args
            .value("reject-status")
            .unwrap_or("486")
            .parse::<u16>()
            .map_err(|_| "--reject-status must be an integer from 400 through 699".to_owned())?;
        if !(400..=699).contains(&reject_status) {
            return Err("--reject-status must be an integer from 400 through 699".to_owned());
        }
        Ok(Self {
            calls,
            duration,
            max_active,
            cleanup,
            dialog_duration,
            seed,
            provisional_percent,
            answer_percent,
            reject_status,
            mode: Mode::parse(args.value("mode"))?,
        })
    }
}

fn positive_usize(value: Option<&str>, flag: &str) -> Result<usize, String> {
    let Some(value) = value else {
        return Err(format!("{flag} is required"));
    };
    let parsed = value
        .parse::<usize>()
        .map_err(|_| format!("{flag} must be a positive whole number"))?;
    if parsed == 0 {
        return Err(format!("{flag} must be greater than zero"));
    }
    Ok(parsed)
}

fn required_positive_duration(args: &crate::Args<'_>, name: &str) -> Result<Duration, String> {
    let flag = format!("--{name}");
    let Some(seconds) = args.number(name) else {
        return Err(format!("{flag} is required"));
    };
    if seconds == 0 {
        return Err(format!("{flag} must be greater than zero"));
    }
    Ok(Duration::from_secs(seconds))
}

fn percent(value: Option<&str>, default: u8, flag: &str) -> Result<u8, String> {
    let parsed = value
        .map_or(Ok(default), str::parse::<u8>)
        .map_err(|_| format!("{flag} must be an integer from 0 through 100"))?;
    if parsed > 100 {
        return Err(format!("{flag} must be an integer from 0 through 100"));
    }
    Ok(parsed)
}

#[derive(Debug, Clone, Copy)]
struct Decision {
    provisional: bool,
    answer: bool,
}

fn decision(limits: Limits, index: usize) -> Decision {
    let mut state = limits.seed ^ u64::try_from(index).unwrap_or(u64::MAX);
    let provisional = splitmix64(&mut state) % 100 < u64::from(limits.provisional_percent);
    let answer = splitmix64(&mut state) % 100 < u64::from(limits.answer_percent);
    Decision {
        provisional,
        answer,
    }
}

fn splitmix64(state: &mut u64) -> u64 {
    *state = state.wrapping_add(0x9e37_79b9_7f4a_7c15);
    let mut value = *state;
    value = (value ^ (value >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
    value = (value ^ (value >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
    value ^ (value >> 31)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Terminal {
    Completed,
    Cancelled,
    Rejected,
    Failed,
}

#[derive(Debug)]
struct WorkerResult {
    terminal: Terminal,
    established: bool,
    invalid: u64,
    responses: BTreeMap<u16, u64>,
    setup: Option<Duration>,
    teardown: Option<Duration>,
    error: Option<String>,
}

impl WorkerResult {
    fn new(terminal: Terminal) -> Self {
        Self {
            terminal,
            established: false,
            invalid: 0,
            responses: BTreeMap::new(),
            setup: None,
            teardown: None,
            error: None,
        }
    }

    fn failed(message: impl Into<String>) -> Self {
        let mut result = Self::new(Terminal::Failed);
        result.error = Some(message.into());
        result
    }

    fn response(&mut self, status: u16) {
        *self.responses.entry(status).or_default() += 1;
    }
}

#[derive(Debug, Default)]
struct Totals {
    invitations: u64,
    admitted: u64,
    established: u64,
    completed: u64,
    cancelled: u64,
    rejected: u64,
    failed: u64,
    invalid: u64,
    active_high_water: usize,
    responses: BTreeMap<u16, u64>,
    setup: Vec<Duration>,
    teardown: Vec<Duration>,
    first_error: Option<String>,
}

impl Totals {
    fn apply(&mut self, result: WorkerResult) {
        self.established += u64::from(result.established);
        match result.terminal {
            Terminal::Completed => self.completed += 1,
            Terminal::Cancelled => self.cancelled += 1,
            Terminal::Rejected => self.rejected += 1,
            Terminal::Failed => self.failed += 1,
        }
        self.invalid = self.invalid.saturating_add(result.invalid);
        for (status, count) in result.responses {
            *self.responses.entry(status).or_default() += count;
        }
        if let Some(setup) = result.setup {
            self.setup.push(setup);
        }
        if let Some(teardown) = result.teardown {
            self.teardown.push(teardown);
        }
        if self.first_error.is_none() {
            self.first_error = result.error;
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Completion {
    Completed,
    Interrupted,
    Failed,
}

impl Completion {
    const fn as_str(self) -> &'static str {
        match self {
            Self::Completed => "completed",
            Self::Interrupted => "interrupted",
            Self::Failed => "failed",
        }
    }
}

#[allow(
    clippy::too_many_lines,
    reason = "the command's admission, routing, cleanup barriers and final evidence remain in lifecycle order"
)]
pub(crate) async fn run(raw: &[String], format: Format) -> Exit {
    let args = match crate::arguments(raw, HELP, format) {
        Ok(args) => args,
        Err(exit) => return exit,
    };
    if let Some(positional) = args.positional() {
        return fail(
            format,
            Exit::Usage,
            &format!("load-responder takes no positional argument, got {positional:?}"),
        );
    }
    let limits = match Limits::parse(&args) {
        Ok(limits) => limits,
        Err(message) => return fail(format, Exit::Usage, &message),
    };
    let local: SocketAddr = match args.value("local").unwrap_or("127.0.0.1:0").parse() {
        Ok(local) => local,
        Err(_) => return fail(format, Exit::Usage, "--local must be an IP socket address"),
    };
    let mut config = TransportConfig::new(local);
    config.sent_by = local.ip().to_string();
    let (endpoint, incoming) = match bind(config).await {
        Ok(endpoint) => endpoint,
        Err(error) => return fail(format, Exit::Failed, &format!("bind: {error}")),
    };
    if let Err(error) = emit_readiness(&endpoint, limits) {
        endpoint.shutdown().await;
        return fail(
            format,
            Exit::Failed,
            &format!("could not flush readiness: {error}"),
        );
    }

    let mut dispatcher =
        sipx_call::Dispatcher::with_queue(endpoint.clone(), incoming, PER_DIALOG_QUEUE);
    let calls = dispatcher.calls();
    let mut workers = JoinSet::<WorkerResult>::new();
    let stop = CancellationToken::new();
    let started = Instant::now();
    let admission_deadline = limits
        .duration
        .and_then(|duration| started.checked_add(duration));
    let mut cleanup_deadline = None;
    let mut admission_open = true;
    let mut completion = Completion::Completed;
    let mut reason: Option<String> = None;
    let mut totals = Totals::default();
    let mut ctrl_c = std::pin::pin!(tokio::signal::ctrl_c());
    let mut signal_open = true;
    loop {
        if !admission_open && workers.is_empty() {
            let routes = calls.len();
            if routes == 0 {
                // Endpoint shutdown below is the transaction/timer cleanup barrier. Waiting for
                // Timer J on the wall clock would make successful finite runs idle for 32 seconds;
                // shutting down after every dialog route is gone cancels and joins that owned state.
                break;
            }
        }

        let selected = tokio::select! {
            biased;
            signal = &mut ctrl_c, if signal_open => {
                signal_open = false;
                match signal {
                    Ok(()) => LoopEvent::Interrupt,
                    Err(error) => LoopEvent::Internal(format!("signal handler failed: {error}")),
                }
            }
            () = sleep_until(admission_deadline), if admission_open && admission_deadline.is_some() => {
                LoopEvent::AdmissionElapsed
            }
            () = sleep_until(cleanup_deadline), if !admission_open && cleanup_deadline.is_some() => {
                LoopEvent::CleanupElapsed
            }
            joined = workers.join_next(), if !workers.is_empty() => LoopEvent::Worker(joined),
            surfaced = dispatcher.next() => LoopEvent::Dispatched(surfaced),
        };

        match selected {
            LoopEvent::Interrupt => {
                close_admission(&mut admission_open, &mut cleanup_deadline, limits.cleanup);
                completion = Completion::Interrupted;
                reason = Some("interrupt".to_owned());
                stop.cancel();
            }
            LoopEvent::Internal(message) => {
                close_admission(&mut admission_open, &mut cleanup_deadline, limits.cleanup);
                completion = Completion::Failed;
                reason.get_or_insert(message);
                stop.cancel();
            }
            LoopEvent::AdmissionElapsed => {
                close_admission(&mut admission_open, &mut cleanup_deadline, limits.cleanup);
            }
            LoopEvent::CleanupElapsed => {
                completion = Completion::Failed;
                reason.get_or_insert_with(|| "cleanup deadline expired".to_owned());
                stop.cancel();
                break;
            }
            LoopEvent::Worker(Some(Ok(result))) => {
                if result.terminal == Terminal::Failed {
                    close_admission(&mut admission_open, &mut cleanup_deadline, limits.cleanup);
                    completion = Completion::Failed;
                    stop.cancel();
                }
                totals.apply(result);
            }
            LoopEvent::Worker(Some(Err(error))) => {
                totals.failed += 1;
                close_admission(&mut admission_open, &mut cleanup_deadline, limits.cleanup);
                completion = Completion::Failed;
                reason.get_or_insert_with(|| format!("dialog worker failed to join: {error}"));
                stop.cancel();
            }
            LoopEvent::Worker(None) => {}
            LoopEvent::Dispatched(None) => {
                close_admission(&mut admission_open, &mut cleanup_deadline, limits.cleanup);
                completion = Completion::Failed;
                reason.get_or_insert_with(|| "endpoint receiver closed".to_owned());
                stop.cancel();
            }
            LoopEvent::Dispatched(Some(Dispatched::OutOfDialog(incoming))) => {
                totals.invalid += 1;
                if respond_out_of_dialog(&endpoint, &incoming).await.is_err() {
                    completion = Completion::Failed;
                    reason
                        .get_or_insert_with(|| "could not refuse out-of-dialog request".to_owned());
                    stop.cancel();
                    close_admission(&mut admission_open, &mut cleanup_deadline, limits.cleanup);
                } else {
                    *totals.responses.entry(405).or_default() += 1;
                }
            }
            LoopEvent::Dispatched(Some(Dispatched::Invitation(invitation))) => {
                let index = usize::try_from(totals.invitations).unwrap_or(usize::MAX);
                totals.invitations += 1;
                let reached_call_bound = limits
                    .calls
                    .is_some_and(|bound| totals.invitations >= bound as u64);
                let at_capacity = workers.len() >= limits.max_active;
                if !admission_open || at_capacity {
                    match invitation
                        .refuse(&endpoint, 503, "Service Unavailable")
                        .await
                    {
                        Ok(()) => {
                            totals.rejected += 1;
                            *totals.responses.entry(503).or_default() += 1;
                        }
                        Err(sipx_call::Error::InvitationCancelled) => totals.cancelled += 1,
                        Err(error) => {
                            totals.failed += 1;
                            completion = Completion::Failed;
                            reason
                                .get_or_insert_with(|| format!("overload refusal failed: {error}"));
                            stop.cancel();
                            close_admission(
                                &mut admission_open,
                                &mut cleanup_deadline,
                                limits.cleanup,
                            );
                        }
                    }
                    if reached_call_bound {
                        close_admission(&mut admission_open, &mut cleanup_deadline, limits.cleanup);
                    }
                    continue;
                }

                totals.admitted += 1;
                let endpoint = endpoint.clone();
                let calls_handle = calls.clone();
                let worker_stop = stop.clone();
                workers.spawn(async move {
                    run_invitation(
                        invitation,
                        index,
                        endpoint,
                        calls_handle,
                        limits,
                        worker_stop,
                    )
                    .await
                });
                totals.active_high_water = totals.active_high_water.max(workers.len());
                if reached_call_bound {
                    close_admission(&mut admission_open, &mut cleanup_deadline, limits.cleanup);
                }
            }
            LoopEvent::Dispatched(Some(_)) => {
                completion = Completion::Failed;
                reason.get_or_insert_with(|| "unsupported dispatcher event".to_owned());
                stop.cancel();
                close_admission(&mut admission_open, &mut cleanup_deadline, limits.cleanup);
            }
        }
    }

    if !workers.is_empty() {
        stop.cancel();
        workers.abort_all();
        while let Some(joined) = workers.join_next().await {
            if let Ok(result) = joined {
                totals.apply(result);
            }
        }
    }
    endpoint.shutdown().await;
    let routes = calls.len();
    // `Handle::shutdown` is the durable endpoint-driver barrier: every transaction and timer has
    // been cancelled and joined when it returns.
    let transactions = 0;
    let owned_tasks = workers.len();
    if routes != 0 || transactions != 0 || owned_tasks != 0 {
        completion = Completion::Failed;
        reason.get_or_insert_with(|| "non-zero state remained after cleanup".to_owned());
    }
    if completion == Completion::Failed && reason.is_none() {
        reason = totals.first_error.clone();
    }
    emit_summary(
        format,
        completion,
        limits,
        &totals,
        routes,
        transactions,
        owned_tasks,
        reason.as_deref(),
    );
    if completion == Completion::Failed {
        Exit::Failed
    } else {
        Exit::Success
    }
}

// `Dispatched` owns a parsed request and is deliberately larger than timer/control variants. It is
// selected once and immediately consumed, so boxing it would add an allocation to every packet.
#[allow(clippy::large_enum_variant)]
enum LoopEvent {
    Interrupt,
    Internal(String),
    AdmissionElapsed,
    CleanupElapsed,
    Worker(Option<Result<WorkerResult, tokio::task::JoinError>>),
    Dispatched(Option<Dispatched>),
}

fn close_admission(open: &mut bool, deadline: &mut Option<Instant>, cleanup: Duration) {
    if *open {
        *open = false;
        *deadline = Instant::now().checked_add(cleanup);
    }
}

async fn sleep_until(deadline: Option<Instant>) {
    match deadline {
        Some(deadline) => tokio::time::sleep_until(deadline).await,
        None => std::future::pending().await,
    }
}

fn emit_readiness(endpoint: &Handle, limits: Limits) -> std::io::Result<()> {
    let events = limits.max_active.saturating_mul(PER_DIALOG_QUEUE);
    let ready = serde_json::json!({
        "schema": "sipx.comparative-load.ready.v1",
        "role": "responder",
        "pid": std::process::id(),
        "address": endpoint.local_addr().to_string(),
        "transport": "udp",
        "limits": {
            "active": limits.max_active,
            "events": events,
            "stdout_bytes": 16 * 1024 * 1024,
            "stderr_bytes": 16 * 1024 * 1024,
        }
    });
    let mut stdout = std::io::stdout().lock();
    writeln!(stdout, "{ready}")?;
    stdout.flush()
}

async fn respond_out_of_dialog(endpoint: &Handle, incoming: &Incoming) -> sipx_call::Result<()> {
    let status = StatusCode::new(405).ok_or_else(|| sipx_call::Error::Rejected {
        status: 405,
        reason: "invalid refusal status".to_owned(),
    })?;
    let response = ResponseBuilder::to_request(&incoming.request, status, "Method Not Allowed")?
        .header(
            HeaderName::Allow,
            Bytes::from_static(b"INVITE, ACK, CANCEL, BYE"),
        )?
        .build();
    endpoint.respond(&incoming.key, response).await?;
    Ok(())
}

async fn run_invitation(
    invitation: Invitation,
    index: usize,
    endpoint: Handle,
    calls: Calls,
    limits: Limits,
    stop: CancellationToken,
) -> WorkerResult {
    match limits.mode {
        Mode::Signalling => run_signalling(invitation, index, endpoint, calls, limits, stop).await,
        Mode::GeneratedMedia => {
            run_generated_media(invitation, index, endpoint, calls, limits, stop).await
        }
    }
}

async fn apply_initial_policy(
    invitation: &Invitation,
    endpoint: &Handle,
    limits: Limits,
    index: usize,
    result: &mut WorkerResult,
) -> Result<bool, sipx_call::Error> {
    let choice = decision(limits, index);
    if choice.provisional {
        invitation.trying(endpoint).await?;
        result.response(100);
    }
    if !choice.answer {
        invitation
            .refuse(endpoint, limits.reject_status, "Policy Rejection")
            .await?;
        result.response(limits.reject_status);
        result.terminal = Terminal::Rejected;
        return Ok(false);
    }
    Ok(true)
}

async fn run_signalling(
    invitation: Invitation,
    index: usize,
    endpoint: Handle,
    calls: Calls,
    limits: Limits,
    stop: CancellationToken,
) -> WorkerResult {
    let mut result = WorkerResult::new(Terminal::Failed);
    match apply_initial_policy(&invitation, &endpoint, limits, index, &mut result).await {
        Ok(false) => return result,
        Err(sipx_call::Error::InvitationCancelled) => {
            result.terminal = Terminal::Cancelled;
            return result;
        }
        Err(error) => return WorkerResult::failed(format!("initial policy failed: {error}")),
        Ok(true) => {}
    }
    let tag = dialog_tag(invitation.request(), limits.seed, index);
    if sipx_call::Dialog::from_request(&invitation.request().request, &tag).is_none() {
        result.invalid = 1;
        match invitation.refuse(&endpoint, 400, "Bad Request").await {
            Ok(()) => result.response(400),
            Err(sipx_call::Error::InvitationCancelled) => {
                result.terminal = Terminal::Cancelled;
                return result;
            }
            Err(error) => result.error = Some(format!("malformed refusal failed: {error}")),
        }
        return result;
    }
    let contact = format!("<sip:load@{}>", endpoint.advertised());
    let setup_started = Instant::now();
    let mut call = match invitation
        .answer_signalling_with_tag(&endpoint, contact, tag)
        .await
    {
        Ok(call) => call,
        Err(sipx_call::Error::InvitationCancelled) => {
            result.terminal = Terminal::Cancelled;
            return result;
        }
        Err(error) => return WorkerResult::failed(format!("signalling answer failed: {error}")),
    };
    result.response(200);
    drive_signalling(&mut call, limits, stop, setup_started, &mut result).await;
    calls.forget(call.dialog());
    call.stop().await;
    result
}

async fn drive_signalling(
    call: &mut sipx_call::SignallingCall,
    limits: Limits,
    stop: CancellationToken,
    setup_started: Instant,
    result: &mut WorkerResult,
) {
    let deadline = Instant::now().checked_add(limits.dialog_duration);
    loop {
        enum Drive {
            Event(Option<SignallingEvent>),
            Stop,
            Deadline,
        }
        let action = tokio::select! {
            event = call.next() => Drive::Event(event),
            () = stop.cancelled() => Drive::Stop,
            () = sleep_until(deadline) => Drive::Deadline,
        };
        match action {
            Drive::Event(Some(SignallingEvent::Acknowledged)) => {
                if !result.established {
                    result.established = true;
                    result.setup = Some(setup_started.elapsed());
                }
            }
            Drive::Event(Some(SignallingEvent::RemoteBye)) if result.established => {
                result.terminal = Terminal::Completed;
                result.response(200);
                result.teardown = call.last_request_elapsed();
                return;
            }
            Drive::Event(Some(SignallingEvent::RemoteBye)) => {
                result.invalid += 1;
                result.error = Some("BYE arrived before a valid ACK".to_owned());
                return;
            }
            Drive::Event(Some(
                SignallingEvent::InvalidAck
                | SignallingEvent::InvalidDialog
                | SignallingEvent::InvalidCSeq
                | SignallingEvent::Unsupported,
            )) => {
                result.invalid += 1;
                result.error = Some("invalid in-dialog request".to_owned());
                terminate_signalling(call, limits.cleanup, result).await;
                return;
            }
            Drive::Event(Some(SignallingEvent::AckTimedOut)) => {
                result.error = Some("ACK timer expired".to_owned());
                return;
            }
            Drive::Event(Some(SignallingEvent::TransportFailed)) => {
                result.error = Some("final response retransmission failed".to_owned());
                return;
            }
            Drive::Event(Some(_)) => {
                result.error = Some("unknown signalling event".to_owned());
                return;
            }
            Drive::Event(None) => {
                result.error = Some("dialog inbox closed".to_owned());
                return;
            }
            Drive::Stop => {
                result.terminal = Terminal::Cancelled;
                terminate_signalling(call, limits.cleanup, result).await;
                return;
            }
            Drive::Deadline => {
                terminate_signalling(call, limits.cleanup, result).await;
                return;
            }
        }
    }
}

async fn terminate_signalling(
    call: &mut sipx_call::SignallingCall,
    within: Duration,
    result: &mut WorkerResult,
) {
    let started = Instant::now();
    match call.hang_up(within).await {
        Ok(status) => {
            result.response(status);
            result.teardown = Some(started.elapsed());
            if result.error.is_none() && result.terminal != Terminal::Cancelled {
                result.terminal = Terminal::Completed;
            }
        }
        Err(error) => result.error = Some(format!("dialog teardown failed: {error}")),
    }
}

async fn run_generated_media(
    invitation: Invitation,
    index: usize,
    endpoint: Handle,
    calls: Calls,
    limits: Limits,
    stop: CancellationToken,
) -> WorkerResult {
    let mut result = WorkerResult::new(Terminal::Failed);
    match apply_initial_policy(&invitation, &endpoint, limits, index, &mut result).await {
        Ok(false) => return result,
        Err(sipx_call::Error::InvitationCancelled) => {
            result.terminal = Terminal::Cancelled;
            return result;
        }
        Err(error) => return WorkerResult::failed(format!("initial policy failed: {error}")),
        Ok(true) => {}
    }
    if invitation.request().request.body().is_empty() {
        match invitation
            .refuse(&endpoint, 488, "Not Acceptable Here")
            .await
        {
            Ok(()) => {
                result.terminal = Terminal::Rejected;
                result.response(488);
            }
            Err(sipx_call::Error::InvitationCancelled) => result.terminal = Terminal::Cancelled,
            Err(error) => result.error = Some(format!("media refusal failed: {error}")),
        }
        return result;
    }
    let invite_cseq = request_cseq(&invitation.request().request)
        .filter(|value| value.method == Method::Invite)
        .map(|value| value.sequence);
    let setup_started = Instant::now();
    let mut call = match invitation
        .answer(&endpoint, endpoint.local_addr().ip())
        .await
    {
        Ok(call) => call,
        Err(sipx_call::Error::InvitationCancelled) => {
            result.terminal = Terminal::Cancelled;
            return result;
        }
        Err(error) => {
            let _ = invitation
                .refuse(&endpoint, 488, "Not Acceptable Here")
                .await;
            result.response(488);
            result.terminal = Terminal::Rejected;
            result.error = Some(format!("generated-media answer refused: {error}"));
            return result;
        }
    };
    result.response(200);
    let (_, mut requests) = invitation.into_parts();
    let deadline = Instant::now().checked_add(limits.dialog_duration);
    let acknowledged = wait_for_generated_ack(
        &mut call,
        &mut requests,
        invite_cseq,
        deadline,
        &stop,
        &mut result,
    )
    .await;
    if acknowledged {
        result.established = true;
        result.setup = Some(setup_started.elapsed());
        let frame = deterministic_frame(limits.seed, index);
        if call.media().play(&frame, frame.len()).await {
            drive_generated_media(&mut call, &mut requests, deadline, stop, &mut result).await;
        } else {
            result.error = Some("generated-media playback failed".to_owned());
        }
    }
    if !call.is_ended() {
        let started = Instant::now();
        match call.hang_up().await {
            Ok(()) => result.teardown = Some(started.elapsed()),
            Err(error) => result.error = Some(format!("generated-media teardown failed: {error}")),
        }
    }
    calls.forget(&call.dialog);
    result
}

async fn wait_for_generated_ack(
    call: &mut sipx_call::Call,
    requests: &mut tokio::sync::mpsc::Receiver<Incoming>,
    invite_cseq: Option<u32>,
    deadline: Option<Instant>,
    stop: &CancellationToken,
    result: &mut WorkerResult,
) -> bool {
    enum AwaitAck {
        Request(Option<Box<Incoming>>),
        Stop,
        Deadline,
    }
    let action = tokio::select! {
        incoming = requests.recv() => AwaitAck::Request(incoming.map(Box::new)),
        () = stop.cancelled() => AwaitAck::Stop,
        () = sleep_until(deadline) => AwaitAck::Deadline,
    };
    match action {
        AwaitAck::Request(Some(incoming)) => {
            let valid = incoming.request.method == Method::Ack
                && call.dialog.matches(&incoming.request)
                && request_cseq(&incoming.request).is_some_and(|value| {
                    value.method == Method::Ack && Some(value.sequence) == invite_cseq
                });
            if valid && call.handle(&incoming).await.unwrap_or(false) {
                return true;
            }
            result.invalid += 1;
            result.error = Some("generated-media ACK was invalid".to_owned());
        }
        AwaitAck::Request(None) => {
            result.error = Some("generated-media dialog inbox closed".to_owned());
        }
        AwaitAck::Stop => result.terminal = Terminal::Cancelled,
        AwaitAck::Deadline => {
            result.error = Some("generated-media ACK deadline expired".to_owned());
        }
    }
    false
}

async fn drive_generated_media(
    call: &mut sipx_call::Call,
    requests: &mut tokio::sync::mpsc::Receiver<Incoming>,
    deadline: Option<Instant>,
    stop: CancellationToken,
    result: &mut WorkerResult,
) {
    loop {
        enum Drive {
            Request(Option<Box<Incoming>>),
            Stop,
            Deadline,
        }
        let action = tokio::select! {
            incoming = requests.recv() => Drive::Request(incoming.map(Box::new)),
            () = stop.cancelled() => Drive::Stop,
            () = sleep_until(deadline) => Drive::Deadline,
        };
        match action {
            Drive::Request(Some(incoming)) => {
                let remote_bye = incoming.request.method == Method::Bye;
                let started = Instant::now();
                match call.handle(&incoming).await {
                    Ok(true) if remote_bye && call.is_ended() => {
                        result.terminal = Terminal::Completed;
                        result.response(200);
                        result.teardown = Some(started.elapsed());
                        return;
                    }
                    Ok(true) => {}
                    Ok(false) => {
                        result.invalid += 1;
                        result.error =
                            Some("unsupported generated-media dialog request".to_owned());
                        return;
                    }
                    Err(error) => {
                        result.error = Some(format!("generated-media request failed: {error}"));
                        return;
                    }
                }
            }
            Drive::Request(None) => {
                result.error = Some("generated-media dialog inbox closed".to_owned());
                return;
            }
            Drive::Stop => {
                result.terminal = Terminal::Cancelled;
                return;
            }
            Drive::Deadline => {
                result.terminal = Terminal::Completed;
                return;
            }
        }
    }
}

fn deterministic_frame(seed: u64, index: usize) -> [i16; 160] {
    let mut state = seed ^ u64::try_from(index).unwrap_or(u64::MAX).rotate_left(17);
    let mut frame = [0i16; 160];
    for sample in &mut frame {
        let [first, second, ..] = splitmix64(&mut state).to_be_bytes();
        *sample = i16::from_be_bytes([first, second]);
    }
    frame
}

fn dialog_tag(request: &Incoming, seed: u64, index: usize) -> String {
    let call_id = request
        .request
        .headers
        .value(&HeaderName::CallId)
        .unwrap_or_default();
    let profile = String::from_utf8_lossy(&call_id);
    let parsed = profile
        .strip_prefix("cl-")
        .and_then(|rest| rest.strip_suffix("@driver.invalid"))
        .and_then(|rest| rest.rsplit_once('-'))
        .and_then(|(run_id, number)| number.parse::<usize>().ok().map(|number| (run_id, number)));
    let seed_text = seed.to_string();
    let index_text;
    let fields: [&[u8]; 4] = if let Some((run_id, number)) = parsed {
        index_text = number.to_string();
        [
            seed_text.as_bytes(),
            run_id.as_bytes(),
            index_text.as_bytes(),
            b"to",
        ]
    } else {
        index_text = index.to_string();
        [
            seed_text.as_bytes(),
            call_id.as_ref(),
            index_text.as_bytes(),
            b"to",
        ]
    };
    let mut hash = Sha256::new();
    for (position, field) in fields.iter().enumerate() {
        if position != 0 {
            hash.update([0]);
        }
        hash.update(field);
    }
    let digest = hash.finalize();
    let mut hex = String::with_capacity(18);
    hex.push_str("t-");
    for byte in digest.iter().take(8) {
        use std::fmt::Write as _;
        let _ = write!(hex, "{byte:02x}");
    }
    hex
}

fn request_cseq(request: &sipx_sip::Request) -> Option<CSeq> {
    request
        .headers
        .typed::<CSeq>()
        .and_then(std::result::Result::ok)
}

fn percentile(values: &[Duration], numerator: usize) -> Option<u64> {
    if values.is_empty() {
        return None;
    }
    let mut values = values.to_vec();
    values.sort_unstable();
    let rank = values.len().saturating_mul(numerator).saturating_add(99) / 100;
    values
        .get(rank.saturating_sub(1).min(values.len().saturating_sub(1)))
        .map(|value| u64::try_from(value.as_millis()).unwrap_or(u64::MAX))
}

fn latency(values: &[Duration]) -> serde_json::Value {
    if values.is_empty() {
        return serde_json::Value::Null;
    }
    serde_json::json!({
        "count": values.len(),
        "p50": percentile(values, 50),
        "p95": percentile(values, 95),
        "p99": percentile(values, 99),
        "maximum": values.iter().map(Duration::as_millis).max().and_then(|value| u64::try_from(value).ok()),
    })
}

#[allow(clippy::too_many_arguments)]
fn emit_summary(
    format: Format,
    completion: Completion,
    limits: Limits,
    totals: &Totals,
    routes: usize,
    transactions: usize,
    owned_tasks: usize,
    reason: Option<&str>,
) {
    let responses: BTreeMap<String, u64> = totals
        .responses
        .iter()
        .map(|(status, count)| (status.to_string(), *count))
        .collect();
    let summary = serde_json::json!({
        "schema": "sipx.load-responder.v1",
        "status": completion.as_str(),
        "seed": limits.seed,
        "mode": limits.mode.as_str(),
        "limits": {
            "calls": limits.calls,
            "duration_ms": limits.duration.map(|value| u64::try_from(value.as_millis()).unwrap_or(u64::MAX)),
            "max_active": limits.max_active,
            "dialog_duration_ms": u64::try_from(limits.dialog_duration.as_millis()).unwrap_or(u64::MAX),
            "cleanup_ms": u64::try_from(limits.cleanup.as_millis()).unwrap_or(u64::MAX),
        },
        "counts": {
            "invitations": totals.invitations,
            "admitted": totals.admitted,
            "established": totals.established,
            "completed": totals.completed,
            "cancelled": totals.cancelled,
            "rejected": totals.rejected,
            "failed": totals.failed,
            "active_high_water": totals.active_high_water,
            "invalid_messages": totals.invalid,
        },
        "responses": responses,
        "latency_ms": {
            "setup": latency(&totals.setup),
            "teardown": latency(&totals.teardown),
        },
        "post_drain": {
            "active_dialogs": 0,
            "dispatcher_routes": routes,
            "endpoint_transactions": transactions,
            "owned_tasks": owned_tasks,
        },
        "reason": reason,
    });
    match format {
        Format::Json => println!("{summary}"),
        Format::Text => {
            println!("status             {}", completion.as_str());
            println!("invitations        {}", totals.invitations);
            println!("established        {}", totals.established);
            println!("completed          {}", totals.completed);
            println!("failed             {}", totals.failed);
            println!("summary_json       {summary}");
        }
    }
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
mod tests {
    use super::*;

    fn args(items: &[&str]) -> Vec<String> {
        items.iter().map(|item| (*item).to_owned()).collect()
    }

    #[test]
    fn responder_requires_every_lifecycle_bound() {
        for raw in [
            args(&["load-responder", "--max-active", "2", "--calls", "1"]),
            args(&["load-responder", "--cleanup", "40", "--calls", "1"]),
            args(&["load-responder", "--max-active", "2", "--cleanup", "40"]),
        ] {
            let parsed = crate::Args::new(&raw).expect("argument shape");
            assert!(Limits::parse(&parsed).is_err(), "{raw:?}");
        }
    }

    #[test]
    fn seeded_policy_and_tags_are_reproducible() {
        let limits = Limits {
            calls: Some(1),
            duration: None,
            max_active: 1,
            cleanup: Duration::from_secs(40),
            dialog_duration: Duration::from_secs(40),
            seed: 41,
            provisional_percent: 50,
            answer_percent: 50,
            reject_status: 486,
            mode: Mode::Signalling,
        };
        assert_eq!(
            decision(limits, 9).provisional,
            decision(limits, 9).provisional
        );
        assert_eq!(decision(limits, 9).answer, decision(limits, 9).answer);
        assert_ne!(splitmix64(&mut 1), splitmix64(&mut 2));
    }

    #[test]
    fn transport_and_policy_ranges_fail_before_io() {
        for raw in [
            args(&[
                "load-responder",
                "--max-active",
                "2",
                "--calls",
                "1",
                "--cleanup",
                "40",
                "--transport",
                "tcp",
            ]),
            args(&[
                "load-responder",
                "--max-active",
                "2",
                "--calls",
                "1",
                "--cleanup",
                "40",
                "--answer-percent",
                "101",
            ]),
            args(&[
                "load-responder",
                "--max-active",
                "2",
                "--calls",
                "1",
                "--cleanup",
                "40",
                "--reject-status",
                "200",
            ]),
        ] {
            let parsed = crate::Args::new(&raw).expect("argument shape");
            assert!(Limits::parse(&parsed).is_err(), "{raw:?}");
        }
    }
}
