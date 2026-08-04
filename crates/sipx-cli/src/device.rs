//! Feature-gated live audio for the diagnostic phone.
//!
//! Device callbacks live here, at the command leaf. They exchange bounded PCM frames with a media
//! session and never enter `sipx-audio` or `sipx-media`.

use crate::Args;
use crate::output::{Exit, Format, Report, fail};

pub(crate) const HELP: &str = "\
sipx devices — list stable audio device identifiers

USAGE:
    sipx devices [--json]

The command opens no stream. Device support requires the `device-audio` build feature.
";

/// A command's two local audio endpoints.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Selection {
    input: Endpoint,
    output: Endpoint,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Endpoint {
    Null,
    Wav(String),
    Device(String),
}

impl Selection {
    /// Resolve the new endpoint spelling and the two existing WAV aliases before any I/O.
    pub(crate) fn from_args(args: &Args<'_>) -> Result<Self, String> {
        let input = endpoint(
            args.value("audio-input"),
            args.value("play"),
            "input",
            "play",
        )?;
        let output = endpoint(
            args.value("audio-output"),
            args.value("record"),
            "output",
            "record",
        )?;
        Ok(Self { input, output })
    }

    pub(crate) fn wav_input(&self) -> Option<&str> {
        match &self.input {
            Endpoint::Wav(path) => Some(path),
            Endpoint::Null | Endpoint::Device(_) => None,
        }
    }

    pub(crate) fn wav_output(&self) -> Option<&str> {
        match &self.output {
            Endpoint::Wav(path) => Some(path),
            Endpoint::Null | Endpoint::Device(_) => None,
        }
    }

    /// Open explicit devices paused. This is called before transport bind.
    pub(crate) fn open(&self) -> Result<Driver, String> {
        let input = match &self.input {
            Endpoint::Device(id) => Some(id.as_str()),
            Endpoint::Null | Endpoint::Wav(_) => None,
        };
        let output = match &self.output {
            Endpoint::Device(id) => Some(id.as_str()),
            Endpoint::Null | Endpoint::Wav(_) => None,
        };
        Driver::open(input, output)
    }
}

fn endpoint(
    explicit: Option<&str>,
    wav_alias: Option<&str>,
    direction: &str,
    alias: &str,
) -> Result<Endpoint, String> {
    if explicit.is_some() && wav_alias.is_some() {
        return Err(format!(
            "--audio-{direction} and --{alias} name the same audio {direction}; choose one"
        ));
    }
    let Some(value) = explicit else {
        return Ok(wav_alias.map_or(Endpoint::Null, |path| Endpoint::Wav(path.to_owned())));
    };
    if value == "null" {
        return Ok(Endpoint::Null);
    }
    let Some((kind, value)) = value.split_once(':') else {
        return Err(format!(
            "--audio-{direction} must be wav:<path>, device:<id> or null"
        ));
    };
    if value.is_empty() {
        return Err(format!("--audio-{direction} {kind}: has no value"));
    }
    match kind {
        "wav" => Ok(Endpoint::Wav(value.to_owned())),
        "device" => Ok(Endpoint::Device(value.to_owned())),
        "generator" => Err(format!(
            "--audio-{direction} generator:{value} is not shipped yet"
        )),
        _ => Err(format!(
            "unsupported --audio-{direction} kind {kind:?}; expected wav, device or null"
        )),
    }
}

/// List stable identifiers without opening a stream.
pub(crate) fn list(raw: &[String], format: Format) -> Exit {
    if crate::wants_help(raw) {
        print!("{HELP}");
        return Exit::Success;
    }
    #[cfg(feature = "device-audio")]
    {
        match enabled::devices() {
            Ok(devices) => {
                match format {
                    Format::Json => {
                        let value = serde_json::json!({
                            "schema": "sipx.devices.v1",
                            "devices": devices.iter().map(DeviceInfo::json).collect::<Vec<_>>(),
                        });
                        println!("{value}");
                    }
                    Format::Text => {
                        for device in devices {
                            let directions = match (device.input, device.output) {
                                (true, true) => "input,output",
                                (true, false) => "input",
                                (false, true) => "output",
                                (false, false) => "none",
                            };
                            println!("{}\t{directions}\t{}", device.id, device.name);
                        }
                    }
                }
                Exit::Success
            }
            Err(message) => fail(format, Exit::Failed, &message),
        }
    }
    #[cfg(not(feature = "device-audio"))]
    {
        fail(
            format,
            Exit::Failed,
            "audio devices require a build with the `device-audio` feature",
        )
    }
}

#[cfg(feature = "device-audio")]
#[derive(Debug, Clone, PartialEq, Eq)]
struct DeviceInfo {
    id: String,
    name: String,
    input: bool,
    output: bool,
}

#[cfg(feature = "device-audio")]
impl DeviceInfo {
    fn json(&self) -> serde_json::Value {
        serde_json::json!({
            "id": self.id,
            "name": self.name,
            "input": self.input,
            "output": self.output,
        })
    }
}

/// Paused device streams and their bounded callback queues.
#[cfg(feature = "device-audio")]
pub(crate) struct Driver(enabled::Driver);

#[cfg(not(feature = "device-audio"))]
#[derive(Debug)]
pub(crate) struct Driver;

impl Driver {
    #[cfg(feature = "device-audio")]
    fn open(input: Option<&str>, output: Option<&str>) -> Result<Self, String> {
        enabled::Driver::open(input, output).map(Self)
    }

    #[cfg(not(feature = "device-audio"))]
    fn open(input: Option<&str>, output: Option<&str>) -> Result<Self, String> {
        if input.is_some() || output.is_some() {
            return Err("audio devices require a build with the `device-audio` feature".to_owned());
        }
        Ok(Self)
    }

    #[must_use]
    pub(crate) fn has_output(&self) -> bool {
        #[cfg(feature = "device-audio")]
        {
            self.0.has_output()
        }
        #[cfg(not(feature = "device-audio"))]
        {
            let _ = self;
            false
        }
    }

    /// Run every selected stream until the call duration ends, then causally join both relays.
    pub(crate) async fn run(
        &mut self,
        media: &sipx_media::MediaSession,
        duration: std::time::Duration,
    ) -> Result<u64, String> {
        #[cfg(feature = "device-audio")]
        {
            self.0.run(media, duration).await
        }
        #[cfg(not(feature = "device-audio"))]
        {
            let _ = (self, media, duration);
            std::future::ready(()).await;
            Ok(0)
        }
    }

    /// Add selected configurations and loss counters to the terminal result.
    #[must_use]
    pub(crate) fn report(&self, report: Report) -> Report {
        #[cfg(feature = "device-audio")]
        {
            self.0.report(report)
        }
        #[cfg(not(feature = "device-audio"))]
        {
            let _ = self;
            report
        }
    }
}

#[cfg(feature = "device-audio")]
mod enabled {
    use std::collections::VecDeque;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
    use std::time::Duration;

    use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
    use cpal::{BufferSize, Data, Device, SampleFormat, Stream, StreamConfig};
    use tokio::sync::{mpsc, watch};

    use super::{DeviceInfo, Report};

    const MEDIA_QUEUE_FRAMES: usize = 50;
    const MAX_CHANNELS: u16 = 32;
    const MAX_DEVICE_RATE: u32 = 384_000;
    const MAX_CALLBACK_SAMPLES: usize = 1_048_576;
    const MAX_DEVICES: usize = 1_024;
    const OPEN_TIMEOUT: Duration = Duration::from_secs(2);

    pub(super) fn devices() -> Result<Vec<DeviceInfo>, String> {
        let mut listed = Vec::new();
        for host_id in cpal::available_hosts() {
            let host = cpal::host_from_id(host_id)
                .map_err(|error| format!("audio host {host_id}: {error}"))?;
            let devices = host
                .devices()
                .map_err(|error| format!("enumerate audio host {host_id}: {error}"))?;
            for device in devices {
                let id = device
                    .id()
                    .map_err(|error| format!("read audio device identifier: {error}"))?
                    .to_string();
                let name = device
                    .description()
                    .map_err(|error| format!("describe audio device {id}: {error}"))?
                    .name()
                    .to_owned();
                listed.push(DeviceInfo {
                    id,
                    name,
                    input: device.supports_input(),
                    output: device.supports_output(),
                });
                if listed.len() > MAX_DEVICES {
                    return Err(format!(
                        "audio device enumeration exceeds the {MAX_DEVICES}-identifier bound"
                    ));
                }
            }
        }
        listed.sort_by(|left, right| left.id.cmp(&right.id));
        listed.dedup_by(|left, right| left.id == right.id);
        Ok(listed)
    }

    pub(super) struct Driver {
        input: Option<Input>,
        output: Option<Output>,
        stats: Arc<Stats>,
    }

    struct Input {
        stream: Option<Stream>,
        frames: mpsc::Receiver<Vec<i16>>,
        error: watch::Receiver<Option<String>>,
        target_rate: Arc<AtomicU32>,
        packet_samples: Arc<AtomicU32>,
        config: Chosen,
    }

    struct Output {
        stream: Option<Stream>,
        frames: mpsc::Sender<Vec<i16>>,
        error: watch::Receiver<Option<String>>,
        source_rate: Arc<AtomicU32>,
        config: Chosen,
        stats: Arc<Stats>,
    }

    #[derive(Debug, Clone)]
    struct Chosen {
        id: String,
        rate: u32,
        channels: u16,
        format: SampleFormat,
    }

    #[derive(Debug, Default)]
    struct Stats {
        input_dropped: AtomicU64,
        output_dropped: AtomicU64,
        output_silence: AtomicU64,
    }

    impl Driver {
        pub(super) fn open(input: Option<&str>, output: Option<&str>) -> Result<Self, String> {
            let stats = Arc::new(Stats::default());
            let input = input.map(|id| open_input(id, &stats)).transpose()?;
            let output = output
                .map(|id| open_output(id, Arc::clone(&stats)))
                .transpose()?;
            Ok(Self {
                input,
                output,
                stats,
            })
        }

        pub(super) fn has_output(&self) -> bool {
            self.output.is_some()
        }

        pub(super) async fn run(
            &mut self,
            media: &sipx_media::MediaSession,
            duration: Duration,
        ) -> Result<u64, String> {
            let rate = media.codec().clock_rate();
            let packet_samples = u32::try_from(media.samples_per_packet())
                .map_err(|_| "media packet is too large for a device frame".to_owned())?;
            let starting = (|| {
                if let Some(input) = &self.input {
                    input.target_rate.store(rate, Ordering::Release);
                    input
                        .packet_samples
                        .store(packet_samples, Ordering::Release);
                    input
                        .stream
                        .as_ref()
                        .ok_or_else(|| "audio input stream was already stopped".to_owned())?
                        .play()
                        .map_err(|error| device_error("audio input", &input.config.id, &error))?;
                }
                if let Some(output) = &self.output {
                    output.source_rate.store(rate, Ordering::Release);
                    output
                        .stream
                        .as_ref()
                        .ok_or_else(|| "audio output stream was already stopped".to_owned())?
                        .play()
                        .map_err(|error| device_error("audio output", &output.config.id, &error))?;
                }
                Ok::<(), String>(())
            })();
            if let Err(error) = starting {
                self.shutdown();
                return Err(error);
            }

            let deadline = tokio::time::Instant::now() + duration;
            let (stop_tx, stop_rx) = watch::channel(false);
            let input = relay_input(
                self.input.as_mut(),
                media,
                deadline,
                stop_rx.clone(),
                stop_tx.clone(),
            );
            let output = relay_output(
                self.output.as_mut(),
                media,
                deadline,
                stop_rx,
                stop_tx.clone(),
            );
            let (input_result, output_result) = tokio::join!(input, output);
            let _ = stop_tx.send(true);

            self.shutdown();
            input_result?;
            output_result
        }

        /// Pausing stops new callbacks; dropping the stream waits for the backend worker it owns.
        /// Taking each option makes this idempotent on every error path.
        fn shutdown(&mut self) {
            if let Some(stream) = self.input.as_mut().and_then(|input| input.stream.take()) {
                let _ = stream.pause();
                drop(stream);
            }
            if let Some(stream) = self.output.as_mut().and_then(|output| output.stream.take()) {
                let _ = stream.pause();
                drop(stream);
            }
        }

        pub(super) fn report(&self, mut report: Report) -> Report {
            if let Some(input) = &self.input {
                report = report
                    .text("audio_input_device", input.config.id.clone())
                    .text("audio_input_config", config_name(&input.config));
            }
            if let Some(output) = &self.output {
                report = report
                    .text("audio_output_device", output.config.id.clone())
                    .text("audio_output_config", config_name(&output.config));
            }
            if self.input.is_some() || self.output.is_some() {
                report = report
                    .number(
                        "device_input_dropped_samples",
                        counter(&self.stats.input_dropped),
                    )
                    .number(
                        "device_output_dropped_samples",
                        counter(&self.stats.output_dropped),
                    )
                    .number(
                        "device_output_silence_samples",
                        counter(&self.stats.output_silence),
                    );
            }
            report
        }
    }

    impl Drop for Driver {
        fn drop(&mut self) {
            self.shutdown();
        }
    }

    fn counter(value: &AtomicU64) -> i64 {
        i64::try_from(value.load(Ordering::Relaxed)).unwrap_or(i64::MAX)
    }

    fn config_name(config: &Chosen) -> String {
        format!(
            "{}Hz,{}ch,{}",
            config.rate,
            config.channels,
            format_name(config.format)
        )
    }

    fn format_name(format: SampleFormat) -> &'static str {
        match format {
            SampleFormat::I16 => "i16",
            SampleFormat::F32 => "f32",
            SampleFormat::U16 => "u16",
            _ => "unsupported",
        }
    }

    async fn relay_input(
        input: Option<&mut Input>,
        media: &sipx_media::MediaSession,
        deadline: tokio::time::Instant,
        mut stop: watch::Receiver<bool>,
        stop_all: watch::Sender<bool>,
    ) -> Result<(), String> {
        let Some(input) = input else {
            return Ok(());
        };
        loop {
            tokio::select! {
                () = tokio::time::sleep_until(deadline) => return Ok(()),
                changed = stop.changed() => {
                    if changed.is_err() || *stop.borrow() {
                        return Ok(());
                    }
                }
                changed = input.error.changed() => {
                    if changed.is_err() {
                        return Ok(());
                    }
                    if let Some(error) = input.error.borrow().clone() {
                        let _ = stop_all.send(true);
                        return Err(error);
                    }
                }
                frame = input.frames.recv() => {
                    let Some(frame) = frame else {
                        return Ok(());
                    };
                    let playing = media.play(&frame, media.samples_per_packet());
                    tokio::select! {
                        () = tokio::time::sleep_until(deadline) => return Ok(()),
                        _ = playing => {}
                    }
                }
            }
        }
    }

    async fn relay_output(
        output: Option<&mut Output>,
        media: &sipx_media::MediaSession,
        deadline: tokio::time::Instant,
        mut stop: watch::Receiver<bool>,
        stop_all: watch::Sender<bool>,
    ) -> Result<u64, String> {
        let Some(output) = output else {
            return Ok(0);
        };
        let mut received = 0u64;
        loop {
            tokio::select! {
                () = tokio::time::sleep_until(deadline) => return Ok(received),
                changed = stop.changed() => {
                    if changed.is_err() || *stop.borrow() {
                        return Ok(received);
                    }
                }
                changed = output.error.changed() => {
                    if changed.is_err() {
                        return Ok(received);
                    }
                    if let Some(error) = output.error.borrow().clone() {
                        let _ = stop_all.send(true);
                        return Err(error);
                    }
                }
                frame = media.recv() => {
                    let Some(frame) = frame else {
                        return Ok(received);
                    };
                    received = received.saturating_add(u64::try_from(frame.len()).unwrap_or(u64::MAX));
                    if let Err(mpsc::error::TrySendError::Full(frame)) =
                        output.frames.try_send(frame)
                    {
                        output.stats.output_dropped.fetch_add(
                            u64::try_from(frame.len()).unwrap_or(u64::MAX),
                            Ordering::Relaxed,
                        );
                    }
                }
            }
        }
    }

    fn open_input(id: &str, stats: &Arc<Stats>) -> Result<Input, String> {
        let device = exact_device(id, "audio input")?;
        let chosen = choose(&device, id, Direction::Input)?;
        let config = StreamConfig {
            channels: chosen.channels,
            sample_rate: chosen.rate,
            buffer_size: BufferSize::Default,
        };
        let (frames_tx, frames) = mpsc::channel(MEDIA_QUEUE_FRAMES);
        let (error_tx, error) = watch::channel(None);
        let target_rate = Arc::new(AtomicU32::new(0));
        let packet_samples = Arc::new(AtomicU32::new(0));
        let mut converter = InputConverter::new(chosen.rate, chosen.channels);
        let callback_rate = Arc::clone(&target_rate);
        let callback_packet = Arc::clone(&packet_samples);
        let callback_stats = Arc::clone(stats);
        let callback_id = chosen.id.clone();
        let callback_error = error_tx.clone();
        let stream_id = chosen.id.clone();
        let stream = device
            .build_input_stream_raw(
                config,
                chosen.format,
                move |data, _| {
                    let rate = callback_rate.load(Ordering::Acquire);
                    let packet = callback_packet.load(Ordering::Acquire);
                    if rate == 0 || packet == 0 {
                        return;
                    }
                    let Ok(packet) = usize::try_from(packet) else {
                        return;
                    };
                    match converter.convert(data, rate, packet) {
                        Ok(frames) => {
                            for frame in frames {
                                if let Err(mpsc::error::TrySendError::Full(frame)) =
                                    frames_tx.try_send(frame)
                                {
                                    callback_stats.input_dropped.fetch_add(
                                        u64::try_from(frame.len()).unwrap_or(u64::MAX),
                                        Ordering::Relaxed,
                                    );
                                }
                            }
                        }
                        Err(message) => {
                            callback_error.send_replace(Some(format!(
                                "audio input {callback_id}: unsupported device callback: {message}"
                            )));
                        }
                    }
                },
                move |error| {
                    error_tx.send_replace(Some(device_error("audio input", &stream_id, &error)));
                },
                Some(OPEN_TIMEOUT),
            )
            .map_err(|error| device_error("audio input", id, &error))?;
        Ok(Input {
            stream: Some(stream),
            frames,
            error,
            target_rate,
            packet_samples,
            config: chosen,
        })
    }

    fn open_output(id: &str, stats: Arc<Stats>) -> Result<Output, String> {
        let device = exact_device(id, "audio output")?;
        let chosen = choose(&device, id, Direction::Output)?;
        let config = StreamConfig {
            channels: chosen.channels,
            sample_rate: chosen.rate,
            buffer_size: BufferSize::Default,
        };
        let (frames, frames_rx) = mpsc::channel(MEDIA_QUEUE_FRAMES);
        let (error_tx, error) = watch::channel(None);
        let source_rate = Arc::new(AtomicU32::new(0));
        let callback_rate = Arc::clone(&source_rate);
        let callback_stats = Arc::clone(&stats);
        let callback_id = chosen.id.clone();
        let callback_error = error_tx.clone();
        let stream_id = chosen.id.clone();
        let mut converter = OutputConverter::new(chosen.rate, chosen.channels, frames_rx);
        let stream = device
            .build_output_stream_raw(
                config,
                chosen.format,
                move |data, _| {
                    let rate = callback_rate.load(Ordering::Acquire);
                    if let Err(message) = converter.fill(data, rate, &callback_stats) {
                        callback_error.send_replace(Some(format!(
                            "audio output {callback_id}: unsupported device callback: {message}"
                        )));
                    }
                },
                move |error| {
                    error_tx.send_replace(Some(device_error("audio output", &stream_id, &error)));
                },
                Some(OPEN_TIMEOUT),
            )
            .map_err(|error| device_error("audio output", id, &error))?;
        Ok(Output {
            stream: Some(stream),
            frames,
            error,
            source_rate,
            config: chosen,
            stats,
        })
    }

    fn exact_device(id: &str, direction: &str) -> Result<Device, String> {
        let parsed = id
            .parse::<cpal::DeviceId>()
            .map_err(|error| format!("{direction} {id}: invalid identifier: {error}"))?;
        let host = cpal::host_from_id(parsed.host())
            .map_err(|error| format!("{direction} {id}: host not available: {error}"))?;
        host.device_by_id(&parsed)
            .ok_or_else(|| format!("{direction} {id}: device not available"))
    }

    #[derive(Debug, Clone, Copy)]
    enum Direction {
        Input,
        Output,
    }

    fn choose(device: &Device, id: &str, direction: Direction) -> Result<Chosen, String> {
        let ranges: Vec<_> = match direction {
            Direction::Input => device
                .supported_input_configs()
                .map_err(|error| device_error("audio input", id, &error))?
                .collect(),
            Direction::Output => device
                .supported_output_configs()
                .map_err(|error| device_error("audio output", id, &error))?
                .collect(),
        };
        let selected = ranges
            .into_iter()
            .filter(|range| {
                range.channels() > 0
                    && range.channels() <= MAX_CHANNELS
                    && range.min_sample_rate() > 0
                    && matches!(
                        range.sample_format(),
                        SampleFormat::I16 | SampleFormat::F32 | SampleFormat::U16
                    )
            })
            .map(|range| {
                let rate = 8_000u32.clamp(range.min_sample_rate(), range.max_sample_rate());
                let distance = rate.abs_diff(8_000);
                let format = range.sample_format();
                let format_rank = match format {
                    SampleFormat::I16 => 0,
                    SampleFormat::F32 => 1,
                    SampleFormat::U16 => 2,
                    _ => 3,
                };
                (
                    (distance, range.channels(), format_rank, rate),
                    Chosen {
                        id: id.to_owned(),
                        rate,
                        channels: range.channels(),
                        format,
                    },
                )
            })
            .filter(|(_, chosen)| chosen.rate <= MAX_DEVICE_RATE)
            .min_by_key(|(rank, _)| *rank)
            .map(|(_, chosen)| chosen);
        selected.ok_or_else(|| {
            format!(
                "{} {id}: unsupported device configuration; need 1..={MAX_CHANNELS} channels of i16, f32 or u16 PCM at no more than {MAX_DEVICE_RATE} Hz",
                match direction {
                    Direction::Input => "audio input",
                    Direction::Output => "audio output",
                }
            )
        })
    }

    fn device_error(direction: &str, id: &str, error: &cpal::Error) -> String {
        let category = match error.kind() {
            cpal::ErrorKind::DeviceBusy => "busy",
            cpal::ErrorKind::DeviceNotAvailable => "not available",
            cpal::ErrorKind::HostUnavailable => "host not available",
            cpal::ErrorKind::PermissionDenied => "permission denied",
            cpal::ErrorKind::UnsupportedConfig | cpal::ErrorKind::UnsupportedOperation => {
                "unsupported"
            }
            cpal::ErrorKind::ResourceExhausted => "resource exhausted",
            cpal::ErrorKind::StreamInvalidated => "stream invalidated",
            cpal::ErrorKind::Xrun => "xrun",
            _ => "backend failure",
        };
        format!("{direction} {id}: {category}: {error}")
    }

    struct LinearResampler {
        source_rate: u32,
        target_rate: u32,
        previous: Option<f32>,
        source_index: u64,
        next_numerator: u64,
    }

    impl LinearResampler {
        fn new() -> Self {
            Self {
                source_rate: 0,
                target_rate: 0,
                previous: None,
                source_index: 0,
                next_numerator: 0,
            }
        }

        fn push(&mut self, samples: &[f32], source_rate: u32, target_rate: u32) -> Vec<i16> {
            if source_rate == 0 || target_rate == 0 {
                return Vec::new();
            }
            if self.source_rate != source_rate || self.target_rate != target_rate {
                self.source_rate = source_rate;
                self.target_rate = target_rate;
                self.previous = None;
                self.source_index = 0;
                self.next_numerator = 0;
            }
            let estimate = samples
                .len()
                .saturating_mul(usize::try_from(target_rate).unwrap_or(usize::MAX))
                / usize::try_from(source_rate).unwrap_or(1)
                + 1;
            let mut output = Vec::with_capacity(estimate);
            for &current in samples {
                let Some(previous) = self.previous else {
                    output.push(to_i16(f64::from(current)));
                    self.previous = Some(current);
                    self.next_numerator = u64::from(source_rate);
                    continue;
                };
                self.source_index = self.source_index.saturating_add(1);
                let interval_start = self
                    .source_index
                    .saturating_sub(1)
                    .saturating_mul(u64::from(target_rate));
                let interval_end = self.source_index.saturating_mul(u64::from(target_rate));
                while self.next_numerator <= interval_end {
                    let fraction_numerator =
                        u32::try_from(self.next_numerator.saturating_sub(interval_start))
                            .unwrap_or(u32::MAX);
                    let fraction = f64::from(fraction_numerator) / f64::from(target_rate);
                    let value =
                        f64::from(previous) + (f64::from(current) - f64::from(previous)) * fraction;
                    output.push(to_i16(value));
                    self.next_numerator =
                        self.next_numerator.saturating_add(u64::from(source_rate));
                }
                self.previous = Some(current);
            }
            output
        }
    }

    struct InputConverter {
        source_rate: u32,
        channels: u16,
        resampler: LinearResampler,
        pending: VecDeque<i16>,
    }

    impl InputConverter {
        fn new(source_rate: u32, channels: u16) -> Self {
            Self {
                source_rate,
                channels,
                resampler: LinearResampler::new(),
                pending: VecDeque::new(),
            }
        }

        fn convert(
            &mut self,
            data: &Data,
            target_rate: u32,
            packet_samples: usize,
        ) -> Result<Vec<Vec<i16>>, String> {
            let mono = mono(data, self.channels)?;
            self.pending
                .extend(self.resampler.push(&mono, self.source_rate, target_rate));
            let mut frames = Vec::new();
            while self.pending.len() >= packet_samples {
                frames.push(self.pending.drain(..packet_samples).collect());
            }
            Ok(frames)
        }
    }

    struct OutputConverter {
        device_rate: u32,
        channels: u16,
        frames: mpsc::Receiver<Vec<i16>>,
        resampler: LinearResampler,
        pending: VecDeque<i16>,
    }

    impl OutputConverter {
        fn new(device_rate: u32, channels: u16, frames: mpsc::Receiver<Vec<i16>>) -> Self {
            Self {
                device_rate,
                channels,
                frames,
                resampler: LinearResampler::new(),
                pending: VecDeque::new(),
            }
        }

        fn fill(&mut self, data: &mut Data, source_rate: u32, stats: &Stats) -> Result<(), String> {
            let channels = usize::from(self.channels);
            if data.len() > MAX_CALLBACK_SAMPLES {
                return Err(format!(
                    "callback has {} samples, above the {MAX_CALLBACK_SAMPLES}-sample bound",
                    data.len()
                ));
            }
            if channels == 0 || !data.len().is_multiple_of(channels) {
                return Err("callback sample count is not whole device frames".to_owned());
            }
            let needed = data.len() / channels;
            while self.pending.len() < needed {
                let Ok(frame) = self.frames.try_recv() else {
                    break;
                };
                let float = frame
                    .into_iter()
                    .map(|sample| f32::from(sample) / 32768.0)
                    .collect::<Vec<_>>();
                self.pending
                    .extend(self.resampler.push(&float, source_rate, self.device_rate));
            }
            let mut mono = Vec::with_capacity(needed);
            for _ in 0..needed {
                if let Some(sample) = self.pending.pop_front() {
                    mono.push(sample);
                } else {
                    mono.push(0);
                    stats.output_silence.fetch_add(1, Ordering::Relaxed);
                }
            }
            write(data, channels, &mono)
        }
    }

    fn mono(data: &Data, channels: u16) -> Result<Vec<f32>, String> {
        let channels = usize::from(channels);
        if data.len() > MAX_CALLBACK_SAMPLES {
            return Err(format!(
                "callback has {} samples, above the {MAX_CALLBACK_SAMPLES}-sample bound",
                data.len()
            ));
        }
        if channels == 0 || !data.len().is_multiple_of(channels) {
            return Err("callback sample count is not whole device frames".to_owned());
        }
        match data.sample_format() {
            SampleFormat::I16 => Ok(downmix(
                data.as_slice::<i16>()
                    .ok_or_else(|| "i16 callback has another representation".to_owned())?,
                channels,
                |sample| f32::from(sample) / 32768.0,
            )),
            SampleFormat::F32 => Ok(downmix(
                data.as_slice::<f32>()
                    .ok_or_else(|| "f32 callback has another representation".to_owned())?,
                channels,
                |sample| sample.clamp(-1.0, 1.0),
            )),
            SampleFormat::U16 => Ok(downmix(
                data.as_slice::<u16>()
                    .ok_or_else(|| "u16 callback has another representation".to_owned())?,
                channels,
                |sample| (f32::from(sample) - 32768.0) / 32768.0,
            )),
            other => Err(format!("sample format {other:?}")),
        }
    }

    fn downmix<T: Copy>(samples: &[T], channels: usize, convert: impl Fn(T) -> f32) -> Vec<f32> {
        let divisor = f32::from(u16::try_from(channels).unwrap_or(u16::MAX));
        samples
            .chunks_exact(channels)
            .map(|frame| frame.iter().copied().map(&convert).sum::<f32>() / divisor)
            .collect()
    }

    fn write(data: &mut Data, channels: usize, mono: &[i16]) -> Result<(), String> {
        match data.sample_format() {
            SampleFormat::I16 => {
                let samples = data
                    .as_slice_mut::<i16>()
                    .ok_or_else(|| "i16 callback has another representation".to_owned())?;
                copy_channels(samples, channels, mono, |sample| sample);
            }
            SampleFormat::F32 => {
                let samples = data
                    .as_slice_mut::<f32>()
                    .ok_or_else(|| "f32 callback has another representation".to_owned())?;
                copy_channels(samples, channels, mono, |sample| {
                    f32::from(sample) / 32768.0
                });
            }
            SampleFormat::U16 => {
                let samples = data
                    .as_slice_mut::<u16>()
                    .ok_or_else(|| "u16 callback has another representation".to_owned())?;
                copy_channels(samples, channels, mono, |sample| {
                    u16::try_from(i32::from(sample) + 32768).unwrap_or(u16::MAX)
                });
            }
            other => return Err(format!("sample format {other:?}")),
        }
        Ok(())
    }

    fn copy_channels<T: Copy>(
        output: &mut [T],
        channels: usize,
        mono: &[i16],
        convert: impl Fn(i16) -> T,
    ) {
        for (frame, sample) in output.chunks_exact_mut(channels).zip(mono.iter().copied()) {
            frame.fill(convert(sample));
        }
    }

    #[allow(
        clippy::cast_possible_truncation,
        reason = "the value is rounded and clamped to the i16 domain immediately before the cast"
    )]
    fn to_i16(sample: f64) -> i16 {
        let scaled = sample.clamp(-1.0, 1.0) * 32768.0;
        let rounded = scaled
            .round()
            .clamp(f64::from(i16::MIN), f64::from(i16::MAX));
        i16::try_from(rounded as i32).unwrap_or(if rounded.is_sign_negative() {
            i16::MIN
        } else {
            i16::MAX
        })
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
        #[allow(
            clippy::cast_possible_truncation,
            reason = "the fixture values are in the exactly representable telephony sample range"
        )]
        fn linear_conversion_preserves_eight_kilohertz_points_through_48_kilohertz() {
            let original = [-20_000i16, -8_000, 0, 8_000, 20_000];
            let source = original
                .windows(2)
                .flat_map(|pair| {
                    (0..6).map(move |part| {
                        let fraction = f64::from(part) / 6.0;
                        let value = f64::from(pair[0])
                            + (f64::from(pair[1]) - f64::from(pair[0])) * fraction;
                        (value / 32768.0) as f32
                    })
                })
                .chain(std::iter::once(
                    f32::from(*original.last().unwrap()) / 32768.0,
                ))
                .collect::<Vec<_>>();
            let mut converter = LinearResampler::new();
            assert_eq!(converter.push(&source, 48_000, 8_000), original);
        }

        #[test]
        fn endpoint_aliases_are_exact_and_conflicts_are_refused() {
            let raw = ["dial", "sip:a@b", "--audio-input", "wav:a.wav"].map(str::to_owned);
            let selected =
                super::super::Selection::from_args(&crate::Args::new(&raw).unwrap()).unwrap();
            assert_eq!(selected.wav_input(), Some("a.wav"));

            let raw = [
                "dial",
                "sip:a@b",
                "--audio-input",
                "wav:a.wav",
                "--play",
                "b.wav",
            ]
            .map(str::to_owned);
            assert!(super::super::Selection::from_args(&crate::Args::new(&raw).unwrap()).is_err());
        }
    }
}
