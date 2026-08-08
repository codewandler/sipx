---
id: M-76
title: Bound the inbound audio queue in time
pillar: Media
status: in-progress
priority: 1
design:
epic: demand
areas: [sipx-media, sipx-audio]
predicate:
announcement:
note: M-45 measured the jitter buffer and cleared it · the inbound channel holds 256 frames, which is 5.12 seconds of audio with no time bound, counter or shed policy
---

# Bound the inbound audio queue in time

## Goal

Put a stated time bound, a counter and a shed policy on the queue between the media session and the
application, which is where seconds of added delay actually accumulate.

## Acceptance

- [x] The inbound audio queue's bound is expressed in time rather than in frames, so the same
      configuration means the same delay at every packet duration.
- [x] A failing-first test proves an application reading slightly slower than real time settles at a
      bounded delay rather than at the far end of the queue and staying there.
- [x] Overflow is a stated policy — shed oldest, shed newest, or backpressure — chosen deliberately,
      documented in `docs/specs/media-runtime.md` §4, and counted in `MediaDiscardCounts` like every
      other media discard.
- [ ] The delivery contract change is stated in `CHANGELOG.md` with migration guidance: this alters
      what every application sees when it reads slowly. — **not done here.** `CHANGELOG.md` is the
      coordinator's file; the entry to lift is the blockquote in Progress below.
- [ ] `./scripts/gate.py` green. — not run; the wave runs one gate. The focused equivalents are in
      Progress.

## Progress

- 2026-08-08: filed from `M-45`'s measurement, which is the point of that story. `M-45`
  characterised the jitter buffer across ten 1,500-packet traces and **cleared it**: bounded, no
  ratchet, strictly ascending across the wrap, late and duplicate refused, worst measured hold 515 ms
  under a 300 ms spike every third packet. The delay is downstream. `crates/sipx-media/src/session.rs`
  holds inbound audio in an `mpsc::channel::<Vec<i16>>(256)`, one frame per packet, delivered with
  `send().await` — at 20 ms that is **5.12 seconds** of audio that can queue before backpressure
  reaches the socket, with no bound in time, no counter and no shed policy. That fits the two field
  reports of "seconds of added delay" far better than the buffer does.
- 2026-08-08: implemented on `impl/M-76`. The `mpsc::channel::<Vec<i16>>(256)` is replaced by
  `crates/sipx-media/src/inbound.rs`, a queue bounded by `Config::inbound_queue` (default
  **200 ms**) and specified in `docs/specs/media-runtime.md` §4.3.

  **The unit, decided deliberately.** The bound is a `Duration`, and it is enforced against the
  queued *sample count* at the session's audio rate rather than against a frame count. A frame
  depth means a different delay in every codec — the inconsistency `M-45` left noted against the
  buffer's own packet-counted depth — and it also means a different delay in the *same* session,
  because a far end may change its packetisation mid-call and this side cannot refuse it. Counting
  the audio is the only form of the bound that stays true whatever arrives. The default comes from
  the delay budget rather than the queue: ITU-T G.114 puts the one-way target at 150 ms and the
  limit of acceptable conversation at 400 ms, and the jitter buffer's ceiling, the network and the
  application each take a share of it.

  **The policy, decided against the alternatives.** Shed **oldest**. Backpressure — what this
  replaced — does not bound anything; it moves the delay into the kernel's socket buffer, where it
  is neither bounded nor counted. Shedding *newest* bounds the delay identically in the steady
  state but leaves the application listening to the oldest audio it could still be holding: after a
  stall it hears the beginning of the stall and the recent speech is gone. Shedding oldest is also
  what `call-audio-seam.md` §6.1 and `speech-providers.md` §8 already state at the other two
  application-facing queues, so all three shed the same end. Every shed frame increments
  `MediaDiscardCounts::inbound_frames_shed`.

  **Measured**, in `crates/sipx-media/tests/inbound_queue.rs`: 200 packets arriving with nothing
  reading the session handed the application **4 s** of audio before this change and **200 ms**
  after; the same bound holds in milliseconds at a 60 ms packetisation over a different number of
  frames; the marker that arrived last is delivered and the one that arrived first is not; and
  delivered frames plus `inbound_frames_shed` equals `packets_received` exactly.

  Verification run in this worktree, exit codes reported directly rather than through a pipe:
  `cargo test -p sipx-media -p sipx-rtp -p sipx-call --all-features`,
  `cargo clippy -p sipx-media -p sipx-rtp --all-targets --all-features --no-deps -- -D warnings`,
  `cargo fmt --all --check`, `check-fixed-sleep.py --check`, `check-audio-claims.py --check`,
  `check-provenance.sh`. `./scripts/gate.py` deliberately not run: the wave runs one.

  **`CHANGELOG.md` entry to lift** — this story does not write it, because that file belongs to the
  integration step:

  > **Changed — the inbound audio queue is bounded in time (`M-76`).** `MediaSession::recv`, and
  > everything built on it (`record_until_idle`, `record_at_least`, `capture`, `Bridge`,
  > `Conference`), now holds at most `Config::inbound_queue` of received audio — **200 ms** by
  > default — instead of 256 decoded frames delivered with a blocking send, which was 5.12 seconds
  > at the universal 20 ms packetisation. Past the bound the **oldest** queued frame is shed and
  > counted as `MediaDiscardCounts::inbound_frames_shed`, and delivery no longer blocks the receive
  > loop. See `docs/specs/media-runtime.md` §4.3.
  >
  > *What changes for your application.* One that reads as fast as the far end sends sees no
  > difference. One that reads more slowly now settles 200 ms behind live audio and stays there,
  > where before it settled at the far end of a five-second queue and stayed there — but it now
  > loses the audio it was not keeping up with, where before that audio was delivered late. If you
  > read a whole clip *after* it arrived rather than while it was arriving, you will now receive
  > only the last `inbound_queue` of it.
  >
  > *What to do.* Read concurrently with arrival — `record_at_least` and `record_until_idle`
  > already do — or raise `Config::inbound_queue` to cover the longest pause your reader takes.
  > (A `Call` uses the default, as it does for every other media setting; the field is on the
  > `MediaSession` configuration you build yourself.)
  > Watch `discard_counts().inbound_frames_shed`: anything above zero is this side reading more
  > slowly than the far end is talking, and it is the counter to check before blaming the network
  > or the jitter buffer for added delay. For a consumer that is legitimately slow — a recogniser,
  > an encoder, a disk writer — attach a `PcmProcessor` instead: `docs/specs/call-audio-seam.md`
  > §6 gives it its own queue with its own bound, so it cannot pull the call's delivery around.

## Notes

- `M-45` deliberately did not touch it: changing that queue's depth or policy changes the delivery
  contract for every application, and deserves its own story and its own measurement.
- `M-45` also left the buffer's depth counted in **packets** rather than time — the same wall-clock
  jitter costs a 40 ms mean hold at 20 ms ptime and 119 ms at 60 ms. That is a sizing inconsistency
  rather than the unbounded growth this story is about, but the two decisions should be made with
  the same unit in mind.
