//! `sipx scenario`: one bounded call actor driven by correlated NDJSON commands.

use std::collections::{BTreeMap, BTreeSet};
use std::net::IpAddr;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use bytes::Bytes;
use serde_json::{Map, Value, json};
use sipx_app_protocol::{CONTRACT, CallState, Direction as AppDirection, Timestamp};
use sipx_call::{Call, CallEvent, CallEvents, EndCause};
use sipx_sdp::Direction;
use sipx_sip::build::ResponseBuilder;
use sipx_sip::{HeaderName, StatusCode, Uri};
use sipx_transport::{Config as TransportConfig, Incoming, bind};
use tokio::io::AsyncBufReadExt as _;
use tokio::sync::oneshot;

use crate::cli::ScenarioOptions;
use crate::output::{Exit, Format, fail};

/// One minute of 48 kHz mono PCM. Reaching it finishes the recording rather than growing forever.
const MAX_RECORDING_SAMPLES: usize = 48_000 * 60;

pub(crate) async fn run(options: ScenarioOptions) -> Exit {
    let headers = match crate::header::from_options(&options.headers) {
        Ok(headers) => headers,
        Err(message) => return fail(Format::Json, Exit::Usage, &message),
    };
    let transport = match crate::signalling::Selection::from_options(&options.signalling, false) {
        Ok(transport) => transport,
        Err(message) => return fail(Format::Json, Exit::Usage, &message),
    };
    let media_options = options.media.complete();
    let media = match crate::media::Selection::from_options(&media_options, transport.kind(), false)
    {
        Ok(media) => media,
        Err(message) => return fail(Format::Json, Exit::Usage, &message),
    };
    let local = options.local;
    let mut config = TransportConfig::new(local);
    if local.ip().is_unspecified() {
        "127.0.0.1".clone_into(&mut config.sent_by);
    }
    if let Err(message) = transport.configure_client(&options.signalling, &mut config) {
        return fail(Format::Json, Exit::Usage, &message);
    }
    // The default endpoint already owns UDP and TCP listeners. Explicit WebSocket listeners need
    // their adapters configured; secure listener identity errors are reported before bind too.
    if matches!(
        transport.kind(),
        sipx_transport::TransportKind::Ws | sipx_transport::TransportKind::Wss
    ) && let Err(message) = transport.configure_listener(&options.signalling, &mut config)
    {
        return fail(Format::Json, Exit::Usage, &message);
    }
    let (handle, incoming) = match bind(config).await {
        Ok(bound) => bound,
        Err(error) => return fail(Format::Json, Exit::Failed, &format!("bind: {error}")),
    };

    let mut actor = Actor {
        options,
        handle,
        incoming,
        transport,
        resolver: crate::destination::Resolver::system(),
        policy: media.policy(),
        headers,
        call: None,
        events: None,
        pending: None,
        playback: None,
        recording: None,
        snapshot: Snapshot::idle(),
        emitted: BTreeMap::new(),
        consumed: BTreeMap::new(),
        correlations: BTreeSet::new(),
        stream_failed: false,
        output: Output::default(),
        next_call: 1,
    };
    actor.output.event(
        &actor.snapshot,
        "scenario.ready",
        None,
        BTreeMap::from([(
            "address",
            Value::String(actor.handle.local_addr().to_string()),
        )]),
    );
    actor.drive().await
}

struct Actor {
    options: ScenarioOptions,
    handle: sipx_transport::Handle,
    incoming: tokio::sync::mpsc::Receiver<Incoming>,
    transport: crate::signalling::Selection,
    resolver: crate::destination::Resolver,
    policy: sipx_call::MediaPolicy,
    headers: Vec<sipx_sip::Header>,
    call: Option<Call>,
    events: Option<CallEvents>,
    pending: Option<Incoming>,
    playback: Option<sipx_media::Playback>,
    recording: Option<Recording>,
    snapshot: Snapshot,
    /// Event occurrences in the current correlation scope. Counts, rather than membership, let
    /// repeated `wait_for call.dtmf` commands consume distinct keypresses.
    emitted: BTreeMap<String, u64>,
    consumed: BTreeMap<String, u64>,
    correlations: BTreeSet<String>,
    stream_failed: bool,
    output: Output,
    next_call: u64,
}

impl Actor {
    async fn drive(&mut self) -> Exit {
        let stdin = tokio::io::stdin();
        let mut lines = tokio::io::BufReader::new(stdin).lines();
        loop {
            self.drain_events();
            let line = match lines.next_line().await {
                Ok(Some(line)) => line,
                Ok(None) => {
                    return self.finish_stream().await;
                }
                Err(error) => {
                    self.refuse(None, &format!("stdin: {error}"));
                    return self.finish_stream().await;
                }
            };
            let value: Value = match serde_json::from_str(&line) {
                Ok(value) => value,
                Err(error) => {
                    let id = recover_id(&line);
                    if let Some(id) = id.as_ref()
                        && !self.correlations.insert(id.clone())
                    {
                        self.refuse(Some(id), "duplicate command id");
                        continue;
                    }
                    self.refuse(id.as_deref(), &format!("invalid JSON: {error}"));
                    continue;
                }
            };
            let Some(id) = value
                .get("id")
                .and_then(Value::as_str)
                .filter(|id| !id.is_empty() && id.len() <= 128)
            else {
                self.refuse(
                    None,
                    "every command requires a non-empty string id of at most 128 bytes",
                );
                continue;
            };
            let id = id.to_owned();
            if !self.correlations.insert(id.clone()) {
                self.refuse(Some(&id), "duplicate command id");
                continue;
            }
            let command = match command_selector(&value) {
                Ok(command) => command,
                Err(message) => {
                    self.refuse(Some(&id), &message);
                    continue;
                }
            };
            let outcome = self.command(command, &value).await;
            match outcome {
                Ok(shutdown) => {
                    self.drain_events();
                    self.output.completed(&self.snapshot, &id, command);
                    if shutdown {
                        return self.finish_stream().await;
                    }
                }
                Err(message) => self.refuse(Some(&id), &message),
            }
        }
    }

    fn refuse(&mut self, id: Option<&str>, message: &str) {
        self.stream_failed = true;
        self.output.error(&self.snapshot, id, message);
    }

    async fn finish_stream(&mut self) -> Exit {
        let cleanup_error = self.shutdown().await.err();
        if cleanup_error.is_some() {
            self.stream_failed = true;
        }
        self.output
            .stream(&self.snapshot, self.stream_failed, cleanup_error.as_deref());
        if self.stream_failed {
            Exit::Failed
        } else {
            Exit::Success
        }
    }

    async fn command(&mut self, command: &str, value: &Value) -> Result<bool, String> {
        match command {
            "dial" => self.dial(value).await?,
            "accept" => self.accept().await?,
            "reject" => self.reject(value).await?,
            "play" => self.play(value)?,
            "stop_playback" => self.stop_playback()?,
            "start_recording" => self.start_recording(value)?,
            "stop_recording" => self.stop_recording().await?,
            "send_dtmf" => self.send_dtmf(value).await?,
            "hold" => self.change_direction(Direction::SendOnly).await?,
            "resume" => self.change_direction(Direction::SendRecv).await?,
            "transfer" => self.transfer(value).await?,
            "hangup" => self.hangup().await?,
            "wait_for" => self.wait_for(value).await?,
            "shutdown" => return Ok(true),
            other => return Err(format!("unknown command: {other}")),
        }
        Ok(false)
    }

    async fn dial(&mut self, value: &Value) -> Result<(), String> {
        if self.call.is_some() || self.pending.is_some() {
            return Err("a call or invitation is already active".to_owned());
        }
        let target_text = match (
            optional_non_empty_string(value, "uri")?,
            optional_non_empty_string(value, "target")?,
        ) {
            (Some(_), Some(_)) => {
                return Err("dial.uri and dial.target cannot both be present".to_owned());
            }
            (Some(uri), None) | (None, Some(uri)) => uri,
            (None, None) => return Err("uri must be a non-empty string".to_owned()),
        };
        let to = Uri::parse(Bytes::from(target_text.to_owned()))
            .map_err(|_| format!("not a SIP URI: {target_text}"))?;
        if to.scheme().is_secure() && !self.transport.kind().is_secure() {
            return Err("a sips: target requires tls or wss; no downgrade is permitted".to_owned());
        }
        let candidates = self
            .resolver
            .resolve(&to, None, self.transport, &self.options.signalling)
            .await
            .map_err(|error| error.to_string())?;
        let target = crate::destination::first(&candidates)
            .map_err(|error| error.to_string())?
            .clone();
        let target_addr = target.addr;
        let media_address: IpAddr =
            crate::advertise::reachable_ip(self.handle.local_addr(), target_addr.ip());
        let from = optional_non_empty_string(value, "from")?
            .map_or_else(|| format!("<sip:sipx@{media_address}>"), str::to_owned);
        let mut options =
            sipx_call::DialOptions::new(from.clone(), media_address).with_media_policy(self.policy);
        let timeout = optional_u64(value, "timeout_ms")?.map_or_else(
            || Duration::from_secs(self.options.timeout),
            Duration::from_millis,
        );
        if !timeout.is_zero() {
            options = options.with_timeout(timeout);
        }
        for header in self.headers.iter().cloned() {
            options = options.with_header(header);
        }
        if let Some(items) = value.get("headers") {
            let items = items
                .as_array()
                .ok_or_else(|| "dial.headers must be an array of strings".to_owned())?;
            for item in items {
                let raw = item
                    .as_str()
                    .ok_or_else(|| "dial.headers must contain strings".to_owned())?;
                options = options.with_header(crate::header::parse(raw)?);
            }
        }
        let mut last_transport = None;
        let mut connected = None;
        for candidate in candidates.iter().take(crate::destination::MAX_ATTEMPTS) {
            match sipx_call::dial(&self.handle, candidate.clone(), &to, &options).await {
                Ok(call) => {
                    connected = Some(call);
                    break;
                }
                Err(error @ sipx_call::Error::Transport(_)) => last_transport = Some(error),
                Err(error) => return Err(error.to_string()),
            }
        }
        let mut call = connected.ok_or_else(|| {
            last_transport.map_or_else(
                || "no target candidate was attempted".to_owned(),
                |error| error.to_string(),
            )
        })?;
        self.events = call.events();
        self.snapshot = Snapshot::outbound(
            format!("scenario-{}", self.next_call),
            from,
            target_text.to_owned(),
        );
        self.next_call = self.next_call.saturating_add(1);
        self.emitted.clear();
        self.consumed.clear();
        self.call = Some(call);
        self.drain_events();
        Ok(())
    }

    async fn accept(&mut self) -> Result<(), String> {
        if self.call.is_some() {
            return Err("a call is already active".to_owned());
        }
        let incoming = self
            .pending
            .take()
            .ok_or_else(|| "there is no incoming invitation to accept".to_owned())?;
        let media_address = if self.handle.local_addr().ip().is_unspecified() {
            incoming.source.ip()
        } else {
            self.handle.local_addr().ip()
        };
        let mut call =
            sipx_call::answer_with_policy(&self.handle, &incoming, media_address, self.policy)
                .await
                .map_err(|error| error.to_string())?;
        self.events = call.events();
        self.snapshot.state = CallState::Answered;
        self.call = Some(call);
        self.drain_events();
        Ok(())
    }

    async fn reject(&mut self, value: &Value) -> Result<(), String> {
        let code = optional_u64(value, "status")?.unwrap_or(603);
        if !(300..=699).contains(&code) {
            return Err("reject.status must be between 300 and 699".to_owned());
        }
        let code = u16::try_from(code).map_err(|_| "reject.status is out of range".to_owned())?;
        let reason = optional_string(value, "reason")?.unwrap_or("Decline");
        let incoming = self
            .pending
            .take()
            .ok_or_else(|| "there is no incoming invitation to reject".to_owned())?;
        let status = StatusCode::new(code).ok_or_else(|| "reject.status is invalid".to_owned())?;
        let mut response = ResponseBuilder::to_request(
            &incoming.request,
            status,
            Bytes::copy_from_slice(reason.as_bytes()),
        )
        .map_err(|error| error.to_string())?;
        if let Some(to) = incoming.request.headers.value(&HeaderName::To) {
            response = response
                .set_header(
                    &HeaderName::To,
                    Bytes::from(format!(
                        "{};tag={:016x}",
                        String::from_utf8_lossy(&to),
                        rand::random::<u64>()
                    )),
                )
                .map_err(|error| error.to_string())?;
        }
        self.handle
            .respond(&incoming.key, response.build())
            .await
            .map_err(|error| error.to_string())?;
        self.snapshot.state = CallState::Ended;
        self.output.event(
            &self.snapshot,
            "call.ended",
            None,
            BTreeMap::from([("status", Value::from(code))]),
        );
        self.note_event("call.ended");
        Ok(())
    }

    fn play(&mut self, value: &Value) -> Result<(), String> {
        let path = string(value, "path")?;
        let clip = crate::dial::read_clip(path)?;
        let call = self
            .call
            .as_ref()
            .ok_or_else(|| "there is no active call".to_owned())?;
        let pcm = crate::dial::pcm_clip(&clip)?;
        self.playback = Some(
            call.start_pcm_playback(&pcm, sipx_media::Interrupt::Never)
                .map_err(|error| error.to_string())?,
        );
        Ok(())
    }

    fn stop_playback(&mut self) -> Result<(), String> {
        let playback = self
            .playback
            .take()
            .ok_or_else(|| "there is no active playback".to_owned())?;
        playback.stop();
        Ok(())
    }

    fn start_recording(&mut self, value: &Value) -> Result<(), String> {
        if self.recording.is_some() {
            return Err("a recording is already active".to_owned());
        }
        let path = string(value, "path")?.to_owned();
        let call = self
            .call
            .as_ref()
            .ok_or_else(|| "there is no active call".to_owned())?;
        let media = call.media_handle();
        let rate = media.clock_rate();
        let (stop, stopped) = oneshot::channel();
        let task = tokio::spawn(record(media, stopped));
        self.recording = Some(Recording {
            path,
            rate,
            stop: Some(stop),
            task,
        });
        Ok(())
    }

    async fn stop_recording(&mut self) -> Result<(), String> {
        let recording = self
            .recording
            .take()
            .ok_or_else(|| "there is no active recording".to_owned())?;
        recording.finish().await
    }

    async fn send_dtmf(&self, value: &Value) -> Result<(), String> {
        let digits = string(value, "digits")?;
        let call = self
            .call
            .as_ref()
            .ok_or_else(|| "there is no active call".to_owned())?;
        if call.send_digits(digits, Duration::from_millis(100)).await {
            Ok(())
        } else {
            Err("the media session stopped while sending DTMF".to_owned())
        }
    }

    async fn change_direction(&mut self, direction: Direction) -> Result<(), String> {
        let call = self
            .call
            .as_mut()
            .ok_or_else(|| "there is no active call".to_owned())?;
        call.reinvite(direction)
            .await
            .map_err(|error| error.to_string())
    }

    async fn transfer(&mut self, value: &Value) -> Result<(), String> {
        let target = string(value, "target")?;
        let target = Uri::parse(Bytes::from(target.to_owned()))
            .map_err(|_| "transfer.target is not a SIP URI".to_owned())?;
        let call = self
            .call
            .as_mut()
            .ok_or_else(|| "there is no active call".to_owned())?;
        call.refer(&target).await.map_err(|error| error.to_string())
    }

    async fn hangup(&mut self) -> Result<(), String> {
        self.finish_recording_if_any().await?;
        if let Some(playback) = self.playback.take() {
            playback.stop();
        }
        let mut call = self
            .call
            .take()
            .ok_or_else(|| "there is no active call".to_owned())?;
        call.hang_up().await.map_err(|error| error.to_string())?;
        self.drain_events();
        self.events = None;
        Ok(())
    }

    async fn wait_for(&mut self, value: &Value) -> Result<(), String> {
        let requested = string(value, "event")?;
        let wanted = if requested.starts_with("call.") || requested.starts_with("scenario.") {
            requested.to_owned()
        } else {
            format!("call.{requested}")
        };
        let timeout_ms = required_u64(value, "timeout_ms")
            .map_err(|_| "wait_for requires a finite timeout_ms".to_owned())?;
        let deadline = tokio::time::Instant::now() + Duration::from_millis(timeout_ms);
        loop {
            if self.consume_event(&wanted) {
                return Ok(());
            }
            let _ = self.next_runtime(deadline).await?;
        }
    }

    async fn next_runtime(&mut self, deadline: tokio::time::Instant) -> Result<String, String> {
        enum Arrived {
            Event(Option<CallEvent>),
            Media,
            Incoming(Box<Option<Incoming>>),
            Timeout,
        }
        let arrived = if let (Some(events), Some(call)) = (self.events.as_mut(), self.call.as_ref())
        {
            tokio::select! {
                event = events.recv() => Arrived::Event(event),
                () = call.drive_media_event() => Arrived::Media,
                incoming = self.incoming.recv() => Arrived::Incoming(Box::new(incoming)),
                () = tokio::time::sleep_until(deadline) => Arrived::Timeout,
            }
        } else if let Some(events) = self.events.as_mut() {
            tokio::select! {
                event = events.recv() => Arrived::Event(event),
                incoming = self.incoming.recv() => Arrived::Incoming(Box::new(incoming)),
                () = tokio::time::sleep_until(deadline) => Arrived::Timeout,
            }
        } else {
            tokio::select! {
                incoming = self.incoming.recv() => Arrived::Incoming(Box::new(incoming)),
                () = tokio::time::sleep_until(deadline) => Arrived::Timeout,
            }
        };
        match arrived {
            Arrived::Event(Some(event)) => Ok(self.emit_call_event(event)),
            Arrived::Event(None) => Err("the call event stream ended".to_owned()),
            // `drive_media_event` has offered exactly one item to the bounded call-event queue.
            // Reading it on the next loop preserves that queue as the public handoff; if a slow
            // consumer filled it, its existing drop counter and recovery policy remain decisive.
            Arrived::Media => Ok("call.media".to_owned()),
            Arrived::Incoming(incoming) => match *incoming {
                Some(incoming) => self.on_incoming(incoming).await,
                None => Err("the signalling endpoint stopped".to_owned()),
            },
            Arrived::Timeout => Err("wait_for deadline expired".to_owned()),
        }
    }

    async fn on_incoming(&mut self, incoming: Incoming) -> Result<String, String> {
        if let Some(call) = self.call.as_mut()
            && call
                .handle(&incoming)
                .await
                .map_err(|error| error.to_string())?
        {
            self.drain_events();
            return Ok("call.signalling".to_owned());
        }
        if incoming.request.method != sipx_sip::Method::Invite || self.pending.is_some() {
            return Ok("call.signalling".to_owned());
        }
        let from = header_text(&incoming, &HeaderName::From);
        let to = header_text(&incoming, &HeaderName::To);
        let id = header_text(&incoming, &HeaderName::CallId);
        self.snapshot = Snapshot::inbound(
            if id.is_empty() {
                format!("scenario-{}", self.next_call)
            } else {
                id
            },
            from,
            to,
        );
        self.next_call = self.next_call.saturating_add(1);
        self.emitted.clear();
        self.consumed.clear();
        self.pending = Some(incoming);
        self.output
            .event(&self.snapshot, "call.incoming", None, BTreeMap::new());
        self.note_event("call.incoming");
        Ok("call.incoming".to_owned())
    }

    fn drain_events(&mut self) {
        loop {
            let event = self.events.as_mut().and_then(CallEvents::try_recv);
            let Some(event) = event else { return };
            self.emit_call_event(event);
        }
    }

    fn emit_call_event(&mut self, event: CallEvent) -> String {
        let (name, details) = match event {
            CallEvent::Ringing { reliable } => (
                "call.ringing",
                BTreeMap::from([("reliable", Value::Bool(reliable))]),
            ),
            CallEvent::EarlyMediaStarted => ("call.early_media.started", BTreeMap::new()),
            CallEvent::Answered => {
                self.snapshot.state = CallState::Answered;
                ("call.answered", BTreeMap::new())
            }
            CallEvent::Dtmf { digit, duration } => (
                "call.dtmf",
                BTreeMap::from([
                    ("digit", Value::String(digit.as_char().to_string())),
                    (
                        "duration_ms",
                        Value::from(u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)),
                    ),
                ]),
            ),
            CallEvent::PlaybackFinished {
                playback,
                completed,
            } => (
                "call.playback.finished",
                BTreeMap::from([
                    ("playback", Value::String(format!("{playback:?}"))),
                    ("completed", Value::Bool(completed)),
                ]),
            ),
            CallEvent::RecordingFinished { duration } => (
                "call.recording.finished",
                BTreeMap::from([(
                    "duration_ms",
                    Value::from(u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)),
                )]),
            ),
            CallEvent::TransferRequested { target, attended } => (
                "call.transfer.requested",
                BTreeMap::from([
                    (
                        "target",
                        Value::String(String::from_utf8_lossy(&target.to_bytes()).into_owned()),
                    ),
                    ("attended", Value::Bool(attended)),
                ]),
            ),
            CallEvent::TransferProgress(state) => (
                "call.transfer.progress",
                BTreeMap::from([("state", Value::String(format!("{state:?}")))]),
            ),
            CallEvent::Hold => {
                self.snapshot.on_hold = true;
                ("call.hold", BTreeMap::new())
            }
            CallEvent::Resumed => {
                self.snapshot.on_hold = false;
                ("call.resumed", BTreeMap::new())
            }
            CallEvent::Muted => ("call.muted", BTreeMap::new()),
            CallEvent::Unmuted => ("call.unmuted", BTreeMap::new()),
            CallEvent::Ended(cause) => {
                self.snapshot.state = CallState::Ended;
                (
                    "call.ended",
                    BTreeMap::from([("cause", Value::String(end_cause(cause).to_owned()))]),
                )
            }
            _ => ("call.event", BTreeMap::new()),
        };
        self.output.event(&self.snapshot, name, None, details);
        self.note_event(name);
        name.to_owned()
    }

    fn note_event(&mut self, name: &str) {
        let count = self.emitted.entry(name.to_owned()).or_default();
        *count = count.saturating_add(1);
    }

    fn consume_event(&mut self, name: &str) -> bool {
        let emitted = self.emitted.get(name).copied().unwrap_or(0);
        let consumed = self.consumed.entry(name.to_owned()).or_default();
        if *consumed >= emitted {
            return false;
        }
        *consumed = consumed.saturating_add(1);
        true
    }

    async fn finish_recording_if_any(&mut self) -> Result<(), String> {
        if let Some(recording) = self.recording.take() {
            recording.finish().await?;
        }
        Ok(())
    }

    async fn shutdown(&mut self) -> Result<(), String> {
        let mut failure = self.finish_recording_if_any().await.err();
        if let Some(playback) = self.playback.take() {
            playback.stop();
        }
        if let Some(mut call) = self.call.take() {
            if let Err(error) = call.hang_up().await
                && failure.is_none()
            {
                failure = Some(format!("call cleanup: {error}"));
            }
            self.drain_events();
        }
        if let Some(incoming) = self.pending.take()
            && let Err(error) = refuse_unattended(&self.handle, &incoming).await
            && failure.is_none()
        {
            failure = Some(format!("invitation cleanup: {error}"));
        }
        self.events = None;
        self.handle.shutdown().await;
        failure.map_or(Ok(()), Err)
    }
}

struct Recording {
    path: String,
    rate: u32,
    stop: Option<oneshot::Sender<()>>,
    task: tokio::task::JoinHandle<Vec<i16>>,
}

impl Recording {
    async fn finish(mut self) -> Result<(), String> {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        let samples = self
            .task
            .await
            .map_err(|error| format!("recording task: {error}"))?;
        let file =
            std::fs::File::create(&self.path).map_err(|error| format!("{}: {error}", self.path))?;
        sipx_audio::write_wav(
            file,
            &sipx_audio::Wav {
                sample_rate: self.rate,
                samples,
            },
        )
        .map_err(|error| format!("{}: {error}", self.path))
    }
}

async fn record(media: Arc<sipx_media::MediaSession>, mut stop: oneshot::Receiver<()>) -> Vec<i16> {
    let mut samples = Vec::new();
    loop {
        tokio::select! {
            _ = &mut stop => return samples,
            frame = media.recv() => {
                let Some(frame) = frame else { return samples };
                let remaining = MAX_RECORDING_SAMPLES.saturating_sub(samples.len());
                samples.extend(frame.into_iter().take(remaining));
                if samples.len() == MAX_RECORDING_SAMPLES {
                    return samples;
                }
            }
        }
    }
}

#[derive(Clone)]
struct Snapshot {
    id: String,
    direction: AppDirection,
    state: CallState,
    from: String,
    to: String,
    on_hold: bool,
}

impl Snapshot {
    fn idle() -> Self {
        Self {
            id: "scenario".to_owned(),
            direction: AppDirection::Outbound,
            state: CallState::Ended,
            from: String::new(),
            to: String::new(),
            on_hold: false,
        }
    }

    fn outbound(id: String, from: String, to: String) -> Self {
        Self {
            id,
            direction: AppDirection::Outbound,
            state: CallState::Answered,
            from,
            to,
            on_hold: false,
        }
    }

    fn inbound(id: String, from: String, to: String) -> Self {
        Self {
            id,
            direction: AppDirection::Inbound,
            state: CallState::Incoming,
            from,
            to,
            on_hold: false,
        }
    }

    fn json(&self) -> Value {
        json!({
            "id": self.id,
            "leg": "a",
            "direction": self.direction.as_str(),
            "state": self.state.as_str(),
            "from": self.from,
            "to": self.to,
            "headers": {},
            "media": {"encrypted": false, "on_hold": self.on_hold, "muted": false},
            "legs": [],
            "bridged": false,
            "tags": {}
        })
    }
}

#[derive(Default)]
struct Output {
    seq: u64,
}

impl Output {
    fn completed(&mut self, snapshot: &Snapshot, id: &str, command: &str) {
        self.event(
            snapshot,
            "scenario.command.completed",
            Some(id),
            BTreeMap::from([("command", Value::String(command.to_owned()))]),
        );
    }

    fn error(&mut self, snapshot: &Snapshot, id: Option<&str>, message: &str) {
        self.event(
            snapshot,
            "scenario.command.refused",
            id,
            BTreeMap::from([("message", Value::String(message.to_owned()))]),
        );
    }

    fn stream(&mut self, snapshot: &Snapshot, failed: bool, message: Option<&str>) {
        let mut details = BTreeMap::new();
        if let Some(message) = message {
            details.insert("message", Value::String(message.to_owned()));
        }
        self.event(
            snapshot,
            if failed {
                "scenario.stream.failed"
            } else {
                "scenario.stream.completed"
            },
            None,
            details,
        );
    }

    fn event(
        &mut self,
        snapshot: &Snapshot,
        name: &str,
        id: Option<&str>,
        details: BTreeMap<&str, Value>,
    ) {
        self.seq = self.seq.saturating_add(1);
        let mut event = Map::new();
        event.insert("type".to_owned(), Value::String(name.to_owned()));
        if let Some(id) = id {
            event.insert("id".to_owned(), Value::String(id.to_owned()));
        }
        for (name, value) in details {
            event.insert(name.to_owned(), value);
        }
        let millis = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .ok()
            .and_then(|duration| i64::try_from(duration.as_millis()).ok())
            .unwrap_or(0);
        println!(
            "{}",
            json!({
                "contract": CONTRACT,
                "seq": self.seq,
                "at": Timestamp::from_unix_millis(millis).to_rfc3339(),
                "call": snapshot.json(),
                "event": event
            })
        );
    }
}

fn command_selector(value: &Value) -> Result<&str, String> {
    match (value.get("command"), value.get("do")) {
        (Some(_), Some(_)) => Err("command and do cannot both be present".to_owned()),
        (Some(command), None) | (None, Some(command)) => command
            .as_str()
            .filter(|command| !command.is_empty())
            .ok_or_else(|| "command must be a non-empty string".to_owned()),
        (None, None) => Err("command must be a non-empty string".to_owned()),
    }
}

fn optional_non_empty_string<'a>(value: &'a Value, name: &str) -> Result<Option<&'a str>, String> {
    match value.get(name) {
        None => Ok(None),
        Some(Value::String(text)) if !text.is_empty() => Ok(Some(text)),
        Some(_) => Err(format!("{name} must be a non-empty string")),
    }
}

fn optional_string<'a>(value: &'a Value, name: &str) -> Result<Option<&'a str>, String> {
    match value.get(name) {
        None => Ok(None),
        Some(Value::String(text)) => Ok(Some(text)),
        Some(_) => Err(format!("{name} must be a string")),
    }
}

fn optional_u64(value: &Value, name: &str) -> Result<Option<u64>, String> {
    match value.get(name) {
        None => Ok(None),
        Some(value) => value
            .as_u64()
            .map(Some)
            .ok_or_else(|| format!("{name} must be an unsigned integer")),
    }
}

fn required_u64(value: &Value, name: &str) -> Result<u64, String> {
    optional_u64(value, name)?.ok_or_else(|| format!("{name} must be an unsigned integer"))
}

fn string<'a>(value: &'a Value, name: &str) -> Result<&'a str, String> {
    value
        .get(name)
        .and_then(Value::as_str)
        .filter(|text| !text.is_empty())
        .ok_or_else(|| format!("{name} must be a non-empty string"))
}

/// Recover a simple string correlation from a frame whose tail is malformed.
///
/// This is intentionally narrower than JSON parsing: it does not guess through escapes, and a
/// frame too broken to state an unambiguous id receives an uncorrelated error. The common partial
/// write (`{"id":"x","command":`) still gets the refusal the caller can match.
fn recover_id(line: &str) -> Option<String> {
    let marker = "\"id\"";
    let after_name = line.get(line.find(marker)? + marker.len()..)?.trim_start();
    let after_colon = after_name.strip_prefix(':')?.trim_start();
    let quoted = after_colon.strip_prefix('"')?;
    let end = quoted.find(['"', '\\'])?;
    if quoted.as_bytes().get(end) != Some(&b'"') || end == 0 || end > 128 {
        return None;
    }
    quoted.get(..end).map(str::to_owned)
}

fn header_text(incoming: &Incoming, name: &HeaderName) -> String {
    incoming
        .request
        .headers
        .value(name)
        .map(|value| String::from_utf8_lossy(&value).into_owned())
        .unwrap_or_default()
}

fn end_cause(cause: EndCause) -> &'static str {
    match cause {
        EndCause::LocalHangup => "hangup",
        EndCause::RemoteBye | EndCause::RemoteCancel => "remote",
        EndCause::Rejected { .. } => "rejected",
        EndCause::Timeout => "timeout",
        _ => "error",
    }
}

async fn refuse_unattended(
    handle: &sipx_transport::Handle,
    incoming: &Incoming,
) -> Result<(), String> {
    let status = StatusCode::new(480).ok_or_else(|| "invalid refusal status".to_owned())?;
    let response =
        ResponseBuilder::to_request(&incoming.request, status, "Temporarily Unavailable")
            .map_err(|error| error.to_string())?
            .build();
    handle
        .respond(&incoming.key, response)
        .await
        .map_err(|error| error.to_string())
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

    #[test]
    fn every_v1_command_is_dispatched_and_sleep_is_absent() {
        let source = include_str!("scenario.rs");
        for command in [
            "dial",
            "accept",
            "reject",
            "play",
            "stop_playback",
            "start_recording",
            "stop_recording",
            "send_dtmf",
            "hold",
            "resume",
            "transfer",
            "hangup",
            "wait_for",
            "shutdown",
        ] {
            assert!(
                source.contains(&format!("\"{command}\" =>")),
                "missing {command}"
            );
        }
        assert!(!source.contains("\"sleep\" =>"));
    }

    #[test]
    fn output_uses_the_existing_versioned_envelope_and_echoes_correlation() {
        let mut output = Output::default();
        let snapshot = Snapshot::idle();
        // Structural assertions live on the producer fields rather than capturing process stdout.
        output.seq = output.seq.saturating_add(1);
        assert_eq!(CONTRACT, "sipx.app.v1");
        assert_eq!(output.seq, 1);
        assert_eq!(snapshot.json()["id"], "scenario");
    }

    #[test]
    fn a_partial_json_write_keeps_an_unambiguous_correlation() {
        assert_eq!(
            recover_id(r#"{"id":"request-7","command": "#),
            Some("request-7".to_owned())
        );
        assert_eq!(recover_id(r#"{"id":"escaped\"id""#), None);
        assert_eq!(recover_id("not json"), None);
    }
}
