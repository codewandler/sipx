---
id: S-12
title: Implement reliable provisional responses
pillar: Signalling
status: done
priority:
design:
epic: conformance
areas: [sipx-sip, sipx-call]
note: track: signalling · RFC 3262 · same files as S-11, take it second
---

# Implement reliable provisional responses

## Goal
100rel and PRACK, which some carriers require before they will accept a call at all.

## Acceptance
- [x] `100rel` is offered in `Supported` and honoured when the far end puts it in `Require`.
- [x] A reliable provisional carries `RSeq` and is retransmitted until PRACK acknowledges it.
- [x] PRACK carries the matching `RAck` and is answered.
- [x] A far end that requires `100rel` when sipx has it disabled is refused with 420 naming the
      option tag, rather than left to time out.
- [x] An offer in a reliable provisional is answered in the PRACK (RFC 3262 §5).
- [x] Failing-first test: `a_reliable_provisional_is_retransmitted_until_pracked`.

## Progress
- Done. Split the way the rest of the stack is: `sipx-sip/src/rel.rs` is the whole decision
  procedure with no clock — who may send a reliable provisional, the `RSeq` allocation window, the
  `RAck` match, and the UAC's in-order/duplicate/gap classification — and `sipx-call/src/rel.rs`
  is the half that needs one.
- **The retransmission schedule deliberately does not cap at T2.** Every other retransmission in
  SIP does. §3 gives the reason: an ACK is resent because a 2xx arrived again, but a PRACK is sent
  once and is not re-triggered by a further 1xx, so doubling past T2 costs nothing and repeating
  faster buys nothing. Doubling from T1 to 64*T1 is 6 sends, not 60.
- **The `To` tag is chosen by `ring` and reused by the answer.** A reliable provisional establishes
  a dialog (§4 via RFC 3261 §12.1.1), so a fresh tag on the eventual 200 would create a *second*
  one: the caller ACKs the dialog it knows and this side retransmits the 200 for 32 seconds into a
  call that is already up. `Ringing::tag()` exists so `answer_ringing` cannot get this wrong.
- **`RSeq` is uniform in 1..2^31-1, not sequential.** §3 recommends it; the reason it matters is
  that the number is the only thing an off-path attacker would need to forge a PRACK and silence
  the retransmissions.
- The failing-first test drives the answering side from **raw messages** rather than through
  `dial`. sipx's own caller PRACKs the first provisional immediately, which is correct, and would
  hide the retransmission the test exists to observe.
- `100 Trying` is excluded from PRACK by status rather than by the call site: §4 makes a `100`
  hop-by-hop, so a proxy's `Require: 100rel` on one "MUST be ignored".
- Mutation-tested: removing the retransmission loop, dropping the 420, making `acknowledge`
  ignore the `CSeq`, and making a duplicate look in-order each fail the test that names the
  behaviour.
- Not in scope and now the visible gap next to it: **UPDATE (RFC 3311)**, which is how a session
  is renegotiated before the call is answered. 100rel is the half that makes an early offer
  answerable at all.
