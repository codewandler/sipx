//! The measurement behind `DSP-K9`'s heap component: `docs/specs/custom-call-dsp.md` §9.1.
//!
//! # Why this is not in the workspace
//!
//! §9.1 said a processor's heap growth "cannot be observed here" and stopped there. The first half
//! was true and the second half was a conclusion drawn too early. Counting allocations needs a
//! global allocator, a global allocator needs `unsafe impl GlobalAlloc`, and
//! `[workspace.lints.rust] unsafe_code = "forbid"` reaches rustc as `-F unsafe_code` for **every**
//! target of every workspace member — an integration test under `crates/sipx-audio/tests/` included.
//! A `forbid` cannot be relaxed by an `allow` either; that attempt is `E0453`, "overruled by
//! previous forbid". Inside the workspace there is no door, and looking for one harder was not going
//! to open it.
//!
//! So this package sits outside the workspace, exactly as `wasm/` does for its WebAssembly exports
//! and `fuzz/` does for its nightly harness. The root manifest already says why that is the right
//! shape: it keeps "no `unsafe`" intact "for every crate that answers to it", and it keeps
//! `scripts/comparison-report.py`'s generated `unsafe-policy` cell — which reads
//! `unsafe_code = "forbid"` straight out of the workspace manifest — a true statement about
//! everything sipx publishes.
//!
//! # What it measures
//!
//! Two figures per processor, because §9.1 makes two separate claims:
//!
//! 1. **Peak live bytes** across construction, `prepare`, `process`, `flush`, `reset` and `cancel`,
//!    held against the processor's declared `state_bytes` less its inline size. This is what
//!    `state_bytes` *is* — the memory the processor owns.
//! 2. **Bytes allocated after `prepare` returned**, which must be zero. This is §9.1's "no
//!    allocation after `prepare` — not per frame, not per position, not per observation", and until
//!    `X-128` nothing checked it at all.
//!
//! Neither figure is computed here. [`sipx_audio::dsp::Conformance::run_with_heap_meter`] holds
//! them to the declaration; this package supplies the bytes and nothing else. That split is
//! deliberate — if the comparison lived out here, a change in the workspace could stop calling it
//! and the gate would not notice.
//!
//! # What it does not measure
//!
//! Allocation on any thread but this one, memory a processor obtains without going through the
//! Rust global allocator, and address-space growth that is not an allocation. A processor doing any
//! of those is outside what this proves, and `DSP-K9` passing here is a statement about the
//! allocator's ledger rather than about the operating system's.
//!
//! # Running it
//!
//! `./scripts/check-dsp-heap.sh`. Exit `0` means every processor's declaration held; exit `1` means
//! one did not, and the line above the summary says which and by how much.

use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use std::process::ExitCode;

use sipx_audio::analysis::AudioDirection;
use sipx_audio::dsp::effects::{
    BitCrush, Gain, HardClip, HighPass, LowPass, MAX_STUTTER_POSITIONS, Peaking, Polarity,
    SoftClip, Stutter,
};
use sipx_audio::dsp::{
    CHECK_ALLOCATION, CheckStatus, Conformance, ConformanceReport, DspCapability, DspFrame,
    DspResetCause, FormatError, FrameAdmission, FrameProcessor, FrameSink, HeapMeter, HeapUse,
    Parameter, ParameterError, ProcessError, Scratch, StreamFormat,
};

// ------------------------------------------------------------- the counting allocator ----

/// Every counter one thread keeps, in one `Copy` cell so a count is a single TLS access.
#[derive(Clone, Copy)]
struct Counters {
    /// Bytes currently outstanding on this thread.
    live: u64,
    /// What `live` was when the window opened. Peaks are measured over this, not over zero, so the
    /// buffers the harness lends a processor do not read as the processor's own state.
    baseline: u64,
    /// Bytes handed out since the window opened, whether or not they were given back.
    allocated: u64,
    /// The high-water mark of `live - baseline` since the window opened.
    peak: u64,
}

thread_local! {
    /// Per thread, because `cargo` and this binary may both run work on threads of their own and a
    /// process-wide counter would measure whichever one happened to be busy.
    ///
    /// `const`-initialized and holding no `Drop` type: reading it allocates nothing, which matters
    /// when the reader is the allocator.
    static COUNTERS: Cell<Counters> = const {
        Cell::new(Counters { live: 0, baseline: 0, allocated: 0, peak: 0 })
    };
}

/// Record `bytes` handed out on this thread.
fn note_alloc(bytes: u64) {
    let _ = COUNTERS.try_with(|cell| {
        let mut counters = cell.get();
        counters.live = counters.live.saturating_add(bytes);
        counters.allocated = counters.allocated.saturating_add(bytes);
        counters.peak = counters
            .peak
            .max(counters.live.saturating_sub(counters.baseline));
        cell.set(counters);
    });
}

/// Record `bytes` given back on this thread.
fn note_dealloc(bytes: u64) {
    let _ = COUNTERS.try_with(|cell| {
        let mut counters = cell.get();
        counters.live = counters.live.saturating_sub(bytes);
        cell.set(counters);
    });
}

/// The system allocator with a per-thread ledger in front of it.
struct CountingAllocator;

// The only `unsafe` in this repository outside `wasm/`'s twelve export attributes, and it is two
// methods long on purpose. `GlobalAlloc`'s `realloc` and `alloc_zeroed` defaults are written in
// terms of `alloc` and `dealloc`, so leaving them alone counts a `Vec` growing or a `vec![0; n]`
// exactly right *and* keeps them out of the unsafe surface. Overriding them for speed would buy
// nothing a measurement run notices.
//
// The `allow` is on this item rather than on the crate — `wasm/src/lib.rs` needs a crate-level one
// because its twelve exports are spread across the file, and nothing here is. The package still
// declares `unsafe_code = "warn"`, so a second `unsafe` appearing anywhere else in this binary is a
// warning, and the check script runs it as `-D warnings`.
#[allow(unsafe_code)]
unsafe impl GlobalAlloc for CountingAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        // SAFETY: `layout` is the caller's obligation and is forwarded unchanged to the system
        // allocator, which is the only thing that ever hands out or reclaims this memory.
        let pointer = unsafe { System.alloc(layout) };
        if !pointer.is_null() {
            note_alloc(layout.size() as u64);
        }
        pointer
    }

    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        // SAFETY: `pointer` came from `Self::alloc` under this same `layout`, which forwarded it to
        // the system allocator; returning it there is the matching operation.
        unsafe { System.dealloc(pointer, layout) };
        note_dealloc(layout.size() as u64);
    }
}

#[global_allocator]
static ALLOCATOR: CountingAllocator = CountingAllocator;

/// The [`HeapMeter`] the conformance harness borrows. Zero-sized: the state is the thread's.
struct ThreadHeapMeter;

impl HeapMeter for ThreadHeapMeter {
    fn begin(&self) {
        let _ = COUNTERS.try_with(|cell| {
            let mut counters = cell.get();
            counters.baseline = counters.live;
            counters.allocated = 0;
            counters.peak = 0;
            cell.set(counters);
        });
    }

    fn measure(&self) -> HeapUse {
        COUNTERS
            .try_with(|cell| {
                let counters = cell.get();
                HeapUse::new(counters.allocated, counters.peak)
            })
            .unwrap_or_default()
    }
}

// ------------------------------------------------------------------ violating fixtures ----

/// §11.3, for the after-`prepare` half of `DSP-K9`: a processor that allocates once per frame and
/// keeps what it allocated.
///
/// Its declaration is generous — a megabyte of `state_bytes` — so that the *only* thing wrong with
/// it is when it allocates rather than how much. A fixture that broke both halves at once would not
/// tell us which half caught it.
#[derive(Default)]
struct HeapHog {
    admission: FrameAdmission,
    /// The violation: one fresh allocation per frame, retained.
    hoard: Vec<Vec<i16>>,
}

impl HeapHog {
    fn declaration() -> DspCapability {
        DspCapability::new("heap-hog").with_state_bytes(1 << 20)
    }
}

impl FrameProcessor for HeapHog {
    fn capability(&self) -> DspCapability {
        Self::declaration()
    }

    fn configure(&mut self, parameters: &[Parameter]) -> Result<(), ParameterError> {
        Self::declaration().validate_parameters(parameters)
    }

    fn prepare(
        &mut self,
        direction: AudioDirection,
        format: StreamFormat,
    ) -> Result<(), FormatError> {
        self.admission
            .prepare(&Self::declaration(), direction, format)
    }

    fn process(
        &mut self,
        frame: &DspFrame<'_>,
        _scratch: &mut Scratch<'_>,
        sink: &mut FrameSink<'_>,
    ) -> Result<(), ProcessError> {
        self.admission.admit(&Self::declaration(), frame)?;
        self.hoard.push(frame.samples().to_vec());
        sink.write(frame.samples())
    }

    fn flush(&mut self, _sink: &mut FrameSink<'_>) -> Result<(), ProcessError> {
        self.admission.admit_flush()
    }

    fn reset(&mut self, _cause: DspResetCause) {
        self.admission.reset();
    }

    fn cancel(&mut self) {
        self.admission.cancel();
        self.hoard = Vec::new();
    }

    fn retained(&self) -> u32 {
        0
    }
}

/// §11.3, for the `state_bytes` half of `DSP-K9`: a processor that owns a buffer it did not declare.
///
/// It allocates once, in `new`, and never again — so it satisfies "no allocation after `prepare`"
/// exactly, and the only thing it gets wrong is the figure. This is the fixture that would have
/// caught a `Stutter` whose delay line grew without its declaration following.
struct HeapLiar {
    admission: FrameAdmission,
    /// The violation: 64 KiB owned against 4,096 declared.
    undeclared: Vec<i16>,
}

impl HeapLiar {
    /// 32,768 positions of `i16` — 65,536 bytes — behind a default 4,096-byte declaration.
    fn new() -> Self {
        Self {
            admission: FrameAdmission::default(),
            undeclared: vec![0; 32_768],
        }
    }

    fn declaration() -> DspCapability {
        DspCapability::new("heap-liar")
    }
}

impl FrameProcessor for HeapLiar {
    fn capability(&self) -> DspCapability {
        Self::declaration()
    }

    fn configure(&mut self, parameters: &[Parameter]) -> Result<(), ParameterError> {
        Self::declaration().validate_parameters(parameters)
    }

    fn prepare(
        &mut self,
        direction: AudioDirection,
        format: StreamFormat,
    ) -> Result<(), FormatError> {
        self.admission
            .prepare(&Self::declaration(), direction, format)
    }

    fn process(
        &mut self,
        frame: &DspFrame<'_>,
        _scratch: &mut Scratch<'_>,
        sink: &mut FrameSink<'_>,
    ) -> Result<(), ProcessError> {
        self.admission.admit(&Self::declaration(), frame)?;
        sink.write(frame.samples())
    }

    fn flush(&mut self, _sink: &mut FrameSink<'_>) -> Result<(), ProcessError> {
        self.admission.admit_flush()
    }

    fn reset(&mut self, _cause: DspResetCause) {
        self.admission.reset();
        self.undeclared.fill(0);
    }

    fn cancel(&mut self) {
        self.admission.cancel();
    }

    fn retained(&self) -> u32 {
        0
    }
}

// ------------------------------------------------------------------------ the run ----

/// What one processor's `DSP-K9` outcome was, reduced to what this binary prints.
struct Measured {
    status: CheckStatus,
    detail: String,
    other_failures: Vec<String>,
}

/// Run one processor through the harness with the meter installed and pull out `DSP-K9`.
fn measure<P, F>(factory: F) -> Measured
where
    P: FrameProcessor,
    F: FnMut() -> P,
{
    let report: ConformanceReport =
        Conformance::new().run_with_heap_meter(factory, &ThreadHeapMeter);
    let allocation = report
        .checks()
        .iter()
        .find(|check| check.id() == CHECK_ALLOCATION);
    let other_failures = report
        .failures()
        .filter(|check| check.id() != CHECK_ALLOCATION)
        .map(ToString::to_string)
        .collect();
    match allocation {
        Some(check) => Measured {
            status: check.status(),
            detail: check.detail().to_owned(),
            other_failures,
        },
        None => Measured {
            status: CheckStatus::Failed,
            detail: "DSP-K9 was absent from the report".to_owned(),
            other_failures,
        },
    }
}

/// Report a processor that must come back measured and within its declaration.
fn expect_measured(name: &str, measured: &Measured) -> bool {
    let ok = measured.status == CheckStatus::Passed && measured.other_failures.is_empty();
    let label = if ok { "ok" } else { "FAILED" };
    println!("  {name:<28} {label}");
    println!("      {}", measured.detail);
    for failure in &measured.other_failures {
        println!("      also failed: {failure}");
    }
    if measured.status == CheckStatus::Unproven {
        println!("      the meter was installed and DSP-K9 still reported unproven");
    }
    ok
}

/// Report a fixture that must be caught.
fn expect_caught(name: &str, measured: &Measured) -> bool {
    let ok = measured.status == CheckStatus::Failed;
    let label = if ok { "caught" } else { "NOT CAUGHT" };
    println!("  {name:<28} {label}");
    println!("      {}", measured.detail);
    ok
}

/// The negative control: the same processor, through the same harness, with no meter.
///
/// This is what says the measurement is load-bearing. A fixture caught under the meter proves the
/// check can fail; the same fixture coming back `Unproven` — not `Failed`, and not `Passed` —
/// without one proves that catching it was the meter's doing and not some other check noticing in
/// passing. Without this line, a `DSP-K9` failure attributed to the heap could be anything.
fn expect_unproven_without_meter<P, F>(name: &str, factory: F) -> bool
where
    P: FrameProcessor,
    F: FnMut() -> P,
{
    let report = Conformance::new().run(factory);
    let status = report
        .checks()
        .iter()
        .find(|check| check.id() == CHECK_ALLOCATION)
        .map(|check| check.status());
    let ok = status == Some(CheckStatus::Unproven);
    let label = if ok {
        "unproven, as it must be"
    } else {
        "EXPECTED UNPROVEN"
    };
    println!("  {name:<28} {label}");
    ok
}

fn main() -> ExitCode {
    let mut ok = true;

    println!("built-in processors, measured against their declared state_bytes:");
    ok &= expect_measured("sipx.gain", &measure(Gain::new));
    ok &= expect_measured("sipx.polarity", &measure(Polarity::new));
    ok &= expect_measured("sipx.hard_clip", &measure(HardClip::new));
    ok &= expect_measured("sipx.soft_clip", &measure(SoftClip::new));
    ok &= expect_measured("sipx.bit_crush", &measure(BitCrush::new));
    ok &= expect_measured("sipx.low_pass", &measure(LowPass::new));
    ok &= expect_measured("sipx.high_pass", &measure(HighPass::new));
    ok &= expect_measured("sipx.peaking", &measure(Peaking::new));
    // The one built-in with a heap component. Its declaration is a function of the line it was
    // built with, so every size is a separate claim and gets a separate measurement.
    //
    // `Stutter::new` refuses exactly one thing — a delay past `MAX_STUTTER_POSITIONS` — and no
    // value in this list is one. The harness catches a panic and reports it as a finding regardless.
    #[allow(clippy::unwrap_used)]
    for delay in [0, 1, 4, MAX_STUTTER_POSITIONS] {
        let name = format!("sipx.stutter({delay})");
        ok &= expect_measured(&name, &measure(|| Stutter::new(delay).unwrap()));
    }

    println!();
    println!("§11.3 fixtures, which must be caught:");
    ok &= expect_caught("HeapHog (per-frame)", &measure(HeapHog::default));
    ok &= expect_caught("HeapLiar (undeclared state)", &measure(HeapLiar::new));

    println!();
    println!("the same two with no meter, which must go back to unproven:");
    ok &= expect_unproven_without_meter("HeapHog", HeapHog::default);
    ok &= expect_unproven_without_meter("HeapLiar", HeapLiar::new);

    println!();
    if ok {
        println!("dsp-heap: every declaration held and both violating fixtures were caught");
        ExitCode::SUCCESS
    } else {
        println!("dsp-heap: a declaration did not hold, or a fixture was not caught");
        ExitCode::FAILURE
    }
}
