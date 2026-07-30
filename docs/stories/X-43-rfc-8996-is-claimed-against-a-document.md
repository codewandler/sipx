---
id: X-43
title: Evidence RFC 8996 with a refusal, not with a document
pillar: Build
status: ready
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
- [ ] **A TLS 1.0 or 1.1 handshake against a sipx listener is refused, and a test asserts it.**
      `docs/rfc/registry.toml`'s RFC 8996 row is `status = "implemented"` with
      `evidence = ["docs/specs/sip-tls.md"]` — our own prose. Every other `implemented` row in the
      registry cites code. RFC 8996 is a **negative** obligation (do not offer these versions), and the
      only evidence that can fail is an attempt: drive a handshake at a deprecated version and require
      it to be rejected.
- [ ] **The row cites the refusal.** Evidence becomes the code that pins the minimum version plus the
      test that proves the refusal, and `docs/compliance.md` is regenerated in the same commit.
- [ ] **The claim's real basis is stated where it is made.** sipx does not implement TLS itself; the
      property holds because the TLS implementation it uses offers no version below 1.2. That is a
      *dependency* property, not sipx behaviour, and the note should say so — otherwise a future change
      of TLS backend silently moves the claim without touching the row.
- [ ] **The registry rule is considered, not just this row.** `rfc-report.py` requires an entry claiming
      implementation to cite *something*, and a document satisfies it. Decide whether `implemented`
      should require at least one `crates/` path, and say why or why not — this row is the only
      instance today, so the rule change is cheap now and the story is the moment to weigh it. If the
      answer is no, record the reason, because the next such row will look like this one.
- [ ] Failing-first test: the handshake test, which cannot exist before the fix, is the failing-first
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
