---
id: A-9
title: Make the published crates safe to freeze — `#[non_exhaustive]` and a README per crate
pillar: Application
status: done
priority: 4
design: docs/vision.md
epic: app-sdk
areas: [docs, sipx-sip, sipx-call, sipx-media, sipx-transport, sipx-ua, sipx-sdp, sipx-rtp, sipx-audio]
note: A-8 stated the promise and left the two mechanical halves — every public error enum outside sipx-app-protocol is exhaustive, so it promises never to add a variant, and ten of eleven crates will publish to crates.io with no README at all
---

# Make the published crates safe to freeze — `#[non_exhaustive]` and a README per crate

## Goal
Close the two mechanical gaps `A-8` deliberately left: an exhaustive public enum is a promise sipx has
already broken, and a crate with no README publishes a one-line description as its entire landing page.

## Acceptance
- [x] **Every public error enum outside `sipx-app-protocol` is `#[non_exhaustive]`, or its exhaustiveness
      is argued per type.** They are all exhaustive today, which is a promise never to add a variant.
      `sipx_call::Error` went from **13 variants at v0.8.0 to 16 at v0.9.0**, breaking every downstream
      exhaustive `match`, and `website/docs/guides/place-a-call.md:128-133` teaches readers to write one —
      it survives only because the sample happens to carry a `_` arm. The list: `ParseError`,
      `BuildError`, `UriError`, `HeaderError`, `FramingError`, `StartLineError`, `HeaderSyntaxError`
      (`sipx-sip`); `Error`, `TlsError`, `WsError` (`sipx-transport`); `Error` (`sipx-ua`); `SdpError`;
      `RtpError`, `RtcpError`, `SrtpError`; `WavError`, `OpusError`; `dtls::Error`, `KeyError`,
      `DtlsError`, `ice::stun::Error` (`sipx-media`); `Error` (`sipx-call`). `sipx-media::Interrupt` is
      already marked and is the model.
- [x] **Expect the workspace to break, and fix it rather than narrowing the change.** `#[non_exhaustive]`
      does not affect matches inside the defining crate, but it does affect **cross-crate** ones — and
      this workspace has eleven crates matching on each other's errors. That breakage is the point: it is
      the same breakage a downstream user would have hit, arriving where it can be fixed.
- [x] **`sipx-app-protocol`'s documented exception is preserved.** `interpreter.rs:208-210` deliberately
      leaves `Output` exhaustive and says why. Do not sweep it up; a blanket rule that overrides a
      reasoned exception is worse than no rule.
- [x] **Every published crate sets `readme` and ships the file it names.** No manifest sets it and
      `[workspace.package]` does not either, so ten of eleven publish with **no README on crates.io** and
      the one-line `description` is the whole landing page. Only `sipx-app-protocol` has one.
- [x] **A per-crate README is not a copy of the workspace README.** It should say what the crate is, what
      it guarantees (pointing at the `# Stability` section `A-8` added rather than restating it — a
      restatement is a fifth front door to drift), and what it deliberately does not do.
- [x] The front-door guard covers the new READMEs. `scripts/check-audio-claims.py` already holds four
      doors per crate to agreement, with the rule that a restatement may say **less** than the crate's own
      listing and never more; a per-crate README is a fifth door and must not be exempt.
- [x] Failing-first test: name the test that fails while an error enum outside `sipx-app-protocol` is
      exhaustive, and the one that fails while a published crate sets no `readme`.

## Progress
- **Done.** The live census found 27 public error enums outside `sipx-app-protocol`: 26 extensible
  errors are now `#[non_exhaustive]`, and `sipx_app::HostError` is the one exhaustive exception,
  argued beside the type as a closed set of host boundaries. The guard has no exception list: an
  exhaustive type carries `Exhaustive by design:` at its declaration or fails.
- **The breakage arrived where it could be fixed.** The RFC 4475 and RFC 5118 integration suites
  exhaustively matched `ParseError`; both now classify the three faults their corpus vocabulary can
  name and leave present or future non-fault variants unclassified through the required wildcard.
- **All eleven published packages ship a README.** Ten are new and `sipx-app-protocol`'s existing
  one now points at the crate-level stability contract. The pages state what the crate is, link the
  one stability source of truth, and name their deliberate lower- or higher-layer boundary.
- **Five front doors per crate are held together.** `check-audio-claims.py` reads the package
  README's lead paragraph beside the manifest description, crate-doc summary and two crate tables:
  55 checked doors. It ignores later disclaimer sections so "does not implement" cannot be read as
  a capability claim.
- **Failing-first evidence:**
  `test_every_public_error_enum_is_non_exhaustive_or_argued_at_the_type` failed on 26 enums, and
  `test_every_published_crate_sets_and_ships_a_readme` failed on ten packages. The latter runs
  `cargo package --list` for every published crate, so it proves the file is shipped rather than
  only present. `./scripts/gate.py` passes all 25 steps.

## Notes
- **Why `A-8` stopped where it did.** Alpha predicate 5 asks that the line between supported and
  experimental *exist*. It now does, in eleven crates, enforced by `stability_problems` in the front-door
  guard. Marking twenty enums is a mechanical change with real cross-crate fallout, and bundling it into
  the story that draws the line would have made the line's arrival contingent on a refactor.
- **This is v1 work, not alpha work**, and v1 predicate 4 in `docs/roadmap.md` is the reason: it asks for
  at least one instance of a breaking change being *shaped by* the contract rather than the contract being
  edited to fit the change. Marking these enums is the cheapest honest instance of that.
- The `readme` half has a subtlety worth knowing before starting: cargo auto-infers `README.md` when the
  file sits beside the manifest, which is why `sipx-app-protocol` already has one without setting the key.
  So the fix is a file per crate, and the key only where the name differs.
- Reads with `X-35`, which built the guard, and with `A-8`, which added the declarations the new READMEs
  must not contradict.
