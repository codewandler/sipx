---
id: S-18
title: Implement presence and PUBLISH
pillar: Signalling
status: backlog
priority:
design:
epic: conformance
areas: [sipx-ua]
note: M8 · RFC 3856 + 3863 + 3903 · blocked by S-13
---

# Implement presence and PUBLISH

## Goal
Presence: the `presence` event package (RFC 3856) with PIDF documents (RFC 3863), and PUBLISH
(RFC 3903) so state can be *put into* the framework by whoever knows it rather than only served
out of whatever sipx happens to observe.

## Acceptance
- [ ] The `presence` package registers with `S-13`'s framework and serves PIDF documents.
- [ ] PIDF is a typed document, not a string template: a tuple with an id, a `basic` status of
      `open` or `closed`, an optional contact URI with a priority, and an optional note.
- [ ] PUBLISH creates soft state with an `SIP-If-Match` entity tag, refreshes it with a
      body-less PUBLISH carrying the tag, and removes it with `Expires: 0` (RFC 3903 §4).
- [ ] A PUBLISH whose `SIP-If-Match` names state that has expired is refused with **412
      Conditional Request Failed**, not accepted as new state — a publisher that lost the race
      must be told to start again rather than allowed to resurrect a stale document.
- [ ] Publishing to a resource with a live subscription notifies the subscriber.
- [ ] Failing-first test: `a_published_presence_document_reaches_a_subscriber`.

## Progress
- Not started. Blocked by `S-13`, and best taken after `S-17` — the packages that report state
  sipx already keeps are the ones that shake out the framework.

## Notes
- The entity tag is the whole point of PUBLISH and the part that is easy to skip. Without it,
  two publishers for one resource silently overwrite each other and neither can tell.
- Scope: PIDF only. RPID (RFC 4480) and the CIPID extensions add vocabulary, not mechanism, and
  can follow if anything asks for them.
