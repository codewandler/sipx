---
id: X-24
title: Stop the specs describing a connection pool key that has moved on twice
pillar: Build
status: ready
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
- [ ] `docs/specs/sip-transport.md:120` describes the key `ConnectionKey` actually is. It says
      `(transport, remote address)`; the type carries the verified identity and, since `T-23`, the
      WebSocket resource.
- [ ] The specs that describe the same key agree with each other — `sip-tls.md` §5 and
      `sip-quic.md` were corrected in `T-23`, and a third description that disagrees with both is
      the reason to prefer one place over three.
- [ ] Whatever keeps it true is stated: either the specs stop restating the key and point at the
      one that defines it, or something checks them. `docs/compliance.md` and `X-22`'s gate drift
      check are the house pattern for "a claim that cannot quietly lag its source", and this is
      the same shape one size down.

## Progress
- Not started.

## Notes
- Found by `T-23` while it was correcting the two specs inside its own fence. The third was
  already wrong before that story — it went stale when the verified identity joined the key, not
  when the resource did, which is the argument for a check rather than another correction.
- Small on its own. Worth doing because the pool key is exactly the sort of invariant a reader
  trusts a spec for rather than reading the type, and it has now been wrong through two changes.
