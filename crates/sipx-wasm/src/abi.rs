//! The kernel ABI (`docs/specs/browser-sdk.md` §4).
//!
//! One method here per §4.3 export, with the same name, the same argument order and the same
//! return convention. The WebAssembly module's `extern "C"` shims are one line each and do
//! nothing but call these; keeping the logic on this side is what lets §9's vectors run
//! unchanged on native Rust and on WebAssembly, which is the whole of `S-41`'s fifth acceptance
//! row.
//!
//! # Memory, without `unsafe`
//!
//! `sipx_alloc` hands out an offset into a buffer the kernel **keeps**. The host writes into
//! linear memory at that offset — in the browser that is literally this buffer — and the entry
//! point then reads its own allocation back. Nothing here dereferences a host pointer, so §4.4's
//! rules are expressed over an allocation table rather than over raw memory, and
//! `unsafe_code = "forbid"` holds for every line of the kernel.
//!
//! A pointer that is not a live allocation, or a length that runs past one, is `E_BAD_POINTER`
//! by the same table — which is exactly what §9.5's `BSDK-NEG-2` asks for.

use std::collections::BTreeMap;

use crate::bounds;
use crate::config::Config;
use crate::error::Error;
use crate::kernel::Kernel;
use crate::output::Record;

/// The ABI integer this crate implements. Generated glue must refuse a mismatch at load.
pub const ABI_VERSION: i32 = 1;

/// A buffer handed to the host by [`Abi::alloc`].
#[derive(Debug)]
struct Allocation {
    bytes: Vec<u8>,
}

/// One kernel handle and the buffer it currently lends the host.
#[derive(Debug)]
struct Slot {
    kernel: Kernel,
    /// §4.4: valid until the next call of **any** export on this handle. Per slot rather than per
    /// instance, because a borrow taken from one handle survives a call on another.
    borrowed: Vec<u8>,
    /// The offset the host was last given for `borrowed`, so the pack is stable while it lives.
    borrowed_key: u32,
}

/// One module instantiation's worth of ABI state.
///
/// §4.1: one instantiation serves one JavaScript agent, and sharing one across workers is outside
/// the contract. The type is therefore deliberately not `Sync`; single-agent use is the host's
/// half of §4.8 and the generated glue enforces it.
#[derive(Debug)]
pub struct Abi {
    slots: BTreeMap<i32, Slot>,
    /// Handles are never reused within an instantiation, so use-after-free is deterministically
    /// `E_INVALID_HANDLE` rather than a live kernel someone else is driving.
    next_handle: i32,
    allocations: BTreeMap<u32, Allocation>,
    /// Synthetic keys for targets whose pointers do not fit in a `u32` — see [`Abi::alloc`].
    next_key: u32,
    /// The `TIMER_CANCEL` records the last teardown produced; see
    /// [`Abi::last_teardown_cancellations`].
    pending_cancellations: Vec<Record>,
}

impl Default for Abi {
    fn default() -> Self {
        Self::new()
    }
}

impl Abi {
    /// A fresh instantiation with no handles and no allocations.
    #[must_use]
    pub fn new() -> Self {
        Self {
            slots: BTreeMap::new(),
            next_handle: 1,
            allocations: BTreeMap::new(),
            // Keys start high enough that a synthetic one can never be mistaken for the null
            // pointer the packed-buffer convention reserves for "none".
            next_key: 0x0001_0000,
            pending_cancellations: Vec::new(),
        }
    }

    /// §4.3 `sipx_abi_version`.
    #[must_use]
    pub fn abi_version(&self) -> i32 {
        ABI_VERSION
    }

    /// §4.3 `sipx_alloc`: allocate a host-input buffer; `0` on failure.
    ///
    /// On a 32-bit target — the browser build — the returned key **is** the linear-memory offset
    /// of the buffer, which is what makes the host's write land in it. Elsewhere a pointer does
    /// not fit in a `u32`, so the table hands out a synthetic key instead and the host writes
    /// through [`Abi::write`]. Every §4.4 rule is expressed over the table, so both builds refuse
    /// exactly the same inputs.
    pub fn alloc(&mut self, len: u32) -> u32 {
        let len = len as usize;
        if len > bounds::MAX_SIP_MESSAGE.max(bounds::MAX_COMMAND) + 1 {
            // Nothing the ABI accepts is larger than the biggest §4.9 input bound plus the one
            // octet a `BSDK-NEG-7`/`BSDK-NEG-12` overshoot needs, so a larger request is a host
            // bug rather than a memory shortage.
            return 0;
        }
        // A zero-length allocation would have a dangling pointer and no octet to key on, and `0`
        // already means failure. One octet of slack costs nothing and keeps the key meaningful.
        let bytes = vec![0u8; len.max(1)];
        let key = self.key_for(&bytes);
        if key == 0 || self.allocations.contains_key(&key) {
            return 0;
        }
        self.allocations.insert(key, Allocation { bytes });
        key
    }

    /// §4.3 `sipx_free`: release a buffer obtained from [`Abi::alloc`].
    pub fn free(&mut self, ptr: u32, _len: u32) {
        self.allocations.remove(&ptr);
    }

    /// Copy host bytes into an allocation.
    ///
    /// In the browser the host writes straight into linear memory at the offset [`Abi::alloc`]
    /// returned; a native test has no linear memory to write into and writes through here
    /// instead. Both land in the same buffer, which is what lets one vector suite drive both.
    ///
    /// Returns `false` when `ptr` is not a live allocation or the bytes do not fit — the same
    /// condition the entry points report as `E_BAD_POINTER`.
    pub fn write(&mut self, ptr: u32, bytes: &[u8]) -> bool {
        let Some(allocation) = self.allocations.get_mut(&ptr) else {
            return false;
        };
        let Some(target) = allocation.bytes.get_mut(..bytes.len()) else {
            return false;
        };
        target.copy_from_slice(bytes);
        true
    }

    /// Allocate and fill in one step, returning the offset the entry points take.
    ///
    /// A convenience over [`Abi::alloc`] and [`Abi::write`] for hosts and tests that already hold
    /// the bytes; it grants no capability the two separate calls do not.
    pub fn alloc_with(&mut self, bytes: &[u8]) -> u32 {
        let len = u32::try_from(bytes.len()).unwrap_or(u32::MAX);
        let ptr = self.alloc(len);
        if ptr != 0 && !self.write(ptr, bytes) {
            self.free(ptr, len);
            return 0;
        }
        ptr
    }

    /// Read back a host input, applying §4.4's bounds rule.
    fn read(&self, ptr: u32, len: u32) -> Result<Vec<u8>, Error> {
        let allocation = self.allocations.get(&ptr).ok_or(Error::BadPointer)?;
        let len = len as usize;
        allocation
            .bytes
            .get(..len)
            .map(<[u8]>::to_vec)
            .ok_or(Error::BadPointer)
    }

    /// §4.3 `sipx_kernel_new`: create a kernel from a `BSDK-CFG` document.
    pub fn kernel_new(&mut self, cfg_ptr: u32, cfg_len: u32) -> i32 {
        if self.slots.len() >= bounds::MAX_HANDLES {
            return Error::Limit.code();
        }
        let document = match self.read(cfg_ptr, cfg_len) {
            Ok(document) => document,
            Err(error) => return error.code(),
        };
        let config = match Config::parse(&document) {
            Ok(config) => config,
            Err(error) => return error.code(),
        };
        let handle = self.next_handle;
        self.next_handle = self.next_handle.saturating_add(1);
        self.slots.insert(
            handle,
            Slot {
                kernel: Kernel::new(config),
                borrowed: Vec::new(),
                borrowed_key: 0,
            },
        );
        handle
    }

    /// §4.3 `sipx_kernel_free`: cancel everything and destroy the kernel.
    ///
    /// A second call is `E_INVALID_HANDLE`, because handles are never reused. The `TIMER_CANCEL`
    /// records the teardown produces are returned to the caller rather than queued: §9.6's
    /// `BSDK-STATE-6` says no output record survives the free, and the glue clears the host
    /// timers from this list (§6.5 step 4).
    pub fn kernel_free(&mut self, handle: i32) -> i32 {
        let Some(mut slot) = self.slots.remove(&handle) else {
            return Error::InvalidHandle.code();
        };
        let cancellations = slot.kernel.shutdown();
        self.pending_cancellations = cancellations;
        0
    }

    /// The `TIMER_CANCEL` records the last [`Abi::kernel_free`] produced.
    ///
    /// The glue clears these host-side timers as §6.5 step 4 requires. They are read from here
    /// rather than drained through `sipx_next_output` because the handle they belonged to no
    /// longer exists — draining them would need a handle the contract has just invalidated.
    #[must_use]
    pub fn last_teardown_cancellations(&self) -> &[Record] {
        &self.pending_cancellations
    }

    /// §4.3 `sipx_command`: submit one §5.2 command.
    pub fn command(&mut self, handle: i32, ptr: u32, len: u32, now_ms: u64) -> i32 {
        self.with_bytes(handle, ptr, len, |kernel, bytes| {
            kernel.command(bytes, now_ms)
        })
    }

    /// §4.3 `sipx_input_bytes`: one received signalling message.
    pub fn input_bytes(&mut self, handle: i32, ptr: u32, len: u32, now_ms: u64) -> i32 {
        self.with_bytes(handle, ptr, len, |kernel, bytes| {
            kernel.input_bytes(bytes, now_ms)
        })
    }

    /// §4.3 `sipx_input_entropy`: append host entropy to the pool.
    pub fn input_entropy(&mut self, handle: i32, ptr: u32, len: u32) -> i32 {
        self.with_bytes(handle, ptr, len, |kernel, bytes| {
            kernel.input_entropy(bytes)
        })
    }

    /// §4.3 `sipx_input_timer`: a previously requested timer fired.
    pub fn input_timer(&mut self, handle: i32, timer_id: u64, now_ms: u64) -> i32 {
        let Some(slot) = self.slots.get_mut(&handle) else {
            return Error::InvalidHandle.code();
        };
        slot.borrowed.clear();
        if slot.kernel.is_poisoned() {
            return Error::Poisoned.code();
        }
        match slot.kernel.input_timer(timer_id, now_ms) {
            Ok(()) => 0,
            Err(error) => {
                slot.kernel.count_rejection(error);
                error.code()
            }
        }
    }

    /// §4.3 `sipx_next_output`: the packed buffer of the next output record; `0` when drained.
    pub fn next_output(&mut self, handle: i32) -> u64 {
        let Some(slot) = self.slots.get_mut(&handle) else {
            return pack_error(Error::InvalidHandle);
        };
        // Draining stays legal on a poisoned instance so the host can retrieve the fatal
        // `"error"` event (§4.9).
        let Some(record) = slot.kernel.next_output() else {
            slot.borrowed.clear();
            return 0;
        };
        let bytes = record.encode();
        Self::lend(slot, bytes, &mut self.next_key)
    }

    /// §4.3 `sipx_snapshot`: the packed buffer of a read-only JSON state and counter snapshot.
    pub fn snapshot(&mut self, handle: i32) -> u64 {
        let Some(slot) = self.slots.get_mut(&handle) else {
            return pack_error(Error::InvalidHandle);
        };
        let bytes = slot.kernel.snapshot();
        Self::lend(slot, bytes, &mut self.next_key)
    }

    /// The bytes of the buffer this handle currently lends the host.
    ///
    /// The browser reads them out of linear memory at the packed offset; a native test reads them
    /// here. Same bytes, same lifetime rule.
    #[must_use]
    pub fn borrowed(&self, handle: i32) -> &[u8] {
        self.slots
            .get(&handle)
            .map_or(&[][..], |slot| slot.borrowed.as_slice())
    }

    /// How many handles are live, for the §4.9 cap and for the create/free memory proof.
    #[must_use]
    pub fn live_handles(&self) -> usize {
        self.slots.len()
    }

    /// How many host allocations are outstanding.
    ///
    /// The create/free cycle in §4.9's last row is proved with this: after `sipx_kernel_new` and
    /// `sipx_kernel_free` in a loop, both this and [`Abi::live_handles`] must return to their
    /// baseline, or the kernel is leaking linear memory a page cannot reclaim.
    #[must_use]
    pub fn live_allocations(&self) -> usize {
        self.allocations.len()
    }

    // ---------------------------------------------------------------- internals

    /// Run one byte-taking entry point under §4.4's ownership and §4.9's poisoning rules.
    fn with_bytes(
        &mut self,
        handle: i32,
        ptr: u32,
        len: u32,
        run: impl FnOnce(&mut Kernel, &[u8]) -> Result<(), Error>,
    ) -> i32 {
        let bytes = match self.read(ptr, len) {
            Ok(bytes) => bytes,
            // The handle is checked first so a bad handle and a bad pointer cannot be confused:
            // `BSDK-NEG-1` and `BSDK-NEG-2` are different rows.
            Err(error) => {
                if !self.slots.contains_key(&handle) {
                    return Error::InvalidHandle.code();
                }
                if let Some(slot) = self.slots.get_mut(&handle) {
                    slot.kernel.count_rejection(error);
                }
                return error.code();
            }
        };
        let Some(slot) = self.slots.get_mut(&handle) else {
            return Error::InvalidHandle.code();
        };
        // §4.4: the borrow ends at the next call of any export on this handle.
        slot.borrowed.clear();
        if slot.kernel.is_poisoned() {
            return Error::Poisoned.code();
        }
        match run(&mut slot.kernel, &bytes) {
            Ok(()) => 0,
            Err(error) => {
                slot.kernel.count_rejection(error);
                error.code()
            }
        }
    }

    /// Publish a kernel-owned buffer to the host and pack its offset and length.
    fn lend(slot: &mut Slot, bytes: Vec<u8>, next_key: &mut u32) -> u64 {
        let len = u32::try_from(bytes.len()).unwrap_or(u32::MAX);
        slot.borrowed = bytes;
        slot.borrowed_key = pointer_of(&slot.borrowed).unwrap_or_else(|| {
            let key = *next_key;
            *next_key = next_key.saturating_add(1);
            key
        });
        (u64::from(slot.borrowed_key) << 32) | u64::from(len)
    }

    /// The table key for a buffer: its linear-memory offset where one fits in a `u32`.
    fn key_for(&mut self, bytes: &[u8]) -> u32 {
        pointer_of(bytes).unwrap_or_else(|| {
            let key = self.next_key;
            self.next_key = self.next_key.saturating_add(1);
            key
        })
    }
}

/// A buffer's address, when the target's pointers fit in the ABI's `u32`.
///
/// `wasm32-unknown-unknown` is such a target, and there the answer is the exported memory's
/// offset — the number the host writes at. On a 64-bit host it is `None`, and the caller uses a
/// synthetic key instead; nothing in the ABI dereferences either.
fn pointer_of(bytes: &[u8]) -> Option<u32> {
    if usize::BITS > 32 {
        return None;
    }
    u32::try_from(bytes.as_ptr() as usize)
        .ok()
        .filter(|key| *key != 0)
}

/// §4.2's packed-buffer error form: pointer `0` with the error code's magnitude in the length.
fn pack_error(error: Error) -> u64 {
    u64::from(error.magnitude())
}

/// Split a packed buffer into its offset and length halves.
#[must_use]
pub fn unpack(packed: u64) -> (u32, u32) {
    (
        u32::try_from(packed >> 32).unwrap_or(0),
        u32::try_from(packed & 0xffff_ffff).unwrap_or(0),
    )
}
