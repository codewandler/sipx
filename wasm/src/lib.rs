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

macro_rules! export_abi {
    ($($item:item)*) => { $($item)* };
}
sipx_wasm::browser_abi!(export_abi);
