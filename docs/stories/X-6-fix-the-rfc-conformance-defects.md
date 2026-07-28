---
id: X-6
title: Fix the RFC conformance defects found by review
pillar: Core
status: done
priority:
design:
epic:
areas: [sipx-sip, sipx-transport, sipx-call, sipx-sdp, sipx-rtp, sipx-media, sipx-ua, sipx-cli]
note: review of implemented behaviour, not of gaps
---

# Fix the RFC conformance defects found by review

## Goal
Audit what the stack *does* against the RFCs it claims to implement, and fix every place the
implemented behaviour diverges. Deliberately not a gap analysis: unimplemented features were
out of scope, because a feature that is absent is visible and a feature that is subtly wrong is
not.

## Acceptance
- [x] Every layer reviewed against its normative reference — RFC 3261 throughout, plus RFC 3263
      resolution, RFC 3581 rport, RFC 5922 TLS identity, RFC 3264/8866 offer-answer, RFC
      3550/3551/4733 media, and RFC 7616 digest.
- [x] Each defect fixed with a failing-first test, and each test confirmed to fail for the
      stated reason before the fix.
- [x] Tests that asserted the non-conforming behaviour rewritten to assert what the RFC
      requires, rather than deleted.
- [x] The gate stays green: `cargo test --workspace --all-features`, clippy at `-D warnings`,
      `cargo fmt --check`, `check-provenance.sh`, `check-features.sh`.

## Progress

Done. Roughly fifty defects across every crate. The ones that would have been felt first:

- **Timer B fired from `Proceeding`**, so a callee who took longer than 64·T1 to answer was hung
  up on — and `send_to_uri` then dialled the next RFC 3263 candidate while the first phone was
  still ringing. §17.1.1.2 fires it from `Calling` only; §16.6 item 11 is explicit that the
  INVITE client transaction has no timeout thereafter, which is the reason proxies need Timer C.
- **`sips:` with a transport parameter resolved to cleartext** on the three paths that never
  reach the SRV stage where the scheme filter lives. Table 1 and §26.2.2: the parameter names
  the transport carried *under* TLS. `sips` over UDP now yields no candidate at all rather than
  a downgrade, there being no TLS over UDP to offer.
- **RFC 3581 was broken in both halves.** `received` was omitted when the sent-by matched the
  source, which §4 requires "even if it is identical"; and `rport` was consulted only alongside
  `received`, so responses went to the sent-by port that a NAT had rewritten.
- **In-dialog requests carried the route set but were addressed to the remote target**, bypassing
  the record-routing proxy that put itself in the dialog precisely to be traversed. That is the
  BYE that never arrives, with the media still running. §12.2.1.1, including strict routing and
  the parameters §19.1.1 bars from a Request-URI.
- **The ACK to a 2xx ran in a transaction**, earning it Timer E retransmissions toward a response
  that never comes; and a *retransmitted* 2xx was never acknowledged again, though §13.2.2.4
  requires an ACK for each one.
- **RTCP named both parties SSRC 0**, and interarrival jitter used non-modular arithmetic, so a
  32-bit timestamp wrap — normal, since §5.1 randomises the starting timestamp — injected
  2³²/16 into the estimate and poisoned it for hundreds of packets.
- **A CRLF before a start-line was a fatal framing error**, so the RFC 5626 keepalives that
  mainstream stacks send routinely closed the connection and every dialog riding it (§7.5).

Two places where the first fix was not what the RFC actually says, corrected before landing: the
strict-route Request-URI now strips the `method` parameter and header component per §12.2.1.1,
and the CANCEL path *waits* for a provisional as §9.1 instructs rather than abandoning the
cancellation.

One in-dialog ordering call worth recording: §12.2.2 literally rejects only a sequence number
*lower* than the dialog's, but §12.2.1.1 requires each new in-dialog request to increment, so a
repeat of the current number is a duplicate rather than a fresh request and is refused on the
same grounds. Accepting it let a stale BYE end a running call.

Review and fixes were split across the layers and run in parallel; the findings that turned out
to be already fixed in flight — the `sips:` certificate identity, most of one WebSocket
finding — were verified against the tree rather than taken on trust.
