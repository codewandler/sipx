//! A host that drives the ABI exactly as `docs/specs/browser-sdk.md` §4 says a host must.
//!
//! Every §9 vector runs through this and nothing else. It allocates with `sipx_alloc`, writes,
//! calls the entry point, frees, and then drains `sipx_next_output` until it returns `0` — the
//! §4.6 obligation — decoding each record from its framing rather than from a helper, so the
//! framing itself is under test.
//!
//! There is deliberately no second path into the kernel. That is what makes "native and WASM
//! produce identical events and wire bytes" a statement about one harness on two targets.

// A shared harness compiled into several test binaries: each one uses a different part of it,
// and everything here is `pub` for the test crate that includes it rather than for a consumer.
#![allow(
    dead_code,
    unreachable_pub,
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use std::fmt::Write as _;

use sipx_wasm::{Abi, unpack};

/// `BSDK-CFG-1`, §9.2, byte for byte.
pub const BSDK_CFG_1: &[u8] = br#"{"v":1,"aor":"sip:alice@example.net","auth":{"username":"alice","password":"secret"},"transport":{"scheme":"wss","host":"edge.example.net","resource":"/sip"},"insecure":"refuse"}"#;

/// `BSDK-CMD-1`, `BSDK-CMD-2`, `BSDK-CMD-3`, §9.2, byte for byte.
pub const BSDK_CMD_1: &[u8] = br#"{"v":1,"cmd":"register","id":1,"expires":600}"#;
pub const BSDK_CMD_2: &[u8] = br#"{"v":1,"cmd":"dial","id":2,"target":"sip:bob@example.net"}"#;
pub const BSDK_CMD_3: &[u8] = br#"{"v":1,"cmd":"hangup","id":3,"call":1}"#;

/// `BSDK-EVT-1`, `BSDK-EVT-2`, `BSDK-EVT-3`, §9.2, byte for byte.
pub const BSDK_EVT_1: &[u8] = br#"{"v":1,"evt":"need-entropy","min":64}"#;
pub const BSDK_EVT_2: &[u8] = br#"{"v":1,"evt":"registration","state":"registered","expires":600}"#;
pub const BSDK_EVT_3: &[u8] = br#"{"v":1,"evt":"need-local-media","call":1,"kind":"offer","constraints":{"audio":true,"video":false}}"#;

/// `BA-SDP-O1` from `docs/specs/webrtc-audio.md` §9.2 — a complete browser-audio offer.
pub const BA_SDP_O1: &str = "v=0\r\n\
o=- 496232 1 IN IP4 192.0.2.10\r\n\
s=-\r\n\
t=0 0\r\n\
a=ice-options:ice2\r\n\
m=audio 49170 UDP/TLS/RTP/SAVPF 111 0 8 13 101\r\n\
c=IN IP4 192.0.2.10\r\n\
a=sendrecv\r\n\
a=rtcp-mux\r\n\
a=ice-ufrag:ofr1\r\n\
a=ice-pwd:offerPassword0123456789AB\r\n\
a=candidate:1 1 UDP 2130706431 192.0.2.10 49170 typ host\r\n\
a=fingerprint:sha-256 00:01:02:03:04:05:06:07:08:09:0A:0B:0C:0D:0E:0F:10:11:12:13:14:15:16:17:18:19:1A:1B:1C:1D:1E:1F\r\n\
a=setup:actpass\r\n\
a=rtpmap:111 opus/48000/2\r\n\
a=rtpmap:0 PCMU/8000\r\n\
a=rtpmap:8 PCMA/8000\r\n\
a=rtpmap:13 CN/8000\r\n\
a=rtpmap:101 telephone-event/8000\r\n\
a=fmtp:101 0-16\r\n";

/// `BA-SDP-A1` from `docs/specs/webrtc-audio.md` §9.3 — the complementary answer.
pub const BA_SDP_A1: &str = "v=0\r\n\
o=- 772211 1 IN IP4 198.51.100.20\r\n\
s=-\r\n\
t=0 0\r\n\
a=ice-options:ice2\r\n\
m=audio 53000 UDP/TLS/RTP/SAVPF 111 0 8 13 101\r\n\
c=IN IP4 198.51.100.20\r\n\
a=sendrecv\r\n\
a=rtcp-mux\r\n\
a=ice-ufrag:ans1\r\n\
a=ice-pwd:answerPassword0123456789A\r\n\
a=candidate:1 1 UDP 2130706431 198.51.100.20 53000 typ host\r\n\
a=fingerprint:sha-256 20:21:22:23:24:25:26:27:28:29:2A:2B:2C:2D:2E:2F:30:31:32:33:34:35:36:37:38:39:3A:3B:3C:3D:3E:3F\r\n\
a=setup:active\r\n\
a=rtpmap:111 opus/48000/2\r\n\
a=rtpmap:0 PCMU/8000\r\n\
a=rtpmap:8 PCMA/8000\r\n\
a=rtpmap:13 CN/8000\r\n\
a=rtpmap:101 telephone-event/8000\r\n\
a=fmtp:101 0-16\r\n";

/// A buffer's length as the ABI's `u32`.
///
/// Every §4.9 input bound is far below `u32::MAX`, so this cannot fail for anything a test hands
/// the ABI; it panics rather than truncating because a silently shortened length would make a
/// negative vector pass for the wrong reason.
pub fn len(bytes: &[u8]) -> u32 {
    u32::try_from(bytes.len()).expect("no test buffer approaches four gigabytes")
}

/// One drained output record, decoded from §4.6's framing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Out {
    Wire(String),
    TimerSet { id: u64, fire_at_ms: u64 },
    TimerCancel(u64),
    Event(String),
}

impl Out {
    pub fn as_wire(&self) -> Option<&str> {
        match self {
            Self::Wire(text) => Some(text),
            _ => None,
        }
    }

    pub fn as_event(&self) -> Option<&str> {
        match self {
            Self::Event(text) => Some(text),
            _ => None,
        }
    }
}

/// The host half of the contract.
pub struct Host {
    abi: Abi,
    pub handle: i32,
    now_ms: u64,
    /// Everything drained since the harness started, in emission order.
    pub log: Vec<Out>,
}

impl Host {
    /// A kernel from `BSDK-CFG-1`.
    pub fn new() -> Self {
        Self::with_config(BSDK_CFG_1)
    }

    pub fn with_config(config: &[u8]) -> Self {
        let mut abi = Abi::new();
        let ptr = abi.alloc_with(config);
        assert_ne!(ptr, 0, "sipx_alloc refused the configuration buffer");
        let handle = abi.kernel_new(ptr, len(config));
        // §4.4: when the entry point returns the buffer belongs to the host again.
        abi.free(ptr, len(config));
        assert!(handle > 0, "sipx_kernel_new returned {handle}");
        Self {
            abi,
            handle,
            now_ms: 0,
            log: Vec::new(),
        }
    }

    pub fn abi(&mut self) -> &mut Abi {
        &mut self.abi
    }

    /// Call an entry point with a raw pointer and length, bypassing the allocation table.
    ///
    /// This is how §9.5's pointer negatives are expressed: a host that hands the ABI an offset
    /// it never obtained from `sipx_alloc`.
    pub fn raw_entropy(&mut self, ptr: u32, len: u32) -> i32 {
        let handle = self.handle;
        self.abi.input_entropy(handle, ptr, len)
    }

    pub fn raw_command(&mut self, ptr: u32, len: u32) -> i32 {
        let handle = self.handle;
        let now = self.now_ms;
        self.abi.command(handle, ptr, len, now)
    }

    /// `sipx_command` with a chosen `now_ms`, for the §4.5 monotonicity negative.
    pub fn command_at(&mut self, document: &[u8], now_ms: u64) -> i32 {
        let ptr = self.abi.alloc_with(document);
        let handle = self.handle;
        let code = self.abi.command(handle, ptr, len(document), now_ms);
        self.abi.free(ptr, len(document));
        self.drain();
        code
    }

    pub fn now(&self) -> u64 {
        self.now_ms
    }

    /// Advance the host's monotonic clock. `now_ms` must be non-decreasing per handle (§4.5).
    pub fn tick(&mut self, by: u64) -> &mut Self {
        self.now_ms += by;
        self
    }

    /// `sipx_input_entropy`, then drain.
    pub fn entropy(&mut self, bytes: &[u8]) -> i32 {
        let ptr = self.abi.alloc_with(bytes);
        let code = self.abi.input_entropy(self.handle, ptr, len(bytes));
        self.abi.free(ptr, len(bytes));
        self.drain();
        code
    }

    /// `sipx_command`, then drain.
    pub fn command(&mut self, document: &[u8]) -> i32 {
        let ptr = self.abi.alloc_with(document);
        let code = self
            .abi
            .command(self.handle, ptr, len(document), self.now_ms);
        self.abi.free(ptr, len(document));
        self.drain();
        code
    }

    /// `sipx_input_bytes`, then drain.
    pub fn receive(&mut self, message: &str) -> i32 {
        self.receive_bytes(message.as_bytes())
    }

    pub fn receive_bytes(&mut self, message: &[u8]) -> i32 {
        let ptr = self.abi.alloc_with(message);
        let code = self
            .abi
            .input_bytes(self.handle, ptr, len(message), self.now_ms);
        self.abi.free(ptr, len(message));
        self.drain();
        code
    }

    /// `sipx_input_timer`, then drain.
    pub fn fire(&mut self, timer_id: u64) -> i32 {
        let code = self.abi.input_timer(self.handle, timer_id, self.now_ms);
        self.drain();
        code
    }

    /// `sipx_snapshot`, copied out before the borrow ends (§4.4).
    pub fn snapshot(&mut self) -> String {
        let packed = self.abi.snapshot(self.handle);
        let (_, len) = unpack(packed);
        let bytes = self.abi.borrowed(self.handle).to_vec();
        assert_eq!(bytes.len(), len as usize, "the packed length must match");
        String::from_utf8(bytes).expect("the snapshot is UTF-8 JSON")
    }

    /// The §4.6 drain obligation: loop until `sipx_next_output` returns `0`.
    pub fn drain(&mut self) -> Vec<Out> {
        let mut taken = Vec::new();
        loop {
            let packed = self.abi.next_output(self.handle);
            if packed == 0 {
                break;
            }
            let (ptr, len) = unpack(packed);
            assert_ne!(ptr, 0, "a real record never uses the error encoding");
            let framed = self.abi.borrowed(self.handle).to_vec();
            assert_eq!(framed.len(), len as usize, "packed length must match");
            taken.push(decode(&framed));
        }
        self.log.extend(taken.iter().cloned());
        taken
    }

    /// Every SIP message the kernel has emitted, as text.
    pub fn wires(&self) -> Vec<&str> {
        self.log.iter().filter_map(Out::as_wire).collect()
    }

    /// Every event document the kernel has emitted, as text.
    pub fn events(&self) -> Vec<&str> {
        self.log.iter().filter_map(Out::as_event).collect()
    }

    /// The event documents whose `"evt"` is `kind`.
    pub fn events_of(&self, kind: &str) -> Vec<&str> {
        let needle = format!(r#""evt":"{kind}""#);
        self.events()
            .into_iter()
            .filter(|event| event.contains(&needle))
            .collect()
    }

    /// The index in the whole log of the first record satisfying `predicate`.
    pub fn position(&self, predicate: impl Fn(&Out) -> bool) -> Option<usize> {
        self.log.iter().position(predicate)
    }

    /// The id of the first `TIMER_SET` whose deadline is at least `after_ms` from now.
    pub fn timer_at_least(&self, after_ms: u64) -> Option<u64> {
        self.log.iter().find_map(|record| match record {
            Out::TimerSet { id, fire_at_ms } if *fire_at_ms >= after_ms => Some(*id),
            _ => None,
        })
    }

    pub fn clear_log(&mut self) {
        self.log.clear();
    }
}

/// Decode §4.6's framing: `u32` type, `u32` length, then the payload.
pub fn decode(framed: &[u8]) -> Out {
    assert!(framed.len() >= 8, "a record carries an eight-octet header");
    let tag = u32::from_le_bytes([framed[0], framed[1], framed[2], framed[3]]);
    let len = u32::from_le_bytes([framed[4], framed[5], framed[6], framed[7]]) as usize;
    let payload = &framed[8..];
    assert_eq!(
        payload.len(),
        len,
        "the length field must describe the payload"
    );
    match tag {
        1 => Out::Wire(String::from_utf8_lossy(payload).into_owned()),
        2 => {
            assert_eq!(len, 16, "TIMER_SET carries two u64s");
            Out::TimerSet {
                id: u64::from_le_bytes(payload[..8].try_into().unwrap()),
                fire_at_ms: u64::from_le_bytes(payload[8..].try_into().unwrap()),
            }
        }
        3 => {
            assert_eq!(len, 8, "TIMER_CANCEL carries one u64");
            Out::TimerCancel(u64::from_le_bytes(payload[..8].try_into().unwrap()))
        }
        4 => Out::Event(String::from_utf8(payload.to_vec()).expect("events are UTF-8")),
        other => panic!("unknown record type {other}"),
    }
}

/// A 256-octet tape, enough for any single vector's draws, starting at `first`.
pub fn tape(first: u8) -> Vec<u8> {
    (0..256u32)
        .map(|index| first.wrapping_add(u8::try_from(index % 256).unwrap_or(0)))
        .collect()
}

/// The 32-octet tape `00 01 02 … 1f` that `BSDK-ENT-1` pins.
pub fn ent_1_tape() -> Vec<u8> {
    (0u8..32).collect()
}

/// Take the header value out of a rendered SIP message.
pub fn header(message: &str, name: &str) -> Option<String> {
    let prefix = format!("{name}: ");
    message
        .split("\r\n")
        .find(|line| line.starts_with(&prefix))
        .map(|line| line[prefix.len()..].to_owned())
}

/// A response to a request the kernel sent, echoing the headers RFC 3261 §8.2.6 requires.
pub fn respond_to(request: &str, status_line: &str, extra: &[&str], body: Option<&str>) -> String {
    let mut out = format!("SIP/2.0 {status_line}\r\n");
    for name in ["Via", "From", "To", "Call-ID", "CSeq"] {
        if let Some(value) = header(request, name) {
            let _ = write!(out, "{name}: {value}\r\n");
        }
    }
    for line in extra {
        out.push_str(line);
        out.push_str("\r\n");
    }
    match body {
        Some(body) => {
            out.push_str("Content-Type: application/sdp\r\n");
            let _ = write!(out, "Content-Length: {}\r\n\r\n", body.len());
            out.push_str(body);
        }
        None => out.push_str("Content-Length: 0\r\n\r\n"),
    }
    out
}
