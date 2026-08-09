//! `sipx-dsp-worker` — the reference supervised DSP worker
//! (`docs/specs/call-dsp-graph.md` §7.5).
//!
//! A conforming worker program, so that §7.4's protocol has a runnable peer rather than only a
//! written one. It applies one declared transform, and its remaining modes exist to produce on
//! demand each of the endings §7.2 tabulates: withhold every frame, hang inside one, answer with
//! the wrong number of samples, exit mid-call, and crash. An application can point a supervised
//! stage at it to check its own supervision before writing a worker of its own, which is
//! [`vision.md`](../vision.md) principle 6 applied to a protocol whose failure modes are the point.
//!
//! ```text
//! sipx-dsp-worker [--mode MODE] [--gain N] [--after N]
//!
//!   --mode gain      apply --gain to every sample (the default)
//!   --mode pid       answer with this process's own pid in the first two samples
//!   --mode withhold  answer every frame withheld
//!   --mode hang      never answer, and never return from the frame
//!   --mode short     answer with one sample fewer than the frame carried
//!   --mode fail      answer terminally failed
//!   --mode exit      leave without answering
//!   --mode crash     abort without answering
//!   --gain N         the integer gain of `--mode gain`, saturating (default 2)
//!   --after N        the frame index the mode takes effect at; earlier frames are gained
//! ```

use std::ffi::OsString;
use std::process::ExitCode;

use sipx_media::dsp::{
    DspCapability, DspFrame, ExecutionPolicy, ExecutionProfile, SupervisedWorker, WorkerResult,
    serve_worker,
};

/// What this worker does once it has reached `--after`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    Gain,
    Pid,
    Withhold,
    Hang,
    Short,
    Fail,
    Exit,
    Crash,
}

impl Mode {
    fn parse(value: &str) -> Option<Self> {
        match value {
            "gain" => Some(Self::Gain),
            "pid" => Some(Self::Pid),
            "withhold" => Some(Self::Withhold),
            "hang" => Some(Self::Hang),
            "short" => Some(Self::Short),
            "fail" => Some(Self::Fail),
            "exit" => Some(Self::Exit),
            "crash" => Some(Self::Crash),
            _ => None,
        }
    }
}

/// The reference worker itself.
struct Reference {
    mode: Mode,
    gain: i16,
    after: u64,
    seen: u64,
}

impl SupervisedWorker for Reference {
    fn capability(&self) -> DspCapability {
        DspCapability::new("sipx-dsp-worker")
            .with_execution(ExecutionPolicy::new(ExecutionProfile::SupervisedIsolated))
    }

    fn run(&mut self, frame: &DspFrame<'_>, out: &mut Vec<i16>) -> WorkerResult {
        let index = self.seen;
        self.seen = self.seen.saturating_add(1);
        if index < self.after {
            return self.gain(frame, out);
        }
        match self.mode {
            Mode::Gain => self.gain(frame, out),
            Mode::Pid => {
                // The pid, split across the first two positions, low half first. A worker that
                // says which process it is in the audio itself is the difference between proving
                // that a process was spawned and proving that the DSP ran in it.
                let pid = std::process::id();
                let low = u16::try_from(pid & 0xffff).unwrap_or(0);
                let high = u16::try_from(pid >> 16).unwrap_or(0);
                out.push(i16::from_ne_bytes(low.to_ne_bytes()));
                out.push(i16::from_ne_bytes(high.to_ne_bytes()));
                out.resize(frame.samples().len(), 0);
                WorkerResult::Produced
            }
            Mode::Withhold => WorkerResult::Withheld,
            Mode::Hang => {
                // §7.3: a worker stuck inside one frame is what the runtime's kill exists for, and
                // what its reader thread's end-of-file exit exists for. Parked rather than spinning
                // — a busy loop would be measuring the machine rather than the supervision.
                loop {
                    std::thread::park();
                }
            }
            Mode::Short => {
                let short = frame.samples().len().saturating_sub(1);
                out.extend(frame.samples().iter().take(short));
                WorkerResult::Produced
            }
            Mode::Fail => WorkerResult::Failed,
            Mode::Exit => std::process::exit(0),
            // A crash the runtime must read as an ending like any other, and the one ending a
            // thread-hosted worker could never have produced without taking the media process with
            // it.
            Mode::Crash => std::process::abort(),
        }
    }
}

impl Reference {
    fn gain(&self, frame: &DspFrame<'_>, out: &mut Vec<i16>) -> WorkerResult {
        out.extend(
            frame
                .samples()
                .iter()
                .map(|sample| sample.saturating_mul(self.gain)),
        );
        WorkerResult::Produced
    }
}

fn main() -> ExitCode {
    let mut mode = Mode::Gain;
    let mut gain: i16 = 2;
    let mut after: u64 = 0;

    let mut arguments = std::env::args_os().skip(1);
    while let Some(argument) = arguments.next() {
        let name = argument.to_string_lossy().into_owned();
        let mut value = || -> Option<OsString> { arguments.next() };
        let parsed = match name.as_str() {
            "--mode" => value()
                .and_then(|raw| Mode::parse(&raw.to_string_lossy()))
                .map(|chosen| mode = chosen)
                .is_some(),
            "--gain" => value()
                .and_then(|raw| raw.to_string_lossy().parse::<i16>().ok())
                .map(|chosen| gain = chosen)
                .is_some(),
            "--after" => value()
                .and_then(|raw| raw.to_string_lossy().parse::<u64>().ok())
                .map(|chosen| after = chosen)
                .is_some(),
            _ => false,
        };
        if !parsed {
            eprintln!("sipx-dsp-worker: `{name}` is not an option this worker takes");
            return ExitCode::from(2);
        }
    }

    let worker = Reference {
        mode,
        gain,
        after,
        seen: 0,
    };
    // Returning from `main` is what ends this process, hung processing thread and all — §7.4's
    // third obligation, and the reason nothing follows this call.
    match serve_worker(worker) {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("sipx-dsp-worker: {error}");
            ExitCode::from(2)
        }
    }
}
