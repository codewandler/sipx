---
id: X-46
title: Stop `sip-tls.md` claiming a configurable minimum TLS version that does not exist
pillar: Build
status: done
priority: 5
design: docs/designs/rfc-registry-grain.md
epic: conformance
areas: [docs, sipx-transport]
note: found by X-43's implementor — `docs/specs/sip-tls.md` §3.2 lists the minimum protocol version as CONFIGURABLE, but neither `ClientTls` nor `ServerTls` takes a version and nothing above them names one
---

# Stop `sip-tls.md` claiming a configurable minimum TLS version that does not exist

## Goal
Make the TLS spec's list of configurable knobs match what is actually configurable, so a normative
document stops describing an option nobody can set.

## Acceptance
- [x] **The false sentence is located and corrected.** `docs/specs/sip-tls.md` §3.2 lists "the minimum
      protocol version, at or above the floor in §3.5" as CONFIGURABLE. It is not: neither `ClientTls`
      nor `ServerTls` takes a version, and nothing above them names one. The floor is whatever the TLS
      library offers, which `X-43` established is `{1.3, 1.2}` for rustls and pinned with
      `the_library_offers_nothing_below_the_floor`.
- [x] **Decide which way to close it, and say why.** Either the sentence goes (the version is not
      configurable, and the spec says the floor is the library's), or the option is built (the types take
      a minimum version). Both are defensible; what is not defensible is leaving a normative document
      describing an API that does not exist. Note the direction of harm is mild — the claim could only
      let an operator *tighten* — which is an argument about priority, not about whether it is false.
- [x] **If the answer is "delete the claim", the spec says what governs instead.** `X-43` established
      that the property is a **dependency** property rather than sipx behaviour, and stated that in three
      places precisely so a backend swap cannot move it silently. §3.2 should point at the same fact
      rather than implying sipx decides it.
- [x] **Sweep §3.2's other CONFIGURABLE entries against the code in the same change.** One false entry in
      a list of knobs is a reason to check the list, not just the entry — this is the same reasoning that
      made `X-35` read 44 front doors after `X-26` fixed three strings. Report how many entries were
      checked and how many were wrong.
- [x] Failing-first evidence: this is a documentation-accuracy defect, so a conventional red test may not
      exist. State the substitute honestly — a check that each CONFIGURABLE entry names a real
      constructor parameter or setter would be a real guard and would be red now. Decide whether that
      guard is in scope or its own story.

## Progress
- Filed from `X-43`'s ADJACENT finding 1.
- **The claim was corrected, not built.** `docs/specs/sip-tls.md` §3.2 no longer lists a minimum
  protocol version; it states that the version is not configurable and that the floor is the TLS
  library's, pointing at the same fact §3.5, `tls.rs`'s module documentation, `tests/tls_versions.rs`
  and RFC 8996's registry row already state. The knob was declined because its only representable
  value above the default is "1.3 only" — rustls offers `{1.3, 1.2}` and RFC 8996 makes anything
  lower unrepresentable — and because RFC 8996's and 8446's evidence is currently *the absence* of a
  version-selecting API (`docs/rfc/README.md`: "proved by the absence of an API"). Building one would
  have falsified three documents to satisfy a fourth.
- **The guard is in scope and is the failing-first evidence.**
  `crates/sipx-transport/tests/tls_spec.rs::every_configurable_entry_names_a_real_api` requires every
  §3.2 entry to name a public item of `src/tls.rs`, resolving `Type::method` as a pair. At the merge
  base all three entries failed it, the false one indistinguishable from the two true ones. A second
  test, `no_tls_version_is_named_in_the_crate`, is the tripwire for the other direction: it goes red
  the day sipx does select a version, naming the four documents that must move with it.
- **Sweep of §3.2, three entries checked, one false and one imprecise.** The trust anchors and the
  client certificate are real (`TrustAnchors`/`ClientTls::new`, `ClientTls::with_identity`); the
  minimum version was false; and "Default: the system roots" was imprecise — there is no default at
  all, the anchors are a required argument and `ClientTls::new` refuses an empty set at construction.
  Both surviving entries now name their API.
- **Not touched:** `docs/rfc/registry.toml`. What sipx supports did not change, and the RFC 8996 row
  already said the opposite of the deleted claim ("1.2 is the floor and is not configurable
  downward… sipx implements no TLS and names no version"), which is part of why the spec was the
  wrong side of the discrepancy.
- **Adjacent, not fixed:** §3.4's server half — "as a server, sipx requests a client certificate only
  when configured to, and when it does, an unverifiable one is refused" — describes behaviour with no
  API behind it either. `ServerTls` exposes only `new` and `acceptor`, and `new` builds the config
  with `with_no_client_auth()`, so sipx can never be configured to request one. Same defect class,
  different section, and outside this story's Acceptance; it wants its own story.

## Notes
- **Found by `X-43`**, which cited `crates/sipx-transport/src/tls.rs` as evidence for RFC 8996 and read
  the spec closely enough to notice the neighbouring sentence was false. `X-43` deliberately left it
  rather than widening its own scope, which is why this exists.
- **Reads with `X-43`'s new registry rule**: an `implemented`/`partial` row must now cite a `crates/*.rs`
  path, because "a document cannot stop being true on its own". This story is the same principle applied
  one level down — inside a spec rather than in the registry.
- Also flagged by `X-43` and handled at integration rather than here:
  `website/docs/reference/compliance.md` understated the checker's rule after `X-43` tightened it.
