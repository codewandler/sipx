//! What the built-in call-DSP processors cost in CPU time: `X-109`.
//!
//! The companion to [`sipx_audio::dsp::response`], and its opposite in every way that matters. A
//! response is integer arithmetic over a shipped table and is the same figure on every machine at
//! any load. **A cost is a clock reading, and a clock reading on a busy box is not a measurement.**
//! Everything unusual about this example follows from that one sentence.
//!
//! # What it reports
//!
//! For each processor, over a stated signal: **nanoseconds per thousand positions**, and the
//! fraction of one core that keeping up with a live narrowband stream would take, in parts per
//! million. The second is the number that answers "can this run on the media worker" — a processor
//! at 10,000 ppm needs one per cent of a core per call, and
//! `docs/specs/call-dsp-graph.md`'s deadline machinery is what happens when one does not.
//!
//! Every figure is the **minimum** over the repetitions, not the mean. A scheduler can only ever
//! make a run slower, so on a shared machine the minimum is the least contaminated estimator
//! available and the mean is mostly a measurement of the neighbours.
//!
//! # What it refuses to report
//!
//! - **A figure taken under load.** It reads `/proc/loadavg` before and after the run and stops if
//!   the one-minute average is above a quarter of the machine's cores, exiting `2` — the code
//!   `./scripts/check-dsp-heap.sh` and `gate.py` use for *nothing was measured*, as against `1` for
//!   *something is wrong with the tree*. `--under-load` runs anyway and stamps every line and the
//!   JSON with `UNDER LOAD`, because a rough before-and-after during development is worth having
//!   and worth being unable to mistake for a recorded figure.
//! - **A figure from a debug build**, where the integer arithmetic these processors are built from
//!   carries overflow checks that are not in a release call path.
//! - **Any figure at all, if the harness cannot show its own clock working.** Before measuring
//!   anything it times a chain of sixteen `sipx.gain` stages against one, and requires at least
//!   eight times the cost. A harness whose workload has been optimised away, or whose clock has no
//!   resolution at this scale, fails there rather than reporting a very fast processor.
//!
//! It reports no memory: heap is `./scripts/check-dsp-heap.sh`'s, measured exactly against each
//! processor's declared `state_bytes`, and a second mechanism for the same quantity would be a
//! second answer to maintain.
//!
//! # Running it
//!
//! ```sh
//! cargo build --release -p sipx-audio --example dsp_cost
//! ./target/release/examples/dsp_cost                 # a table
//! ./target/release/examples/dsp_cost --json          # the same, recordable
//! ```

use std::fmt::Write as _;
use std::process::ExitCode;
use std::time::Instant;

use sipx_audio::analysis::AudioDirection;
use sipx_audio::dsp::effects::{
    BitCrush, Gain, HardClip, HighPass, LowPass, Peaking, Polarity, SoftClip, Stutter,
};
use sipx_audio::dsp::noise::SubbandSuppressor;
use sipx_audio::dsp::{DspFrame, FrameProcessor, FrameSink, Parameter, Scratch, StreamFormat};

/// The reference stream: narrowband mono, which is what `docs/specs/call-dsp-effects.md`'s vectors
/// and `call-dsp-noise-reduction.md`'s corpus both run on.
const RATE: u32 = 8_000;

/// One frame, in positions: a 20 ms packet at [`RATE`]. Cost is charged per frame as well as per
/// position, so the frame size is part of what a figure is true of.
const FRAME_POSITIONS: usize = 160;

/// Positions in one repetition of every condition but `silence`, which §8 states at 4,096.
///
/// The lengths are §8's exactly rather than padded to one another. A padded corpus would be a
/// different signal from the one `crates/sipx-audio/tests/dsp_noise_reduction.rs` asserts §8's
/// quality figures over, and then the cost and the quality would be figures about two things.
/// Costs stay comparable because every row is reported per thousand positions.
const WORKLOAD_POSITIONS: usize = 12_288;

/// §8's `silence`, which is 4,096 positions and not 12,288.
const SILENCE_POSITIONS: usize = 4_096;

/// Repetitions per row. The whole run is under fifty million positions, which is seconds: `AGENTS.md`
/// requires a bounded, representative workload rather than a long one.
const DEFAULT_REPETITIONS: u32 = 24;

/// The one-minute load average, as a percentage of the machine's cores, above which nothing is
/// measured.
///
/// Ten per cent, and the figure is evidence rather than taste. `X-109` first set it at 25 and ran
/// this harness three times on a twenty-core box at a load of 3.4 to 4.9 — inside that ceiling
/// every time. `sipx.gain` came back at 9,044, 12,275 and 11,247 ns per thousand positions: a 36%
/// spread on a figure whose whole purpose is being compared with next month's. A ceiling that
/// admits that is not a ceiling, so it moved, and [`REPEATABILITY_TOLERANCE_PERCENT`] was added
/// because a load average is a one-minute mean and cannot see a neighbour that starts mid-run.
const DEFAULT_LOAD_CEILING_PERCENT: u64 = 10;

/// How far the control may drift between the start of the run and the end, as a percentage.
///
/// The load-average guard asks whether the box *was* quiet. This asks whether it *stayed* quiet,
/// which is the question that catches the build that started thirty seconds ago and has not
/// reached the one-minute average yet. The same control workload is timed before the first row and
/// after the last; if the two minima disagree by more than this, the run measured two different
/// machines and reports nothing.
const REPEATABILITY_TOLERANCE_PERCENT: u128 = 10;

/// Nanoseconds one position of a live [`RATE`] stream is allowed to take on one core.
const POSITION_BUDGET_NANOSECONDS: u128 = 1_000_000_000 / RATE as u128;

// ------------------------------------------------------------------------ the machine ----

/// What the run can say about the box it ran on. Every field is read, never assumed.
///
/// Load averages are carried in **hundredths**, as integers. `/proc/loadavg` publishes two decimal
/// places and nothing here needs more, and an integer keeps the comparison against the ceiling
/// exact instead of a float comparison nobody would be able to reproduce by hand.
struct Machine {
    cores: u64,
    model: String,
    load_before: Option<u64>,
    load_after: Option<u64>,
}

impl Machine {
    fn read() -> Self {
        Self {
            cores: std::thread::available_parallelism()
                .map_or(0, |count| u64::try_from(count.get()).unwrap_or(0)),
            model: std::fs::read_to_string("/proc/cpuinfo")
                .ok()
                .and_then(|text| {
                    text.lines()
                        .find(|line| line.starts_with("model name"))
                        .and_then(|line| line.split_once(':'))
                        .map(|(_, value)| value.trim().to_owned())
                })
                .unwrap_or_else(|| "unknown".to_owned()),
            load_before: read_load(),
            load_after: None,
        }
    }

    /// The load ceiling in hundredths, or `None` where cores are unknown.
    ///
    /// `percent` of `cores` in hundredths is `cores·percent`, because the two hundreds cancel.
    fn ceiling(&self, percent: u64) -> Option<u64> {
        (self.cores > 0).then(|| self.cores.saturating_mul(percent))
    }
}

impl Machine {
    /// Whether a figure taken on this box right now would describe the code or the neighbours.
    fn is_quiet_enough(&self, percent: u64) -> Result<(), String> {
        match (self.load_before, self.ceiling(percent)) {
            (Some(load), Some(limit)) if load > limit => Err(format!(
                "the one-minute load average is {} against a ceiling of {} ({percent}% of {} \
                 cores) — nothing was measured, because a timing figure taken here would \
                 describe the neighbours",
                show_load(Some(load)),
                show_load(Some(limit)),
                self.cores,
            )),
            (None, _) | (_, None) => Err(
                "this platform publishes no load average, so the run cannot say the box was \
                 quiet — nothing was measured"
                    .to_owned(),
            ),
            _ => Ok(()),
        }
    }
}

/// A load average in hundredths, printed the way `/proc/loadavg` writes it.
fn show_load(load: Option<u64>) -> String {
    load.map_or_else(
        || "unknown".to_owned(),
        |load| format!("{}.{:02}", load / 100, load % 100),
    )
}

/// The one-minute load average, or `None` where this platform does not publish one.
///
/// A missing reading is not a quiet zero. Without it the run cannot say the box was quiet, and a
/// cost figure whose conditions are unknown is the thing `docs/measurements/README.md` says a
/// recorded run must never be.
fn read_load() -> Option<u64> {
    let text = std::fs::read_to_string("/proc/loadavg").ok()?;
    let field = text.split_whitespace().next()?;
    let (whole, fraction) = field.split_once('.').unwrap_or((field, "0"));
    let hundredths: u64 = format!("{fraction:0<2.2}").parse().ok()?;
    Some(whole.parse::<u64>().ok()?.saturating_mul(100) + hundredths)
}

// -------------------------------------------------------------------------- the corpus ----

/// §8's stated recurrence. A sequence written down, never a random source.
fn lcg(state: &mut u32) -> i16 {
    *state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
    i16::from_ne_bytes(((*state >> 16) as u16).to_ne_bytes())
}

fn noise(positions: usize, amplitude: i32) -> Vec<i16> {
    let mut state = 1u32;
    (0..positions)
        .map(|_| {
            let raw = i32::from(lcg(&mut state));
            i16::try_from(raw * amplitude / 32_768).unwrap_or(0)
        })
        .collect()
}

fn voice(positions: usize, amplitude: i32, period: usize) -> Vec<i16> {
    (0..positions)
        .map(|n| {
            if (n / 800) % 2 == 1 {
                return 0;
            }
            let phase = i32::try_from(n % period).unwrap_or(0);
            let span = i32::try_from(period).unwrap_or(1).max(1);
            i16::try_from((phase * 2 - span) * amplitude / span).unwrap_or(0)
        })
        .collect()
}

/// One of `docs/specs/call-dsp-noise-reduction.md` §8's four conditions, at §8's own length.
fn condition(name: &str) -> Vec<i16> {
    match name {
        "silence" => vec![0i16; SILENCE_POSITIONS],
        "stationary" => noise(WORKLOAD_POSITIONS, 2_000),
        "transient" => {
            let mut signal = noise(WORKLOAD_POSITIONS, 2_000);
            for offset in 0..32 {
                if let Some(sample) = signal.get_mut(8_192 + offset) {
                    *sample = if offset % 2 == 0 { i16::MAX } else { -i16::MAX };
                }
            }
            signal
        }
        _ => {
            let near = voice(WORKLOAD_POSITIONS, 8_000, 100);
            let far = voice(WORKLOAD_POSITIONS, 2_000, 137);
            let background = noise(WORKLOAD_POSITIONS, 800);
            (0..WORKLOAD_POSITIONS)
                .map(|n| {
                    let sum = i32::from(near.get(n).copied().unwrap_or(0))
                        + i32::from(far.get(n).copied().unwrap_or(0))
                        + i32::from(background.get(n).copied().unwrap_or(0));
                    i16::try_from(sum.clamp(-32_768, 32_767)).unwrap_or(0)
                })
                .collect()
        }
    }
}

/// FNV-1a over the samples, so a reader can confirm that the signal a figure was taken on is the
/// signal `crates/sipx-audio/tests/dsp_noise_reduction.rs` asserts §8's quality figures over.
///
/// The two generators are separate — a test helper is not reachable from an example — and this is
/// what holds them to the same integers. `CORPUS_CHECKSUMS` in that test file carries the same four
/// numbers.
fn checksum(samples: &[i16]) -> u64 {
    samples.iter().fold(0xcbf2_9ce4_8422_2325, |hash, sample| {
        (hash ^ u64::from(u16::from_ne_bytes(sample.to_ne_bytes())))
            .wrapping_mul(0x0000_0100_0000_01b3)
    })
}

// ------------------------------------------------------------------------ the measurement ----

/// One row: a processor over a signal.
struct Row {
    processor: &'static str,
    signal: &'static str,
    nanoseconds_per_1000_positions: u128,
    real_time_ppm: u128,
}

/// Time one processor over one signal, and return the fastest repetition.
///
/// `prepare` is outside the clock and the checksum is printed, so nothing here can be optimised
/// away and nothing but the sample path is charged.
fn time_one(
    factory: &mut dyn FnMut() -> Option<Box<dyn FrameProcessor>>,
    signal: &[i16],
    repetitions: u32,
    sink_hint: &mut u64,
) -> Option<u128> {
    let format = StreamFormat::new(RATE, 1).ok()?;
    let mut best = u128::MAX;
    for _ in 0..repetitions {
        let mut processor = factory()?;
        let capability = processor.capability();
        processor.prepare(AudioDirection::Inbound, format).ok()?;
        let frame_positions = u32::try_from(FRAME_POSITIONS).unwrap_or(u32::MAX);
        let mut output = vec![
            0i16;
            usize::try_from(capability.max_output_positions(frame_positions))
                .unwrap_or(0)
        ];
        let mut scratch_buffer =
            vec![0i16; usize::try_from(capability.scratch_samples()).unwrap_or(0)];
        let mut observations = Vec::with_capacity(32);
        let mut position = 0u64;
        let start = Instant::now();
        for chunk in signal.chunks(FRAME_POSITIONS) {
            let frame = DspFrame::new(AudioDirection::Inbound, format, position, chunk);
            observations.clear();
            let mut scratch = Scratch::new(&mut scratch_buffer);
            let mut written = FrameSink::new(&mut output, &mut observations, 32);
            processor.process(&frame, &mut scratch, &mut written).ok()?;
            position += u64::try_from(chunk.len()).unwrap_or(0);
        }
        let elapsed = start.elapsed().as_nanos();
        // Read the output after the clock stops, so the compiler cannot drop the work that made it
        // and the read is not charged to the processor.
        *sink_hint = sink_hint.wrapping_add(checksum(&output));
        best = best.min(elapsed);
    }
    Some(best)
}

/// Turn a fastest-repetition duration into the two figures the table reports.
fn row(processor: &'static str, signal: &'static str, nanoseconds: u128, positions: usize) -> Row {
    let per_1000 = nanoseconds * 1_000 / u128::try_from(positions).unwrap_or(1).max(1);
    Row {
        processor,
        signal,
        nanoseconds_per_1000_positions: per_1000,
        real_time_ppm: per_1000 * 1_000_000 / (POSITION_BUDGET_NANOSECONDS * 1_000),
    }
}

/// The harness's proof that it is measuring anything: sixteen gain stages must cost at least eight
/// times one.
///
/// Without this a run whose workload the optimiser removed reports every processor at the clock's
/// resolution and looks like very good news. The factor is eight rather than sixteen because the
/// per-frame overhead the chain does not multiply is real and is charged once.
fn clock_is_working(
    signal: &[i16],
    repetitions: u32,
    sink_hint: &mut u64,
) -> Result<(u128, u128), String> {
    let one = time_one(
        &mut || Some(Box::new(Gain::new())),
        signal,
        repetitions,
        sink_hint,
    )
    .ok_or_else(|| "the control processor refused the reference stream".to_owned())?;
    let sixteen = time_one(
        &mut || Some(Box::new(Chain::new(16))),
        signal,
        repetitions,
        sink_hint,
    )
    .ok_or_else(|| "the control chain refused the reference stream".to_owned())?;
    if sixteen < one.saturating_mul(8) {
        return Err(format!(
            "sixteen gain stages cost {sixteen} ns against one stage's {one} ns — under eight \
             times, so this harness is not measuring the work it thinks it is"
        ));
    }
    Ok((one, sixteen))
}

/// Sixteen `sipx.gain` stages behind one processor, for [`clock_is_working`] and nothing else.
struct Chain {
    stages: Vec<Gain>,
    buffer: Vec<i16>,
}

impl Chain {
    fn new(stages: usize) -> Self {
        Self {
            stages: (0..stages).map(|_| Gain::new()).collect(),
            buffer: Vec::new(),
        }
    }
}

impl FrameProcessor for Chain {
    fn capability(&self) -> sipx_audio::dsp::DspCapability {
        Gain::new().capability().with_id("control.chain")
    }

    fn configure(
        &mut self,
        parameters: &[Parameter],
    ) -> Result<(), sipx_audio::dsp::ParameterError> {
        for stage in &mut self.stages {
            stage.configure(parameters)?;
        }
        Ok(())
    }

    fn prepare(
        &mut self,
        direction: AudioDirection,
        format: StreamFormat,
    ) -> Result<(), sipx_audio::dsp::FormatError> {
        for stage in &mut self.stages {
            stage.prepare(direction, format)?;
        }
        Ok(())
    }

    fn process(
        &mut self,
        frame: &DspFrame<'_>,
        scratch: &mut Scratch<'_>,
        sink: &mut FrameSink<'_>,
    ) -> Result<(), sipx_audio::dsp::ProcessError> {
        self.buffer.clear();
        self.buffer.extend_from_slice(frame.samples());
        let mut carried = std::mem::take(&mut self.buffer);
        for stage in &mut self.stages {
            let mut observations = Vec::new();
            let mut output = vec![0i16; carried.len()];
            {
                let mut stage_sink = FrameSink::new(&mut output, &mut observations, 32);
                let staged = DspFrame::new(
                    frame.direction(),
                    frame.format(),
                    frame.position(),
                    carried.as_slice(),
                );
                stage.process(&staged, scratch, &mut stage_sink)?;
            }
            carried = output;
        }
        let result = sink.write(&carried);
        self.buffer = carried;
        result
    }

    fn flush(&mut self, _sink: &mut FrameSink<'_>) -> Result<(), sipx_audio::dsp::ProcessError> {
        Ok(())
    }

    fn reset(&mut self, cause: sipx_audio::dsp::DspResetCause) {
        for stage in &mut self.stages {
            stage.reset(cause);
        }
    }

    fn cancel(&mut self) {
        for stage in &mut self.stages {
            stage.cancel();
        }
    }

    fn retained(&self) -> u32 {
        0
    }
}

// ------------------------------------------------------------------------------ the run ----

/// Everything the command line can change, which is deliberately not much: the corpus, the frame
/// size and the reference rate are what a figure is comparable across runs *because* they are not
/// options.
struct Options {
    json: bool,
    under_load: bool,
    repetitions: u32,
    ceiling_percent: u64,
}

/// Parse the command line, or name the argument that was not understood.
fn options() -> Result<Options, String> {
    let mut parsed = Options {
        json: false,
        under_load: false,
        repetitions: DEFAULT_REPETITIONS,
        ceiling_percent: DEFAULT_LOAD_CEILING_PERCENT,
    };
    let mut arguments = std::env::args().skip(1);
    while let Some(argument) = arguments.next() {
        match argument.as_str() {
            "--json" => parsed.json = true,
            "--under-load" => parsed.under_load = true,
            "--repetitions" => {
                parsed.repetitions = arguments
                    .next()
                    .and_then(|value| value.parse().ok())
                    .unwrap_or(parsed.repetitions);
            }
            "--load-ceiling" => {
                parsed.ceiling_percent = arguments
                    .next()
                    .and_then(|value| value.parse().ok())
                    .unwrap_or(parsed.ceiling_percent);
            }
            other => return Err(format!("unknown argument {other}")),
        }
    }
    Ok(parsed)
}

/// Every row of the table, in the order it is printed.
fn measure_all(reference: &[i16], repetitions: u32, sink_hint: &mut u64) -> Vec<Row> {
    let mut rows = Vec::new();
    {
        let mut effect =
            |name: &'static str, factory: &mut dyn FnMut() -> Option<Box<dyn FrameProcessor>>| {
                if let Some(nanoseconds) = time_one(factory, reference, repetitions, sink_hint) {
                    rows.push(row(name, "stationary", nanoseconds, reference.len()));
                }
            };
        effect("sipx.gain", &mut || Some(Box::new(Gain::new())));
        effect("sipx.polarity", &mut || Some(Box::new(Polarity::new())));
        effect("sipx.hard_clip", &mut || Some(Box::new(HardClip::new())));
        effect("sipx.soft_clip", &mut || Some(Box::new(SoftClip::new())));
        effect("sipx.bit_crush", &mut || Some(Box::new(BitCrush::new())));
        effect("sipx.low_pass", &mut || Some(Box::new(LowPass::new())));
        effect("sipx.high_pass", &mut || Some(Box::new(HighPass::new())));
        effect("sipx.peaking", &mut || Some(Box::new(Peaking::new())));
        // A 20 ms delay line: the one built-in that owns heap, and the one whose cost is a memcpy
        // rather than arithmetic.
        effect("sipx.stutter", &mut || {
            Stutter::new(160)
                .ok()
                .map(|stutter| Box::new(stutter) as Box<dyn FrameProcessor>)
        });
    }

    // The reducer over all four of §8's conditions, because its cost is the one in this workspace
    // that could plausibly depend on what it is fed: three envelope followers, a recursive minimum
    // and a slew, all of them data-dependent.
    for name in ["silence", "stationary", "transient", "overlapping"] {
        let signal = condition(name);
        if let Some(nanoseconds) = time_one(
            &mut || Some(Box::new(SubbandSuppressor::new()) as Box<dyn FrameProcessor>),
            &signal,
            repetitions,
            sink_hint,
        ) {
            rows.push(row(
                "sipx.subband_suppressor",
                leak(name),
                nanoseconds,
                signal.len(),
            ));
        }
    }
    rows
}

fn main() -> ExitCode {
    let Options {
        json,
        under_load,
        repetitions,
        ceiling_percent,
    } = match options() {
        Ok(parsed) => parsed,
        Err(reason) => {
            eprintln!("dsp-cost: {reason}");
            return ExitCode::from(2);
        }
    };

    if cfg!(debug_assertions) {
        eprintln!(
            "dsp-cost: this is a debug build, where integer overflow checks are compiled into every \
             one of these processors and are not in a release call path — nothing was measured"
        );
        eprintln!("  cargo build --release -p sipx-audio --example dsp_cost");
        return ExitCode::from(2);
    }

    let mut machine = Machine::read();
    if !under_load && let Err(reason) = machine.is_quiet_enough(ceiling_percent) {
        eprintln!("dsp-cost: {reason}");
        eprintln!("  --under-load runs anyway and marks every figure as untrustworthy");
        return ExitCode::from(2);
    }

    let mut sink_hint = 0u64;
    let reference = condition("stationary");
    let control = match clock_is_working(&reference, repetitions, &mut sink_hint) {
        Ok(pair) => pair,
        Err(reason) => {
            eprintln!("dsp-cost: {reason}");
            return ExitCode::from(2);
        }
    };

    let rows = measure_all(&reference, repetitions, &mut sink_hint);

    machine.load_after = read_load();

    // The same control the run opened with, timed again now that every row is behind us. This is
    // the guard that caught the box `X-109` was written on.
    let control_after = time_one(
        &mut || Some(Box::new(Gain::new())),
        &reference,
        repetitions,
        &mut sink_hint,
    )
    .unwrap_or(u128::MAX);
    let drift_percent =
        control_after.abs_diff(control.0) * 100 / control.0.min(control_after).max(1);
    if drift_percent > REPEATABILITY_TOLERANCE_PERCENT && !under_load {
        eprintln!(
            "dsp-cost: the control cost {} ns before the run and {control_after} ns after — {drift_percent}% \
             apart against a {REPEATABILITY_TOLERANCE_PERCENT}% tolerance, so the box did not stay \
             still and nothing was measured",
            control.0,
        );
        eprintln!("  --under-load runs anyway and marks every figure as untrustworthy");
        return ExitCode::from(2);
    }
    let control = (control.0, control.1, control_after);
    let trustworthy = !under_load;
    if json {
        println!(
            "{}",
            as_json(&machine, &rows, repetitions, trustworthy, control)
        );
    } else {
        print_table(&machine, &rows, repetitions, trustworthy, control);
    }
    // Printed so that nothing above can be removed as dead: every measured sample reaches here.
    println!("checksum {sink_hint:#018x}");
    ExitCode::SUCCESS
}

/// The four condition names are compile-time strings; this is the one place that has to say so.
fn leak(name: &str) -> &'static str {
    match name {
        "silence" => "silence",
        "transient" => "transient",
        "overlapping" => "overlapping",
        _ => "stationary",
    }
}

fn banner(trustworthy: bool) -> &'static str {
    if trustworthy {
        ""
    } else {
        "  UNDER LOAD — not a measurement"
    }
}

fn print_table(
    machine: &Machine,
    rows: &[Row],
    repetitions: u32,
    trustworthy: bool,
    control: (u128, u128, u128),
) {
    println!(
        "dsp-cost: {WORKLOAD_POSITIONS} positions per repetition ({SILENCE_POSITIONS} for \
         silence), {repetitions} repetitions, fastest kept"
    );
    println!("  machine     {} × {}", machine.cores, machine.model);
    println!(
        "  load        {} before, {} after (one-minute average)",
        machine
            .load_before
            .map_or("unknown".to_owned(), |load| format!("{load:.2}")),
        machine
            .load_after
            .map_or("unknown".to_owned(), |load| format!("{load:.2}")),
    );
    println!(
        "  clock check one gain stage {} ns, sixteen {} ns — {}×",
        control.0,
        control.1,
        control.1 / control.0.max(1),
    );
    println!(
        "  stability   control {} ns before the run, {} ns after — {}% apart",
        control.0,
        control.2,
        control.2.abs_diff(control.0) * 100 / control.0.min(control.2).max(1),
    );
    for name in ["silence", "stationary", "transient", "overlapping"] {
        let signal = condition(name);
        println!(
            "  corpus      {name:<12} {:>6} positions, checksum {:#018x}",
            signal.len(),
            checksum(&signal),
        );
    }
    println!();
    println!(
        "  {:<28} {:<12} {:>12} {:>10}",
        "processor", "signal", "ns/1000 pos", "ppm core"
    );
    for row in rows {
        println!(
            "  {:<28} {:<12} {:>12} {:>10}{}",
            row.processor,
            row.signal,
            row.nanoseconds_per_1000_positions,
            row.real_time_ppm,
            banner(trustworthy),
        );
    }
    if !trustworthy {
        println!();
        println!("  every figure above was taken under load and is not a measurement");
    }
}

fn as_json(
    machine: &Machine,
    rows: &[Row],
    repetitions: u32,
    trustworthy: bool,
    control: (u128, u128, u128),
) -> String {
    let mut out = String::new();
    let mut field = |line: &str| {
        let _ = writeln!(out, "  {line},");
    };
    field(&format!("\"trustworthy\": {trustworthy}"));
    field(&format!("\"rate_hz\": {RATE}"));
    field(&format!("\"frame_positions\": {FRAME_POSITIONS}"));
    field(&format!("\"workload_positions\": {WORKLOAD_POSITIONS}"));
    field(&format!("\"silence_positions\": {SILENCE_POSITIONS}"));
    field(&format!("\"repetitions\": {repetitions}"));
    field(&format!("\"cores\": {}", machine.cores));
    field(&format!("\"model\": {:?}", machine.model));
    field(&format!(
        "\"load_before\": {:?}",
        show_load(machine.load_before)
    ));
    field(&format!(
        "\"load_after\": {:?}",
        show_load(machine.load_after)
    ));
    field(&format!("\"control_one_stage_ns\": {}", control.0));
    field(&format!("\"control_sixteen_stages_ns\": {}", control.1));
    field(&format!("\"control_one_stage_ns_after\": {}", control.2));
    for name in ["silence", "stationary", "transient", "overlapping"] {
        field(&format!(
            "\"corpus_{name}_checksum\": \"{:#018x}\"",
            checksum(&condition(name))
        ));
    }
    let mut body = String::new();
    for (index, row) in rows.iter().enumerate() {
        let comma = if index + 1 == rows.len() { "" } else { "," };
        let _ = writeln!(
            body,
            "    {{\"processor\": {:?}, \"signal\": {:?}, \"ns_per_1000_positions\": {}, \
             \"real_time_ppm\": {}}}{comma}",
            row.processor, row.signal, row.nanoseconds_per_1000_positions, row.real_time_ppm,
        );
    }
    format!("{{\n{out}  \"rows\": [\n{body}  ]\n}}")
}
