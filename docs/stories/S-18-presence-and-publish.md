---
id: S-18
title: Implement presence and PUBLISH
pillar: Signalling
status: done
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
- [x] The `presence` package registers with `S-13`'s framework and serves PIDF documents.
- [x] PIDF is a typed document, not a string template: a tuple with an id, a `basic` status of
      `open` or `closed`, an optional contact URI with a priority, and an optional note.
- [x] PUBLISH creates soft state with an `SIP-If-Match` entity tag, refreshes it with a
      body-less PUBLISH carrying the tag, and removes it with `Expires: 0` (RFC 3903 §4).
- [x] A PUBLISH whose `SIP-If-Match` names state that has expired is refused with **412
      Conditional Request Failed**, not accepted as new state — a publisher that lost the race
      must be told to start again rather than allowed to resurrect a stale document.
- [x] Publishing to a resource with a live subscription notifies the subscriber.
- [x] Failing-first test: `a_published_presence_document_reaches_a_subscriber`.

## Progress
- Done. `Pidf` is a typed document, `Publish` reads what a PUBLISH is asking for, and `Compositor`
  holds the soft state with its entity tags.
- **The entity tag is the mechanism, and the notes were right that it is the part that gets
  skipped.** Every acceptance issues a *fresh* tag, including a refresh (§6 step 6) — so a
  publisher that kept its old tag is refused on its next attempt, which is what makes the new one
  mean anything. Without tags at all, two publishers for one resource overwrite each other and
  neither can tell.
- **412 for a tag this server does not hold**, and the case the story singles out is tested
  directly: state that expired while the publisher was not looking. Accepting the refresh as a new
  publication would resurrect a document the server had already forgotten and that nothing has
  re-sent.
  - Expiry is judged against the clock inside `find`, not only in `expire`. Otherwise whether a
    publisher is told 412 depends on how recently a sweep ran, which makes the answer a race.
- The three operations are read from what is present (§4.1) rather than dispatched by the caller:
  a tag with no body is a refresh, with a body a modify, with `Expires: 0` a removal. Neither a
  body nor a tag is not an empty publication — there is nothing to publish and nothing to identify
  — so §6 step 5 refuses it.
- **PIDF is `open` or `closed` and nothing else** (RFC 3863 §4.1.3). The vocabulary people expect —
  busy, away, on the phone — is RFC 4480's, a different document; inventing tokens here would put
  values in a namespace that does not define them.
- Priority is clamped to §4.1.4's range rather than trusted: a document carrying 7.5 is one a
  watcher may reject outright, losing the whole presence rather than one number.
- Composition is deliberately not implemented: a second publication for one presentity replaces the
  first. Merging several publishers' documents is what the RFC calls composition *policy*, and a
  policy belongs to whoever has one.
- Mutation-tested: accepting a stale tag, never changing the tag, treating `Expires: 0` as a
  refresh, and accepting a publication with nothing in it.
- **The publish-to-subscriber chain is tested end to end in `crates/sipx-ua/tests/packages.rs`**,
  with nothing assumed on either side: a real SUBSCRIBE for `presence` is established through
  `S-13`'s notifier, a publication goes through the compositor, and the NOTIFY that would follow is
  assembled from both — its `Subscription-State` from the framework, its body from the compositor.
  Publishing a second time shows the *change* reaches the same subscriber, which is what makes it a
  subscription rather than one fetch.
- The end of that chain is asserted too: once the subscription is terminated the notifier produces
  no state, so a further publication reaches nobody through it. A compositor that still holds a
  document is not a subscriber that still gets one.

## Notes
- The entity tag is the whole point of PUBLISH and the part that is easy to skip. Without it,
  two publishers for one resource silently overwrite each other and neither can tell.
- Scope: PIDF only. RPID (RFC 4480) and the CIPID extensions add vocabulary, not mechanism, and
  can follow if anything asks for them.
