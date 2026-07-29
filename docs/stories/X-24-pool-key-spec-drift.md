---
id: X-24
title: Stop the specs describing a connection pool key that has moved on twice
pillar: Build
status: in-progress
priority: 5
design:
epic:
areas: [docs]
note: sip-transport.md still says the key is two fields; it has been four since T-23
---

# Stop the specs describing a connection pool key that has moved on twice

## Goal
Make `docs/specs/sip-transport.md`'s account of the connection pool key true, and make it the kind
of claim that cannot go stale silently again.

## Acceptance
- [x] `docs/specs/sip-transport.md:120` describes the key `ConnectionKey` actually is. It says
      `(transport, remote address)`; the type carries the verified identity and, since `T-23`, the
      WebSocket resource.
- [x] The specs that describe the same key agree with each other — `sip-tls.md` §5 and
      `sip-quic.md` were corrected in `T-23`, and a third description that disagrees with both is
      the reason to prefer one place over three.
- [x] Whatever keeps it true is stated: either the specs stop restating the key and point at the
      one that defines it, or something checks them. `docs/compliance.md` and `X-22`'s gate drift
      check are the house pattern for "a claim that cannot quietly lag its source", and this is
      the same shape one size down.

## Progress
- Done. Both halves of the third item, because either alone leaves the failure reachable.

  **One place.** `sip-transport.md` §8 is the only spec that enumerates the key; `sip-tls.md` §5
  and `sip-quic.md` §6 now link to it instead of restating it. §5 keeps the *why* — that is an
  argument, not a list, and nothing can generate it.

  **And a check**, because pointing at one place would only have reduced three hand-written
  lists to one, and the one left would be the same kind of sentence that went stale twice: prose
  with nothing connecting it to `ConnectionKey`. So §8's list is a generated region rendered from
  the struct's fields and doc comments by `scripts/check-pool-key.py`, `--check` in the gate and
  `--update` to regenerate — the shape `rfc-report.py`/`docs/compliance.md` uses. The script also
  requires that any *other* spec section speaking of the pool key links to §8, which is the
  property this spec lacked: it described the key and cited nothing.

  Wired as `X-22` requires: gate steps `pool key` (CI job `docs`) and `pool key tests` (CI job
  `gate`), with the matching `run:` lines in `ci.yml`, so `gate.py --check` sees no drift.

- Not checked, deliberately: `sip-quic.md`'s key. QUIC is unimplemented, there is no
  `ConnectionKey` for it to disagree with, and a check demanding one would make "spec before
  code" impossible. It cites §8 and derives from it in prose.
- Left alone: `docs/designs/sip-transport.md:17` still says "keyed by (transport, remote)". It is
  an outline explicitly superseded by the spec it names ("_To be written by `T-1`_"); correcting
  a design record of what was planned would rewrite history rather than fix a claim.

## Notes
- Found by `T-23` while it was correcting the two specs inside its own fence. The third was
  already wrong before that story — it went stale when the verified identity joined the key, not
  when the resource did, which is the argument for a check rather than another correction.
- Small on its own. Worth doing because the pool key is exactly the sort of invariant a reader
  trusts a spec for rather than reading the type, and it has now been wrong through two changes.
