//! The single browser wire declaration. Kernel types/parsers/encoders and WASM exports consume
//! these macros; scripts/generate-browser.py projects the same declaration into ESM and types.

macro_rules! wire_strings {
    ($($name:ident { $($variant:ident => $wire:literal),* };)*) => { $(
        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        pub(crate) enum $name { $($variant,)* }
        impl $name {
            pub(crate) fn as_str(self) -> &'static str { match self { $(Self::$variant => $wire,)* } }
        }
    )* };
}
wire_strings! {
    MediaKind { Offer => "offer", Answer => "answer" };
    RegistrationState { Registering => "registering", Registered => "registered", Unregistered => "unregistered", Failed => "failed" };
    Direction { In => "in", Out => "out" };
    CauseClass { Local => "local", Remote => "remote", Refused => "refused", Sip => "sip", Media => "media", Timeout => "timeout" };
}

macro_rules! browser_commands {
    ($emit:ident) => { $emit! {
        Register("register") { expires: u32 => expires_value };
        Unregister("unregister") {};
        Dial("dial") { target: String => sip_uri };
        Ring("ring") { call: u32 => call_value };
        Answer("answer") { call: u32 => call_value };
        Reject("reject") { call: u32 => call_value, status: u16 => status_value };
        Hangup("hangup") { call: u32 => call_value };
        LocalMedia("local-media") { call: u32 => call_value, kind: MediaKind => kind_value, sdp: String => string_value };
        MediaApplied("media-applied") { call: u32 => call_value };
        MediaFailed("media-failed") { call: u32 => call_value, reason: String => string_value };
    } };
}

macro_rules! browser_errors {
    ($emit:ident) => {
        $emit! {
            InvalidHandle = -1 => "invalid-handle";
            BadPointer = -2 => "bad-pointer";
            Utf8 = -3 => "utf8";
            Json = -4 => "json";
            Schema = -5 => "schema";
            State = -6 => "state";
            Bounds = -7 => "bounds";
            Entropy = -8 => "entropy";
            Oom = -9 => "oom";
            Limit = -10 => "limit";
            Time = -11 => "time";
            Poisoned = -12 => "poisoned";
        }
    };
}

macro_rules! browser_objects {
    ($emit:ident) => { $emit! {
        Cause { class: CauseClass, status: Option<u64>, reason: Option<String> };
        OutcomeError { code: &'static str, reason: String };
        CodecFact { name: String, clock_rate: u32, channels: u32, payload_type: u8 };
        DialogFact { call_id: String, local_tag: String, remote_tag: String };
        MediaFacts { dialog: DialogFact, codecs: Vec<CodecFact>, selected_codec: CodecFact, fingerprint_algorithm: &'static str, answer_setup: &'static str, local_dtls_role: &'static str, rtcp_mux: bool, audio_sections: u32 };
        Constraints { audio: bool, video: bool };
    } };
}

macro_rules! browser_events {
    ($emit:ident) => { $emit! {
        NeedEntropy("need-entropy") { min: u64 } => { min: u64 = *min };
        Registration("registration") { state: RegistrationState, expires: Option<u64>, status: Option<u64>, reason: Option<String> } => { state: RegistrationState = state, expires: Option<u64> = expires, status: Option<u64> = status, reason: Option<String> = reason };
        Call("call") { call: u32, dir: Direction, state: &'static str, from: Option<String>, to: Option<String> } => { call: u32 = *call, dir: Direction = dir, state: &'static str = state, from: Option<String> = from, to: Option<String> = to };
        NeedLocalMedia("need-local-media") { call: u32, kind: MediaKind } => { call: u32 = *call, kind: MediaKind = kind, constraints: Constraints = Constraints { audio: true, video: false } };
        RemoteMedia("remote-media") { call: u32, kind: MediaKind, sdp: String } => { call: u32 = *call, kind: MediaKind = kind, sdp: String = sdp };
        NegotiatedMedia("negotiated-media") { call: u32, kernel: MediaFacts } => { call: u32 = *call, kernel: MediaFacts = kernel };
        CallEnded("call-ended") { call: u32, cause: Cause } => { call: u32 = *call, cause: Cause = cause };
        Outcome("outcome") { outcome: Outcome } => { id: u64 = outcome.id, ok: bool = outcome.error.is_none(), error: Option<OutcomeError> = outcome.error };
        Fault("error") { fatal: bool, code: &'static str, reason: String } => { fatal: bool = *fatal, code: &'static str = code, reason: String = reason };
    } };
}

/// Emit the actual browser WASM exports. There is no second ABI-signature table.
#[macro_export]
macro_rules! browser_abi {
    ($emit:ident) => { $emit! {
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
    with_abi(sipx_wasm::Error::Poisoned.code(), |abi| abi.kernel_new(cfg_ptr, cfg_len))
}

/// §4.3 `sipx_kernel_free`: cancel everything and destroy the kernel.
#[unsafe(no_mangle)]
pub extern "C" fn sipx_kernel_free(handle: i32) -> i32 {
    with_abi(sipx_wasm::Error::Poisoned.code(), |abi| abi.kernel_free(handle))
}

/// §4.3 `sipx_command`: submit one §5.2 command.
#[unsafe(no_mangle)]
pub extern "C" fn sipx_command(handle: i32, ptr: u32, len: u32, now_ms: u64) -> i32 {
    with_abi(sipx_wasm::Error::Poisoned.code(), |abi| abi.command(handle, ptr, len, now_ms))
}

/// §4.3 `sipx_input_bytes`: one received signalling message.
#[unsafe(no_mangle)]
pub extern "C" fn sipx_input_bytes(handle: i32, ptr: u32, len: u32, now_ms: u64) -> i32 {
    with_abi(sipx_wasm::Error::Poisoned.code(), |abi| abi.input_bytes(handle, ptr, len, now_ms))
}

/// §4.3 `sipx_input_timer`: a previously requested timer fired.
#[unsafe(no_mangle)]
pub extern "C" fn sipx_input_timer(handle: i32, timer_id: u64, now_ms: u64) -> i32 {
    with_abi(sipx_wasm::Error::Poisoned.code(), |abi| abi.input_timer(handle, timer_id, now_ms))
}

/// §4.3 `sipx_input_entropy`: append host entropy to the pool.
#[unsafe(no_mangle)]
pub extern "C" fn sipx_input_entropy(handle: i32, ptr: u32, len: u32) -> i32 {
    with_abi(sipx_wasm::Error::Poisoned.code(), |abi| abi.input_entropy(handle, ptr, len))
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

    } };
}
