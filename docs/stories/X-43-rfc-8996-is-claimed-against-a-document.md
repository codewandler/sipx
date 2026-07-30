---
id: X-43
title: Evidence RFC 8996 with a refusal, not with a document
pillar: Build
status: in-progress
priority: 5
design: docs/designs/rfc-registry-grain.md
epic: conformance
areas: [docs, sipx-transport]
note: the only `implemented` row of 70 whose evidence cites no code — `evidence = ["docs/specs/sip-tls.md"]` — and it is a negative claim, so the only thing that can back it is a handshake that fails
---

# Evidence RFC 8996 with a refusal, not with a document

## Goal
Make the claim that sipx deprecates TLS 1.0 and 1.1 rest on an observed refusal rather than on a
sentence in our own spec.

## Acceptance
- [x] **A TLS 1.0 or 1.1 handshake against a sipx listener is refused, and a test asserts it.**
      `docs/rfc/registry.toml`'s RFC 8996 row is `status = "implemented"` with
      `evidence = ["docs/specs/sip-tls.md"]` — our own prose. Every other `implemented` row in the
      registry cites code. RFC 8996 is a **negative** obligation (do not offer these versions), and the
      only evidence that can fail is an attempt: drive a handshake at a deprecated version and require
      it to be rejected.
- [x] **The row cites the refusal.** Evidence becomes the code that pins the minimum version plus the
      test that proves the refusal, and `docs/compliance.md` is regenerated in the same commit.
- [x] **The claim's real basis is stated where it is made.** sipx does not implement TLS itself; the
      property holds because the TLS implementation it uses offers no version below 1.2. That is a
      *dependency* property, not sipx behaviour, and the note should say so — otherwise a future change
      of TLS backend silently moves the claim without touching the row.
- [x] **The registry rule is considered, not just this row.** `rfc-report.py` requires an entry claiming
      implementation to cite *something*, and a document satisfies it. Decide whether `implemented`
      should require at least one `crates/` path, and say why or why not — this row is the only
      instance today, so the rule change is cheap now and the story is the moment to weigh it. If the
      answer is no, record the reason, because the next such row will look like this one.
- [x] Failing-first test: the handshake test, which cannot exist before the fix, is the failing-first
      evidence. State what it does at the merge base — and note whether it *passes* there, since the
      refusal may already be true and merely unevidenced. If it passes immediately, say so plainly:
      this is then an evidence defect and not a conformance one, which is a materially different report.

## Notes
- **Found by auditing all 70 rows** rather than by the gate, which is green: `rfc-report.py --check`
  reports "70 RFCs, every claim backed", and by its own rule that is true — the row cites a file that
  exists. The gap is between "cites something" and "cites something that could fail".
- **The 14 sibling rows are fine, and that is worth recording** so nobody re-derives it. Fourteen
  `implemented` rows cite no path containing `test`, which looks like the same defect and is not: Rust
  keeps unit tests in an inline `#[cfg(test)]` module in the file under test, so for example RFC 3264's
  `crates/sipx-sdp/src/answer.rs` carries 28 `#[test]` functions in one such block. A check that
  required a separate test path would fail all fourteen and be wrong about every one.
- **Why it is probably an evidence defect and not a conformance one.** The TLS backend in use offers
  nothing below 1.2, so the refusal is very likely already real. That makes this cheap — but it is
  exactly the class of claim that rots invisibly, because nothing connects the sentence to the
  behaviour, and the property belongs to a dependency rather than to us.
- Reads with `X-33` and `X-30` (which demoted rows whose evidence pointed at code nothing reached — the
  same question with the opposite symptom) and `X-38`, whose reviewer surfaced this row while checking
  whether a redefinition of `implemented` held.

## Progress

**It is an evidence defect, not a conformance one — the refusal was already real.** All four tests in
`crates/sipx-transport/tests/tls_versions.rs` passed the first time they were run, at the merge base
`36d0b3f`, with nothing else changed. That is the outcome the Acceptance allowed for and it is stated
plainly rather than dressed up: no sipx behaviour needed fixing, only the citation. The registry half
*did* fail at the base — `prose_only_claims` run against the row as it stood reports "RFC 8996 claims
implemented but every path it cites is prose".

What the test does, and why it is shaped this way:

- It writes a `ClientHello` byte by byte at a real listener (`bind` with `tls_server` set), with
  `client_version` 1.0 and 1.1 and **no** `supported_versions` extension, and requires a fatal
  `protocol_version` alert (21 / `[2, 70]`) and nothing reaching the application. `docs/specs/sip-tls.md`
  §6 vector **L9**, which had been in the spec unrun since `T-7`.
- **No dependency was added.** `ClientTls` cannot offer a deprecated version — that is the property
  under test — so a crate that drives an obsolete handshake looked necessary and is not: assembling the
  hello needs only `tokio` and byte slices.
- **The 1.2 control is the load-bearing half.** The same bytes with two changed are *accepted*, so the
  refusals are about the version and not about a hello this test built wrong. Not hypothetical: rustls
  demands `signature_algorithms` before it looks at the version at all, so the first draft was refused
  with alert 40 and would have "passed" a looser assertion.
- The assertions were mutation-checked (`PROTOCOL_VERSION` set to 71) to confirm they read real bytes
  off the socket rather than passing vacuously.

**The rule: adopted.** `prose_only_claims` in `scripts/rfc-report.py` now requires every `implemented`
or `partial` row to cite at least one `crates/….rs` path. Measured before adopting: of 32 `implemented`
and 22 `partial` rows, 8996 was the only one failing, so it costs the row that prompted it and nothing
else. Declined: requiring a path containing `test` (would reject fourteen honest rows — this story's
Notes, re-measured and confirmed: fourteen at the base, thirteen now that 8996 cites a test), holding
`syntax` to it (free today, but `known_headers` binds a syntax claim to the parser's name table, which
is stronger than a path), and requiring evidence to be code *only* (RFC 5922's spec citation is how a
reader finds the normative document). The argument is in `docs/designs/rfc-registry-grain.md` under
"X-43"; the decision is held by `ProseIsNotEvidence` in `scripts/test-rfc-report.py`.

**The dependency basis is stated in three places** — the row's note, the module doc of
`crates/sipx-transport/src/tls.rs` (the file the row cites), and
`the_library_offers_nothing_below_the_floor`, which asserts rustls's version set is `{1.3, 1.2}`. A
backend swap now fails a test instead of silently widening the claim.

**Two gate steps are red at the merge base and were not touched**: `maturity tests` and `maturity`.
`docs/maturity.md` says 15 stories closed on 2026-07-30; `git log` at `36d0b3f` counts 16, because
closing `M-31` did not regenerate the report. Reproduced with this story's own change reverted. The
report is the coordinator's to regenerate.

**Left for someone else:** `docs/specs/sip-tls.md` §3.2 lists "the minimum protocol version, at or
above the floor in §3.5" as *configurable*, and it is not — neither `ClientTls` nor `ServerTls` takes a
version. Harmless in that it can only overstate what an operator may tighten, but the spec claims an
option that does not exist. Not this story's to fix; `X-43` only widened the row's evidence.
