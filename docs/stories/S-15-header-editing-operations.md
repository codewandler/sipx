---
id: S-15
title: Add editing operations to Headers
pillar: Signalling
status: ready
priority: 5
design: docs/designs/sip-core.md
epic: sip-core
areas: [sipx-sip, sipx-transport]
note: track: core · a forwarding element cannot edit a header list without rebuilding it
---

# Add editing operations to Headers

## Goal
Give `Headers` the three editing operations that rewriting a message in flight needs — remove the
first occurrence, insert at a position, retain by predicate — so that changing one header does not
mean rebuilding the whole collection by hand.

## Acceptance
- [ ] `Headers::remove_first(&HeaderName) -> Option<Header>` removes the topmost occurrence and
      leaves the relative order of everything else untouched.
- [ ] `Headers::insert(index: usize, header: Header)` places a header at an absolute position;
      an index past the end appends rather than panicking, because this crate reads hostile input
      and index arithmetic on it is not allowed to be a panic site.
- [ ] `Headers::retain(&mut self, f: impl FnMut(&Header) -> bool)` filters in place.
- [ ] `sipx-transport`'s top-`Via` rewrite (`nat.rs::rewrite_top_via`) uses `remove_first` +
      `push_front` instead of the hand-rolled rebuild loop it has today, and its existing tests
      still pass unchanged.
- [ ] Failing-first test: `remove_first_takes_only_the_topmost_via`.

## Progress
- Not started. `Headers` has `push`, `push_front`, `remove_all`, `get`, `get_all` and `count` —
  everything a *reader* needs and nothing an editor does.

## Notes
- The in-crate consumer is real today: `crates/sipx-transport/src/nat.rs:149` allocates a fresh
  `Headers`, copies every header into it to replace one, and assigns it back. That is O(n) clones
  per rewrite on the received-path, and it is the only way the current API allows.
- Ordering is semantic for `Via`, `Record-Route`, `Route` and `Path` — RFC 3261 §8.1.1.7 and
  §16.6 — so "remove the first" and "insert at" have to be exact positions, not set operations.
- Requested by the downstream [sipx-clstr](https://github.com/codewandler/sipx-clstr) platform,
  whose proxy pops the top `Via` and pushes a `Record-Route` on every forwarded request
  ([its ledger](https://github.com/codewandler/sipx-clstr/blob/main/docs/upstream.md)); it blocks
  that project's `PX-3`/`PX-4`.
