---
id: X-145
title: Publish the redaction guarantee the checker enforces
pillar: Experience
status: done
priority: 8
design:
epic: conformance
areas: [website, docs, privacy, redaction]
predicate:
announcement:
note: found in a docs sweep at rc.19 · the strongest privacy property this project has is documented only for speech
---

# Publish the redaction guarantee the checker enforces

## Goal

State publicly, and at the scope it actually holds, that sipx's types cannot render call content in
a diagnostic record — and that this is enforced by a checker rather than by review.

## Context

Found by sweeping the published docs against everything merged since they were last touched
(2026-08-10, at the `1.0.0-rc.19` boundary).

`website/docs/reference/privacy.md` documents the guarantee **only for the speech contract**:

> The guarantee is in the types rather than in a review rule: every value that carries the call's
> data renders as its class and its size.

That was accurate when written, for the scope it claims. Since then the same property has been made
true of most of the workspace, story by story: nine media types stopped rendering audio, an RTP
packet stopped rendering its payload and an SDES item its owner's identity, the DSP graph stopped
rendering a frame in flight, six carriers in the crates where a call comes to rest stopped rendering
their octets, an authenticator stopped rendering its nonce signing key, and a reader now follows a
public `Debug` through six private hops to find carriers nobody declared.

`privacy.md` mentions RTP, SDES and byte buffers **zero times**. `logging.md` says a trace record
carries "never credentials, keys or message bodies" — which is now a checked property and reads on
the page as a policy intention.

So the gap is not an inaccuracy. It is that the strongest privacy claim this project can make is the
one a reader cannot find: someone asking "if I turn on debug logging, can sipx print my customers'
audio or their SIP identities" is answered only about speech, and has to take the rest on trust that
is in fact enforced.

## Acceptance

- [x] `privacy.md` states the guarantee at workspace scope, names the two scopes the checker
      distinguishes (the relay path a call passes through, and the crates where it comes to rest),
      and says what is deliberately *outside* them and why — encoded audio in an opaque byte type is
      not something an element type can decide.
- [x] The claim names its enforcement, so a reader can check it rather than believe it.
- [x] `logging.md`'s "never credentials, keys or message bodies" is tied to the rule that makes it
      true, including the fixed-size-array rule that treats such an array as a key.
- [x] What is *not* redacted is stated as plainly as what is: protocol headers a log exists to print,
      and the counts and lengths kept on purpose because an incident has to stay diagnosable.
- [x] The page cannot silently fall behind the checker again — a generated region, or a test that
      fails when a scope exists in the checker and appears in neither the page nor an exemption.
- [x] The gate is green.

## Progress

- Filed 2026-08-10 from a sweep of published docs against all behavioural changes since their last
  update.

- 2026-08-10: implemented on `impl/X-145`.

  **Failing first.** `TheRepositoryItself.test_public_pages_name_every_checked_diagnostics_scope`
  was added before either public page changed. Its first run failed with eight omissions: the two
  scope names, all four crates those scopes cover, and the enforcing checker's name on both pages.

  **Published contract.** `privacy.md` now states the raw-sample and fixed-size-key guarantees at
  workspace scope, and distinguishes the encoded byte relay path from the crates where a call comes
  to rest. It also states the boundary the narrowing exists for: an opaque byte element cannot
  distinguish audio from a protocol field, protocol headers remain visible, and counts and lengths
  remain visible deliberately. `logging.md` ties its existing trace-level promise to the same
  checker, including the `[u8; N]` key rule, and says explicitly that a SIP identity may appear in
  a protocol diagnostic.

  **Held in sync.** `diagnostic_documentation_problems` reads `RELAY_PATH` and `AUDIO_AT_REST`
  directly. Every crate in either constant must appear on the privacy or logging page or carry a
  reasoned `(scope, crate, reason)` exemption; both pages must name the checker. Mutation tests add
  a crate to each scope, accept a reasoned exemption, reject an empty one, and remove the checker
  from one page.

  **Focused verification.** `scripts/test-audio-claims.py` passes all 169 tests,
  `scripts/check-audio-claims.py --check` passes against the workspace, and
  `scripts/build-docs.sh` builds the samples, checks generated regions and 1,123 relative links,
  builds the site, checks a deliberately dead anchor, and builds the API reference. The complete
  wave gate was left for integration evidence.

- 2026-08-10: the complete 51-step local gate passed at `eb8c3193d436`; exact-SHA main CI and Pages
  passed in run `31399788932`, and the protected rc.21 release passed in run `31400675577`.
