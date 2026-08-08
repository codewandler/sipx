//! The `sipx.browser.v1` WebAssembly module.
//!
//! Twelve exports, one per row of [`docs/specs/browser-sdk.md`](../../docs/specs/browser-sdk.md)
//! §4.3, each a single call into [`sipx_wasm::Abi`]. There is no logic here and there must never
//! be any: the kernel's behaviour has to be the same object the native vector suite runs against,
//! or "identical native and WASM results" is a claim about two different programs.
//!
//! # Why this package is outside the workspace
//!
//! `unsafe_code = "forbid"` is a workspace non-negotiable, and a WebAssembly export needs
//! `#[unsafe(no_mangle)]`, which that lint refuses. Rather than weaken the lint for the twelve
//! crates that answer to it, this package sits outside the workspace exactly as `fuzz/` does — a
//! build target that cannot obey the workspace's rules gets its own manifest, and the diff that
//! moved it there is reviewable.
//!
//! What the allowance buys is precisely twelve attributes. There is **no `unsafe` block, no
//! `unsafe fn` and no raw-pointer dereference anywhere in this file or in `sipx-wasm`**: the ABI's
//! `sipx_alloc` hands out an offset into a buffer the kernel keeps, and the entry points read
//! their own allocations back. §4.4's bounds rules are enforced over that table, which is why a
//! pointer the host never obtained is `E_BAD_POINTER` rather than a memory fault.
//!
//! # Imports
//!
//! There are none, and §4.1 requires that: a module with no imports cannot call the host, so
//! reentrancy is structurally impossible rather than merely forbidden. `harness.mjs` asserts it
//! against the built artifact.

// The whole exception, in one greppable line. Everything above explains it; nothing below adds
// an `unsafe` block, an `unsafe fn` or a raw-pointer dereference. `[lints.rust] unsafe_code` is
// left at `warn` in the manifest so that removing this line makes the twelve attributes visible
// again rather than silently permitted.
#![allow(unsafe_code)]

use std::cell::RefCell;

use sipx_wasm::Abi;

thread_local! {
    /// One instantiation's ABI state.
    ///
    /// §4.1: one module instantiation serves one JavaScript agent, and sharing an instance across
    /// workers is outside the contract. A thread-local rather than a global is that sentence in
    /// the type system — and on this target there is exactly one thread, because §4.1 also rules
    /// out threads, atomics and shared memory.
    static ABI: RefCell<Abi> = RefCell::new(Abi::new());
}

/// Run `f` against this instantiation's ABI.
///
/// A re-entrant call would find the `RefCell` borrowed; it cannot happen, because the module
/// imports nothing and therefore never yields to the host mid-call, but `try_borrow_mut` states
/// that rather than assuming it. `fallback` is what a violated assumption returns.
fn with_abi<T>(fallback: T, f: impl FnOnce(&mut Abi) -> T) -> T {
    ABI.with(|abi| match abi.try_borrow_mut() {
        Ok(mut abi) => f(&mut abi),
        Err(_) => fallback,
    })
}

/// §4.3 `sipx_abi_version`: the ABI integer. Generated glue must refuse a mismatch at load.
#[unsafe(no_mangle)]
pub extern "C" fn sipx_abi_version() -> i32 {
    sipx_wasm::ABI_VERSION
}

/// §4.3 `sipx_alloc`: allocate a host-input buffer; `0` on failure.
#[unsafe(no_mangle)]
pub extern "C" fn sipx_alloc(len: u32) -> u32 {
    with_abi(0, |abi| abi.alloc(len))
}

/// §4.3 `sipx_free`: release a buffer obtained from `sipx_alloc`.
#[unsafe(no_mangle)]
pub extern "C" fn sipx_free(ptr: u32, len: u32) {
    with_abi((), |abi| abi.free(ptr, len));
}

/// §4.3 `sipx_kernel_new`: create a kernel from a `BSDK-CFG` document.
#[unsafe(no_mangle)]
pub extern "C" fn sipx_kernel_new(cfg_ptr: u32, cfg_len: u32) -> i32 {
    with_abi(E_POISONED, |abi| abi.kernel_new(cfg_ptr, cfg_len))
}

/// §4.3 `sipx_kernel_free`: cancel everything and destroy the kernel.
#[unsafe(no_mangle)]
pub extern "C" fn sipx_kernel_free(handle: i32) -> i32 {
    with_abi(E_POISONED, |abi| abi.kernel_free(handle))
}

/// §4.3 `sipx_command`: submit one §5.2 command.
#[unsafe(no_mangle)]
pub extern "C" fn sipx_command(handle: i32, ptr: u32, len: u32, now_ms: u64) -> i32 {
    with_abi(E_POISONED, |abi| abi.command(handle, ptr, len, now_ms))
}

/// §4.3 `sipx_input_bytes`: one received signalling message.
#[unsafe(no_mangle)]
pub extern "C" fn sipx_input_bytes(handle: i32, ptr: u32, len: u32, now_ms: u64) -> i32 {
    with_abi(E_POISONED, |abi| abi.input_bytes(handle, ptr, len, now_ms))
}

/// §4.3 `sipx_input_timer`: a previously requested timer fired.
#[unsafe(no_mangle)]
pub extern "C" fn sipx_input_timer(handle: i32, timer_id: u64, now_ms: u64) -> i32 {
    with_abi(E_POISONED, |abi| abi.input_timer(handle, timer_id, now_ms))
}

/// §4.3 `sipx_input_entropy`: append host entropy to the pool.
#[unsafe(no_mangle)]
pub extern "C" fn sipx_input_entropy(handle: i32, ptr: u32, len: u32) -> i32 {
    with_abi(E_POISONED, |abi| abi.input_entropy(handle, ptr, len))
}

/// §4.3 `sipx_next_output`: the packed buffer of the next output record; `0` when drained.
#[unsafe(no_mangle)]
pub extern "C" fn sipx_next_output(handle: i32) -> u64 {
    with_abi(0, |abi| abi.next_output(handle))
}

/// §4.3 `sipx_snapshot`: the packed buffer of a read-only JSON state and counter snapshot.
#[unsafe(no_mangle)]
pub extern "C" fn sipx_snapshot(handle: i32) -> u64 {
    with_abi(0, |abi| abi.snapshot(handle))
}

/// §4.9's teardown timers: how many `TIMER_CANCEL` records the last `sipx_kernel_free` produced.
///
/// §6.5 step 4 makes the glue clear every host timer the kernel owned, and the handle those timers
/// belonged to no longer exists once the free returns — so they cannot be drained through
/// `sipx_next_output`. This pair of exports is the seam that lets the glue read them anyway.
#[unsafe(no_mangle)]
pub extern "C" fn sipx_teardown_timer_count() -> u32 {
    with_abi(0, |abi| {
        u32::try_from(abi.last_teardown_cancellations().len()).unwrap_or(u32::MAX)
    })
}

/// The timer id of the `index`th teardown cancellation, or `0` when `index` is out of range.
///
/// Timer ids are monotonically increasing from `1` (§4.5), so `0` is unambiguous.
#[unsafe(no_mangle)]
pub extern "C" fn sipx_teardown_timer_id(index: u32) -> u64 {
    with_abi(0, |abi| {
        match abi.last_teardown_cancellations().get(index as usize) {
            Some(sipx_wasm::Record::TimerCancel(id)) => *id,
            _ => 0,
        }
    })
}

/// `E_POISONED` from §4.10, returned when the ABI cannot be entered at all.
const E_POISONED: i32 = -12;
