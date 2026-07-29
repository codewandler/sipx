---
id: X-25
title: Write the media design record the ICE stories keep citing
pillar: Build
status: in-progress
priority: 6
design:
epic: ice
areas: [docs]
note: found by M-16 — six stories name docs/designs/media.md as their design, and it is a stub
---

# Write the media design record the ICE stories keep citing

## Goal
Make `docs/designs/media.md` describe the media stack that exists, so the six ICE stories that name
it as their `design:` are pointing at something worth opening.

## Acceptance
- [x] `docs/designs/media.md` covers what `M-1` … `M-20` actually built: the RTP/RTCP path and its
      jitter buffer, the codecs, symmetric-RTP address learning, DTLS-SRTP (`M-15`), the bridge and
      conference (`M-11`), playback control (`M-17`), mute (`M-18`), and where ICE now sits.
- [x] It states the decisions a reader cannot recover from the code — chiefly why the media state
      machines are sans-IO with a driver over them, which is the pattern `docs/specs/ice.md`
      assumes without arguing for.
- [x] It stops claiming to be an outline with stories "to be cut": the header currently says
      `Status: outline · Stories: _to be cut_`, and eighteen of them have been cut and delivered.
- [x] The relationship to `docs/specs/ice.md` is explicit — a design record says why, a spec says
      what, and a reader arriving from a story's `design:` field should be told which they want.

## Progress
Done. `docs/designs/media.md` is rewritten from the stub as a record of the delivered stack; no
code was touched. Gate green, 15 steps.

- **Where each Acceptance item landed.** Item 1 → "The shape", "The path a packet takes",
  "Codecs", "Addressing: symmetric RTP, and where ICE now sits", "Security: two keyings, and why
  both", "More than one call", "Control surface: playback, mute, hold". Item 2 → "Why the media
  state machines are sans-IO, with a driver over them". Item 3 → the header. Item 4 → "What this
  document is, and which one you probably want", with a routing table.
- **The sans-IO section argues the pattern rather than restating it**, which is what the item asks
  for. It inherits `sip-core.md`'s rejection of "a task per transaction" (timing untestable without
  a clock; retransmission bugs become flaky integration tests), then gives the two reasons the
  argument is *stronger* for ICE than for transactions — the state space is combinatorial rather
  than enumerable, so §5.1.2.1's ordering can only be asserted against a table; and role conflict
  (§7.3.1.1) is reachable over a socket only by a race, which is either a test that passes for the
  wrong reason or the flaky one somebody retries. Third reason, specific to this port: the check
  parser eats unauthenticated datagrams, and only a sans-IO parser can be fuzzed without a network.
- **It also says where the pattern does not apply**, because a reader arriving from `specs/ice.md`
  §2 would otherwise over-generalise: the media session is a driver with no protocol in it, the
  jitter buffer is pure but not event-shaped, Opus is stateful, and `sipx_rtp::quality::ntp_now`
  reads a wall clock on purpose (RFC 3550 §6.4.1's NTP field has no meaning as a fired timer).
- **"Eighteen" in the Acceptance is now nineteen.** `M-19` and `M-20` closed after this story was
  filed. The header names the delivered set exactly — `M-1` … `M-15` and `M-17` … `M-20` of
  `M-1` … `M-24` — rather than a count that goes stale again.
- **Five decisions are recorded as *unrecorded* rather than invented**, in a section of their own:
  the jitter buffer's crate placement, `MissedTickBehavior::Delay`, why G.722 was dropped, why
  `M-14`/`M-15` shipped without a spec, and whether a bridge reports dropped frames. Each says
  where it was looked for.
- **Three findings left for other stories** (not fixed here, all outside this story's write set):
  `sipx-audio`'s crate docs and package description claim G.722 and resampling, neither of which
  exists, and `roadmap.md`'s `media` epic repeats the G.722 claim; `docs/specs/ice.md` §15 still
  carries `M-16`'s "Open, and blocking the codec" question that `M-20` answered and said could be
  struck; and SRTP/DTLS-SRTP have no spec in `docs/specs/`, against non-negotiable 4.
- **No failing-first test is possible for a prose record.** The mechanical checks it does have to
  pass are the gate's own: `build-docs.sh` resolves every relative link (144 internal pages) and
  `check-provenance.sh` scans it (clean, 7 terms). Both were run against the new file.

## Notes
- Found by `M-16` while writing the ICE spec: the design record predates `M-1` and mentions neither
  ICE, NAT traversal nor DTLS-SRTP, yet `M-16` and all six of `M-19` … `M-24` cite it, and so do
  `M-17` and `M-18`. Every one of those stories sends its implementor to a stub.
- Low urgency, real cost: a `design:` field that points at nothing trains the next implementor to
  skip the field, and the next story after that has a design worth reading.
- Sibling of `X-24` in kind — a document that fell behind the code it describes — but not in
  remedy. A design record is an argument and cannot be generated; what keeps it honest is being
  worth reading, not a check.
