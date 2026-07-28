---
id: S-15
title: Add editing operations to Headers
pillar: Signalling
status: done
priority:
design: docs/designs/sip-core.md
epic: sip-core
areas: [sipx-sip, sipx-transport]
note: M7 · a forwarding element cannot edit a header list without rebuilding it
---

# Add editing operations to Headers

## Goal
Give `Headers` the three editing operations that rewriting a message in flight needs — remove the
first occurrence, insert at a position, retain by predicate — so that changing one header does not
mean rebuilding the whole collection by hand.

## Acceptance
- [x] `Headers::remove_first(&HeaderName) -> Option<Header>` removes the topmost occurrence and
      leaves the relative order of everything else untouched.
- [x] `Headers::insert(index: usize, header: Header)` places a header at an absolute position;
      an index past the end appends rather than panicking, because this crate reads hostile input
      and index arithmetic on it is not allowed to be a panic site.
- [x] `Headers::retain(&mut self, f: impl FnMut(&Header) -> bool)` filters in place.
- [x] `sipx-transport`'s top-`Via` rewrite (`nat.rs::rewrite_top_via`) uses `remove_first` +
      `push_front` instead of the hand-rolled rebuild loop it has today, and its existing tests
      still pass unchanged.
- [x] Failing-first test: `remove_first_takes_only_the_topmost_via`.

## Progress
- Done. `remove_first`, `insert` and `retain`, and `nat.rs::replace_top_via` rewritten to use them.
  Its existing tests pass unchanged, which is what the acceptance asked for — the point was that
  the behaviour is already pinned and the implementation was the thing that needed to change.
- The rewrite went from allocating a fresh `Headers` and cloning every header to change one, to two
  operations that clone nothing. That was O(n) clones per rewrite on the received-path.
- **`insert` past the end appends rather than panicking.** This crate parses hostile input and a
  caller's index is often derived from it, so a panic there is a remote denial of service reachable
  through arithmetic — the class of bug the builders exist to make unrepresentable.
- **The mutation that survived was in my own new comment.** `replace_top_via` guards on there
  being a `Via` to replace, and I wrote that the guard mattered — but its one caller has already
  read the top `Via`, so nothing could reach it and removing the guard broke no test. Rather than
  drop the guard, the property is now pinned directly: adding a `Via` to a request that had none
  redirects the response, "replace" and "add" are different functions, and the second caller is how
  that bug arrives. The comment now says the guard is a contract rather than a live check.
- Ordering is exact, not set-like: `Via`, `Route`, `Record-Route` and `Path` order *is* the routing
  (RFC 3261 §8.1.1.7, §16.6), so `remove_first` takes the topmost and everything else keeps its
  place — asserted with a header of another name interleaved, to catch an implementation counting
  positions among matching headers rather than among all of them.

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
