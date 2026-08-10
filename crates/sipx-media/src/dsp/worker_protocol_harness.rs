//! The one opaque door the worker-protocol test harness needs into crate-private callers.
//!
//! The program vocabulary, generator, invariant oracle and corpus live in `sipx-testkit`. This
//! module owns only the observations that cannot be made from outside this crate: the real wire
//! decoder, `worker::serve`, and `supervised::pump`. The public function deliberately accepts and
//! returns byte/string records instead of exposing any of those private types. It is test
//! instrumentation, not application DSP surface (`M-129`).

use std::io::{self, Read, Write};
use std::sync::mpsc::sync_channel;

use sipx_audio::dsp::{
    DspCapability, DspFrame, ExecutionPolicy, ExecutionProfile, MAX_FRAME_SAMPLES, StreamFormat,
};

use super::supervised::{self, Request};
use super::wire::{self, WorkerProtocolError};
use super::worker::{self, SupervisedWorker, WorkerResult};
use crate::processing::AudioDirection;

/// Exercise the private worker-protocol decoder or one of its two real callers.
///
/// The request and observations are intentionally opaque records. Their vocabulary belongs to
/// `sipx-testkit::worker_protocol_sequence`, which is the only workspace caller and documents the
/// encoding beside its oracle. Keeping this as one primitive-shaped entry point prevents a fuzzing
/// harness from becoming the published media crate's application API.
#[doc(hidden)]
#[must_use]
pub fn exercise_worker_protocol_sequence(request: &[u8]) -> Vec<String> {
    let Some((&mode, rest)) = request.split_first() else {
        return vec!["error|empty request".to_owned()];
    };
    match mode {
        0 => decode(rest),
        1 => worker_lane(rest),
        2 => runtime_lane(rest),
        _ => vec![format!("error|unknown mode {mode}")],
    }
}

/// Mode 0: side byte, configured ceiling, chunk size, then the peer's octets.
fn decode(request: &[u8]) -> Vec<String> {
    let Some((&side, rest)) = request.split_first() else {
        return vec!["error|decode side missing".to_owned()];
    };
    let Some((mut ceiling, rest)) = take_u32(rest) else {
        return vec!["error|decode ceiling missing".to_owned()];
    };
    let Some((chunk, octets)) = take_u32(rest) else {
        return vec!["error|decode chunk missing".to_owned()];
    };
    let mut reader = Scripted::new(octets, chunk as usize);
    let mut payload = Vec::new();
    let mut samples = Vec::new();
    let mut observations = Vec::new();

    loop {
        let at = reader.at;
        let verdict = wire::read_message(&mut reader, &mut payload, &mut samples, ceiling);
        let read = reader.at.saturating_sub(at);
        let capacity = payload.capacity();
        match verdict {
            Ok(None) => {
                observations.push(format!(
                    "decode|end|{at}|{read}|{capacity}|{}",
                    wire::MESSAGE_LIMIT
                ));
                return observations;
            }
            Ok(Some(message)) => {
                let (kind, max_samples) = match message {
                    wire::Incoming::Hello { max_samples, .. } => (0x01_u8, max_samples),
                    wire::Incoming::Frame { .. } => (0x02, u32::MAX),
                    wire::Incoming::Result { .. } => (0x81, u32::MAX),
                };
                observations.push(format!(
                    "decode|accepted|{at}|{read}|{capacity}|{kind}|{}|{max_samples}",
                    samples.len()
                ));
                if side & 1 == 1 && max_samples != u32::MAX {
                    ceiling = max_samples;
                }
            }
            Err(wire::Fault::Io(error)) => {
                observations.push(format!(
                    "decode|io|{at}|{read}|{capacity}|{:?}",
                    error.kind()
                ));
                return observations;
            }
            Err(wire::Fault::Refused(error)) => {
                observations.push(format!(
                    "decode|refused|{at}|{read}|{capacity}|{}",
                    refusal_name(&error)
                ));
                return observations;
            }
        }
    }
}

/// Mode 1: chunk size, then the worker's input octets.
fn worker_lane(request: &[u8]) -> Vec<String> {
    let Some((chunk, octets)) = take_u32(request) else {
        return vec!["error|worker chunk missing".to_owned()];
    };
    let mut reader = Scripted::new(octets, chunk as usize);
    let served = worker::serve(Echo, &mut reader, Recorder::default());
    let ending = served.as_ref().map_or_else(
        |error| format!("{:?} {error}", error.kind()),
        |()| "cleanly".to_owned(),
    );
    vec![format!("worker|{}|{}|{ending}", reader.at, octets.len())]
}

/// Mode 2: configured ceiling, chunk size, then the worker's result octets.
fn runtime_lane(request: &[u8]) -> Vec<String> {
    const OFFERED: usize = 8;

    let Some((ceiling, rest)) = take_u32(request) else {
        return vec!["error|runtime ceiling missing".to_owned()];
    };
    let Some((chunk, octets)) = take_u32(rest) else {
        return vec!["error|runtime chunk missing".to_owned()];
    };
    let (requests, inbox) = sync_channel::<Request>(OFFERED);
    let (outbox, answers) = sync_channel(OFFERED);
    for sequence in 0..OFFERED {
        let sequence = sequence as u64;
        if requests
            .try_send(Request {
                sequence,
                position: sequence.saturating_mul(160),
                discontinuity: None,
                samples: vec![0_i16; 160],
            })
            .is_err()
        {
            break;
        }
    }
    drop(requests);

    let reader = Scripted::new(octets, chunk as usize);
    let mut written = Recorder::default();
    supervised::pump(
        &mut written,
        reader,
        &inbox,
        &outbox,
        AudioDirection::Inbound,
        StreamFormat::default(),
        ceiling.min(MAX_FRAME_SAMPLES) as usize,
    );
    drop(outbox);

    let results: Vec<_> = std::iter::from_fn(|| answers.try_recv().ok())
        .map(|response: supervised::Response| result_name(response.result))
        .collect();
    let offers = count_frames(&written.written);
    let unoffered = std::iter::from_fn(|| inbox.try_recv().ok()).count();
    vec![format!(
        "runtime|{offers}|{unoffered}|{}",
        results.join(",")
    )]
}

fn take_u32(input: &[u8]) -> Option<(u32, &[u8])> {
    let (bytes, rest) = input.split_at_checked(4)?;
    let bytes = bytes.first_chunk::<4>()?;
    Some((u32::from_be_bytes(*bytes), rest))
}

fn refusal_name(error: &WorkerProtocolError) -> &'static str {
    match error {
        WorkerProtocolError::BadMagic => "BadMagic",
        WorkerProtocolError::Reserved { .. } => "Reserved",
        WorkerProtocolError::UnknownType { .. } => "UnknownType",
        WorkerProtocolError::Undersized { .. } => "Undersized",
        WorkerProtocolError::Oversized { .. } => "Oversized",
        WorkerProtocolError::LengthMismatch { .. } => "LengthMismatch",
        WorkerProtocolError::Truncated => "Truncated",
        WorkerProtocolError::Value { .. } => "Value",
        WorkerProtocolError::Version { .. } => "Version",
    }
}

fn result_name(result: WorkerResult) -> &'static str {
    match result {
        WorkerResult::Produced => "Produced",
        WorkerResult::Withheld => "Withheld",
        WorkerResult::Failed => "Failed",
    }
}

struct Scripted<'a> {
    octets: &'a [u8],
    at: usize,
    chunk: usize,
}

impl<'a> Scripted<'a> {
    fn new(octets: &'a [u8], chunk: usize) -> Self {
        Self {
            octets,
            at: 0,
            chunk: if chunk == 0 { usize::MAX } else { chunk },
        }
    }
}

impl Read for Scripted<'_> {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        let rest = self.octets.get(self.at..).unwrap_or_default();
        let take = rest.len().min(buffer.len()).min(self.chunk);
        let Some(source) = rest.get(..take) else {
            return Ok(0);
        };
        let Some(slot) = buffer.get_mut(..take) else {
            return Ok(0);
        };
        slot.copy_from_slice(source);
        self.at = self.at.saturating_add(take);
        Ok(take)
    }
}

#[derive(Default)]
struct Recorder {
    written: Vec<u8>,
}

impl Write for Recorder {
    fn write(&mut self, buffer: &[u8]) -> io::Result<usize> {
        self.written.extend_from_slice(buffer);
        Ok(buffer.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

struct Echo;

impl SupervisedWorker for Echo {
    fn capability(&self) -> DspCapability {
        DspCapability::new("worker-protocol-harness")
            .with_execution(ExecutionPolicy::new(ExecutionProfile::SupervisedIsolated))
    }

    fn run(&mut self, frame: &DspFrame<'_>, out: &mut Vec<i16>) -> WorkerResult {
        out.extend_from_slice(frame.samples());
        WorkerResult::Produced
    }
}

fn count_frames(written: &[u8]) -> usize {
    const HEADER: usize = 8;
    const TYPE_FRAME: u8 = 0x02;

    let mut at = 0_usize;
    let mut frames = 0_usize;
    while let Some(header) = written
        .get(at..at.saturating_add(HEADER))
        .and_then(<[u8]>::first_chunk::<HEADER>)
    {
        let Some(declared) = header.get(4..8).and_then(<[u8]>::first_chunk::<4>) else {
            break;
        };
        if header.get(2) == Some(&TYPE_FRAME) {
            frames = frames.saturating_add(1);
        }
        at = at
            .saturating_add(HEADER)
            .saturating_add(u32::from_be_bytes(*declared) as usize);
    }
    frames
}
