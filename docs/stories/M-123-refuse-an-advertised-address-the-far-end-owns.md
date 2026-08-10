---
id: M-123
title: Refuse an advertised media address the far end will read as its own
pillar: Media
status: ready
priority: 10
design: docs/designs/cluster-names.md
epic: media
areas: [sipx-media, sipx-call, sdp, diagnostics]
predicate:
announcement:
note: found while dialling a real endpoint · answered, and no audio, with no error anywhere
---

# Refuse an advertised media address the far end will read as its own

## Goal

Refuse, at dial time, to advertise a media address that the far end will resolve to something on its
own network — and say which address and which range, rather than letting the call answer and carry
no audio.

## Context

Found against a live endpoint on 2026-08-10. A call answered, signalling completed, and no RTP came
back. Nothing was misconfigured in any way either end could see, and every counter on both sides
read healthy.

Two independent faults were behind it, and both are silent by construction:

1. **Signalling and media took different interfaces.** The endpoint was dialled over a tunnel and
   answered with the public address it is configured to advertise. A media socket bound for the
   tunnel cannot reach that address, and no firewall rule changes it. The observable tell was a
   retransmitted `200 OK` — the far end never received the `ACK`, which took the same path media was
   about to take.
2. **The advertised address fell inside a range the far end already uses.** The tunnel address was
   inside the far end's own virtual-service range, so it sent media there in perfect good faith, to
   a destination on its own network. Nothing went wrong: the packets were delivered exactly where
   the SDP said to deliver them.

The second is the one worth a refusal. Private ranges come from a small pool and two networks choose
from it independently; a collision is not exotic and it produces no error at either end. sipx already
separates the bound address from the advertised one (`DialOptions::with_media_bind_address`), so the
material to check with is in hand at dial time.

This story does not attempt to *repair* either fault — advertising a reachable address needs
information sipx does not have without STUN, and `docs/designs/cluster-names.md` records why. It
makes the silent case loud.

## Acceptance

- [ ] A dial whose advertised media address falls inside a range the answer's own connection address
      implies is refused before the call is placed, naming both addresses and the range.
- [ ] The check is on the *answer*, not a guess: it compares what this side is about to advertise
      against what the far end said about itself, and does nothing when the two cannot collide.
- [ ] A caller that means it can proceed — the refusal is overridable by an explicit option, because
      an operator who has arranged routing for an overlapping range is not making this mistake.
- [ ] A one-way media path that is *not* a collision is diagnosed separately and not conflated with
      it: signalling on one interface and media on another has its own message naming both.
- [ ] Failing-first tests cover a colliding pair, a non-colliding pair, the override, and the
      split-interface case; none of them needs a second host.
- [ ] The gate is green.

## Progress

- Filed 2026-08-10 from a live diagnosis. `website/docs/guides/troubleshooting.md` and
  `website/docs/guides/hear-a-call.md` document both failures for a human; this is the half a
  machine can catch.
