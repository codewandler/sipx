//! `docs/specs/browser-sdk.md` §8.1: the RFC 4475 and RFC 5118 torture corpora, replayed through
//! `sipx_input_bytes`.
//!
//! > Every byte entering `sipx_input_bytes` is attacker-controlled … in WASM a panic is a trap, so
//! > the no-panic rule is also an availability rule … `S-41` MUST run the existing RFC 4475/5118
//! > torture corpora against the WASM build with native-identical outcomes.
//!
//! "Native-identical" is asserted two ways. Structurally: every case must return `0` or `E_BOUNDS`,
//! must never poison the kernel, and must never invent a call. And by digest: the whole replay is
//! reduced to one SHA-256 over every case's return code, counter movement and emitted records, and
//! that digest is pinned. The same test binary compiled for `wasm32-wasip1` must produce the same
//! number, which is what makes the claim a comparison rather than two separate assertions.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

mod support;

use sha2::{Digest as _, Sha256};
use support::{Host, Out, tape};

mod corpus {
    include!(concat!(env!("OUT_DIR"), "/corpus.rs"));
}

/// The corpora are recovered from their RFCs by importers whose `--check` is a gate step; if this
/// number moves, an importer moved it, and the digest below has to be re-derived from the new
/// corpus rather than adjusted to match.
#[test]
fn the_corpora_are_present_and_non_trivial() {
    // `const_assert`-shaped rather than a runtime one, because `CASE_COUNT` is a constant: the
    // check is that the build script found the corpora at all, and a build that embedded nothing
    // would fail here rather than let every case below vacuously pass.
    let embedded = corpus::CASE_COUNT;
    assert!(embedded >= 60, "only {embedded} corpus cases were embedded");
    for (name, bytes) in corpus::CASES {
        assert!(!bytes.is_empty(), "{name} is empty");
    }
}

/// Every torture case is a value, not a fault: the entry point returns, the kernel survives, and
/// nothing invents a call.
#[test]
fn no_torture_case_can_fault_the_kernel() {
    for (name, bytes) in corpus::CASES {
        let mut host = Host::new();
        host.entropy(&tape(0x55));
        host.clear_log();

        let code = host.receive_bytes(bytes);
        assert!(
            code == 0 || code == sipx_wasm::Error::Bounds.code(),
            "{name}: hostile input must be a value or a bounds refusal, got {code}"
        );

        let snapshot = host.snapshot();
        assert!(
            snapshot.contains(r#""poisoned":false"#),
            "{name} poisoned the kernel: {snapshot}"
        );
        assert!(
            snapshot.contains(r#""calls":{}"#),
            "{name} invented a call: {snapshot}"
        );
        assert!(
            !snapshot.contains("secret"),
            "{name} put the credential in a snapshot: {snapshot}"
        );
        for event in host.events() {
            assert!(
                !event.contains(r#""fatal":true"#),
                "{name} produced a fatal event: {event}"
            );
        }
    }
}

/// The replay reduced to one number, so a native run and a WebAssembly run can be compared rather
/// than merely both passing.
///
/// If this fails after a deliberate change to the corpus or to the parser, re-derive the digest by
/// printing `replay_digest()` — do not edit it to match a run whose behaviour has not been read.
#[test]
fn the_corpus_replay_digest_is_stable_across_targets() {
    let digest = replay_digest();
    assert_eq!(
        digest,
        EXPECTED_REPLAY_DIGEST,
        "the corpus replay changed; {} cases",
        corpus::CASE_COUNT
    );
}

/// The digest of the whole replay, on `x86_64-unknown-linux-gnu` and `wasm32-wasip1`.
///
/// Re-derived once since it was first pinned, by `S-53`, for the one case below and no other:
/// 61 of the 62 cases replay byte for byte as they did.
const EXPECTED_REPLAY_DIGEST: &str =
    "32c86355a32cde3a180540a0716fd2d48b783fec3d4aa2e4e69c71077a13a42e";

/// `rfc4475/dblreq.dat` is a **datagram** rule, and this entry point is not a datagram.
///
/// RFC 4475 §3.1.1.8 puts a complete second request after the first in one UDP datagram and says
/// a parser must take the first and ignore the rest — which RFC 3261 §18.3 licenses, because a
/// datagram carries at most one message and the remainder is noise. `sipx_input_bytes` is fed a
/// WebSocket message, where RFC 7118 §5 and `docs/specs/sip-tls.md` §4 say the opposite: the frame
/// boundary *is* the message boundary, so the same octets mean the peer disagrees about where
/// messages end.
///
/// This is the single case whose replay `S-53` moved, and it is the only reason the digest above
/// was re-derived. Before, the kernel answered the embedded `REGISTER` with a `405` and set that
/// transaction's timer while the second request vanished uncounted; now the frame is refused whole
/// and the host is told the only way §4.11 allows.
#[test]
fn the_one_datagram_case_that_a_frame_refuses() {
    let bytes = corpus::CASES
        .iter()
        .find(|(name, _)| *name == "rfc4475/dblreq.dat")
        .expect("RFC 4475 §3.1.1.8 is in the corpus")
        .1;

    let mut host = Host::new();
    host.entropy(&tape(0x55));
    host.clear_log();

    assert_eq!(host.receive_bytes(bytes), 0, "a value, not a refusal");
    assert!(
        host.log.is_empty(),
        "the first request must not be answered: {:?}",
        host.log
    );
    let snapshot = host.snapshot();
    assert!(
        snapshot.contains(r#""parse_errors":1"#),
        "counted, which is what closes the connection (browser-signalling.md §5): {snapshot}"
    );
    assert!(
        snapshot.contains(r#""pendingTimers":0"#),
        "and no transaction outlived it: {snapshot}"
    );
}

fn replay_digest() -> String {
    let mut hasher = Sha256::new();
    // `u64`, not `usize`: `usize` is four octets on `wasm32` and eight on the native host, so
    // hashing it directly would make the two targets differ by construction and turn this test
    // into a tautology in reverse.
    hasher.update((corpus::CASE_COUNT as u64).to_le_bytes());
    for (name, bytes) in corpus::CASES {
        let mut host = Host::new();
        host.entropy(&tape(0x55));
        host.clear_log();

        let code = host.receive_bytes(bytes);
        hasher.update(name.as_bytes());
        hasher.update(code.to_le_bytes());
        for record in &host.log {
            match record {
                Out::Wire(text) => {
                    hasher.update([1u8]);
                    hasher.update(text.as_bytes());
                }
                Out::TimerSet { id, fire_at_ms } => {
                    hasher.update([2u8]);
                    hasher.update(id.to_le_bytes());
                    hasher.update(fire_at_ms.to_le_bytes());
                }
                Out::TimerCancel(id) => {
                    hasher.update([3u8]);
                    hasher.update(id.to_le_bytes());
                }
                Out::Event(text) => {
                    hasher.update([4u8]);
                    hasher.update(text.as_bytes());
                }
            }
        }
        hasher.update(host.snapshot().as_bytes());
    }
    format!("{:x}", hasher.finalize())
}

/// A corpus case is not a licence to skip the bound: a 64 KiB + 1 torture message is refused by
/// §4.9 before the parser sees it, on every target.
#[test]
fn the_message_bound_applies_to_corpus_shaped_input() {
    let mut host = Host::new();
    host.entropy(&tape(0x55));
    let mut oversize = corpus::CASES[0].1.to_vec();
    oversize.resize(64 * 1024 + 1, b'x');
    assert_eq!(
        host.receive_bytes(&oversize),
        sipx_wasm::Error::Bounds.code()
    );
}
