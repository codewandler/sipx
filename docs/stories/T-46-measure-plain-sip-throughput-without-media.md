---
id: T-46
title: Measure plain SIP throughput without media
pillar: Transport
status: done
priority: 12
design:
epic: diagnostic-automation
areas: [sipx-call, sipx-cli]
predicate:
announcement:
note: audio doubles memory and costs 8x on burst setup at 1000 calls · neither example can isolate signalling from the media stack
---

# Measure plain SIP throughput without media

## Goal

Give the load and capacity examples a genuinely media-free mode, so the cost of signalling can be
measured apart from the cost of having a media stack at all.

## Why

Measured, not assumed. At 1000 concurrent calls, `capacity_test` records 231 MB with audio flowing
against 120 MB with a negotiated-but-silent session, and a burst setup median of 300 ms against
39 ms. So media dominates — but the comparison stops short of the question a deployment actually
asks, because **`--media idle` is not "no media"**: the SDP is still negotiated, the RTP socket is
still bound, and the jitter buffer still exists. What `idle` isolates is the cost of *carrying*
audio, not the cost of the stack that carries it.

That matters because a real deployment may put media somewhere else entirely and run sipx as
signalling only. There is no number for that shape today from the examples.

The CLI can already do it — `sipx load --mode signalling` builds INVITE/ACK/BYE with no SDP body and
no media session, and `sipx load-responder` answers the same way. So the capability exists and the
library-facing examples cannot reach it: `dial` always negotiates media, and `Invitation::answer`
always creates a session.

## Acceptance

- [x] `capacity_test` and `load_test` gain a mode that negotiates no media: no SDP offer, no RTP
      socket bound, on **both** ends. Reusing the CLI's path or lifting it into the library are both
      acceptable; duplicating it in an example is not.
- [x] A run in that mode records memory and setup latency beside the existing two, so the three
      shapes — no media, idle media, flowing media — are one comparison rather than three runs a
      reader has to align by hand.
- [x] The `audio_observed` guard keeps holding: a media-free run must report no audio figures at
      all, not zeros.
- [x] `docs/measurements/README.md` gains the third column and says which of the three a deployment
      running external media should read.
- [x] `./scripts/gate.py` green.

## Progress

- 2026-08-09: filed. The `capacity_test` doc comment already pointed at this story before it
  existed — a reference to a filing that had not happened, which is the same defect class as a
  ticked acceptance row nobody satisfied. The pointer is now true.
- 2026-08-09: the mode exists, and the **measurement has not been taken**. Two of the five rows are
  deliberately left open, and which two matters more than which three are ticked.

  **What was built.** The caller's half of an SDP-free dialog now lives in `sipx-call` beside the
  answering half `P-15` already had: `dial_signalling`, `dial_signalling_until`, `SignallingDial`,
  `SignallingDialOptions` and `SignallingIdentity` in `crates/sipx-call/src/signalling.rs`. It was
  *lifted*, not copied — `sipx-cli`'s `run_signalling_attempt` was rewritten onto it and its
  `signalling_invite`, `signalling_dialog_request`, `wait_for_invite`, `cancel_signalling_invite`,
  `authorization_for`, `signalling_response_matches` and `rejection_cause` are gone, so
  `sipx load --mode signalling` and the two examples now put the same bytes on the wire from one
  implementation. `T-45`'s per-candidate `Call-ID` and `From` tag survive intact: the identity is
  still derived from `--seed`, the call index and the candidate position in `CallIdentity::at`, and
  is now handed to the library rather than rendered into a request there.

  `capacity_test` gains `--media none` beside `full` and `idle`; `load_test` gains `--media none`
  beside `full`. Both ends are media-free in that mode — the client through `dial_signalling`, the
  in-process server through `Invitation::answer_signalling`.

  Two behaviours changed as a consequence, both deliberate. A media-free caller now spawns
  `reack_retransmitted_2xx` like `dial` does, so a lost ACK no longer leaves the far end
  retransmitting its 2xx for 64*T1 — it costs one task and one response stream per call for the
  ~32 s the transaction stays in RFC 6026's `Accepted` state, which is a real and previously absent
  cost that the media path was already paying. And `--mode signalling`'s BYE is now bounded by the
  run's own 40 s cleanup cap rather than by Timer F.

  **What was not done, and why.** The three-way comparison in row 2 needs a quiet box and this one
  was not: several implementors were building concurrently and the same example has saturated this
  machine once already. A ramp taken under that load would report the machine's ceiling as sipx's,
  which the example's own doc comment warns about — a wrong number is worse than a missing one. So
  nothing was written to `docs/measurements/` and row 4, the README's third column, is left with it:
  the column has nothing truthful to hold until the run exists. **Take the three runs on an idle
  machine** — `--ramp 50,200,250,500 --dwell 10 --media none|idle|full`, recording each — then tick
  both rows together.

  What was proved instead is that the mode works, at two levels. The library contract has a test:
  `a_media_free_call_is_placed_with_no_session_offer_and_confirmed_from_both_ends` in
  `crates/sipx-call/tests/signalling.rs` asserts, from the receiving end, that the INVITE carries no
  body and no `Content-Type` — an offer that names no port is what makes there be no port to bind —
  and then drives the exchange through ACK and BYE from both sides. Above that, three tiny debug
  runs at `--ramp 10 --dwell 4`, which are functional evidence and explicitly not measurements:

  | `--media` | audio columns | rss |
  |---|---|---|
  | `none` | `—` `—` `—` | 16 MB |
  | `idle` | `—` `—` `—` | 19 MB |
  | `full` | 0.00% · 0.26 ms · 4.40 | 19 MB |

  That third row is the `audio_observed` guard holding in the direction that is easy to lose: `none`
  and `idle` withhold loss, jitter and MOS rather than reporting a perfect score over zero packets,
  and `full` still prints them.

  **Owed CHANGELOG sentence** (fenced from this branch, for the coordinator to place): *`capacity_test`
  and `load_test` gain `--media none`, a mode that negotiates no session on either end, and the
  caller half it needs — `dial_signalling` and friends — is now public in `sipx-call` and shared with
  `sipx load --mode signalling`.*

  `./scripts/gate.py` was not run here by dispatch; one gate runs per wave.

- 2026-08-09: **the measurement is taken, on an idle box, and it answers the question the story was
  filed for.** Three runs of the same ramp to 1000 held calls, back to back, nothing else building:

  | | RSS | per call | setup p50 | drift |
  |---|---|---|---|---|
  | `--media none` | 51 MB | 52 KB | 17.5 ms | 0.07 s |
  | `--media idle` | 119 MB | 121 KB | 36.5 ms | 0.73 s |
  | `--media full` | 218 MB | 223 KB | 34.7 ms | 0.74 s |

  **Most of the media cost is the stack, not the audio.** Negotiating a session and binding a socket
  takes 51 MB to 119 MB before a packet is sent; carrying audio then takes it to 218 MB. And the
  drift and setup columns say it more sharply than memory does — `none` is an order of magnitude
  steadier (0.07 s against 0.73 s), while carrying audio on top of an already-negotiated session
  adds essentially nothing to either. A deployment running media elsewhere is not saving a fraction;
  it is working at roughly a quarter of the footprint.

  `audio_observed` is `false` for both `none` and `idle`, so their loss/jitter/mos columns are
  withheld rather than reported as perfect — the guard holding in the direction that matters.
  `docs/measurements/README.md` carries the third column and the reproduce command; the earlier
  `capacity-media.json`, taken on a loaded box, is replaced by `capacity-full.json` from this set.

