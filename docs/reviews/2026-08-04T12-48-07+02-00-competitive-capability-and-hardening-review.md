# Competitive capability and hardening review — 2026-08-04

## Executive assessment

**sipx clears the external capability bar for a SIP user-agent and call framework on every axis it
can be measured on inside this repository, and falls short on the one axis that can only be earned
outside it.**

Measured against the bar set by mature open-source SIP endpoint stacks in other languages, sipx
leads on protocol breadth (100rel/PRACK, session timers, UPDATE, early media, attended transfer,
ICE, QUIC and overload control are all shipped here and are not uniformly available at that bar),
on conformance accounting (72 RFCs in a machine-checked registry against roughly ten claimed in
prose), on verification (1,756 test attributes, five fuzz targets including an adversarial-timing
one, two RFC corpora recovered bit-exactly from their own archives, a 32-step gate that
self-verifies against CI), on documentation rigor (a site that cannot ship an uncompiled sample, a
dead anchor, or a CLI flag its reference omits), and on the security properties that are decided by
construction rather than by discipline — `forbid(unsafe)`, verification that cannot be disabled,
SIPS that never downgrades, pre-allocation bounds, constant-time comparison on every secret path,
and CRLF injection prevented by a fallible constructor.

It falls short on **adoption maturity**, and the shortfall is not close. The reference bar has a
stable major version, years of release history, and third-party production deployment. sipx has
eight days of git history, no published crates, no external user, and no third-party audit. Its own
1.0 predicates already say this — "the public API has been used from outside this repository, by at
least one application nobody here wrote" — so nothing here is news to the roadmap. It is stated
because it is the whole remaining distance.

Two findings are more urgent than anything in the feature comparison:

- **The documented install path does not work.** The published site tells a first-time user to run
  `cargo install --locked --version =1.0.0-alpha.3 sipx-cli`, and no sipx crate exists on the
  registry. The `--git` fallback two paragraphs later does work, so a persistent reader recovers.
  Step one of the quickstart does not. (`A-10`, `A-11`.)
- **The published site is a release and a half stale.** It serves 1.0.0-alpha.3 while committed
  `main` is at alpha.5 and the working tree at beta.1, because deploy is gated on a push to `main`
  that has not happened. Two pages present locally are absent from the live sitemap. The site's own
  honesty guards are protecting content nobody is reading. (`A-12`.)

Both sit inside stories already in flight. Neither is a new defect class; both falsify announcement
predicate 5 today.

The corrected verdict by axis: **protocol breadth: ahead. Conformance accounting: far ahead.
Verification: ahead. Documentation rigor: ahead; documentation teaching: behind. Code quality:
ahead, with a file-size problem. Security: ahead by construction, unproven by exposure. Adoption
maturity: behind, and only time and users close it.**

## Review identity and method

- Review time: `2026-08-04T12:48:07+02:00` (`Europe/Berlin`)
- Base: working tree at workspace version `1.0.0-beta.1`; last commit `01a7e4c`
- Method: source-level inspection of this repository, against a documented external capability bar
  compiled from published specifications, release notes, public advisories and documentation of
  comparable open-source SIP endpoint stacks
- Evidence used: the crate sources and their tests; [`docs/rfc/registry.toml`](../rfc/registry.toml)
  and generated [`compliance.md`](../compliance.md); generated [`maturity.md`](../maturity.md) and
  the [story board](../stories/README.md); `scripts/gate.py --list`; `.github/workflows/ci.yml`;
  the `website/` tree and its published deployment

**On sourcing.** Per `AGENTS.md` non-negotiable 1, this document names no third-party project. The
external bar is stated as a capability threshold, and every conclusion below is expressed in sipx's
own terms with RFC citations. The comparison inputs are recorded outside this repository, in the
same location as the original scope survey, and are not referenced from here.

**On evidence asymmetry.** This repository was inspected at source level; the external bar was
assessed from published material. That asymmetry mildly flatters sipx's apparent rigor, and no
conclusion below rests on it. Where a claim about the bar could not be verified from source, it is
not made.

## Findings by axis

### Protocol breadth — ahead

Shipped here and not uniformly available at the bar: reliable provisional responses with PRACK
(RFC 3262) in both roles, session timers (RFC 4028) with refresher election, UPDATE (RFC 3311) with
491 glare handling, early media on the early dialog (RFC 3960 §3) in both roles, attended transfer
via Replaces (RFC 3891) with Refer-Sub (RFC 4488), ICE (RFC 8445/8839) including restart, a SIP
mapping over QUIC, and hop-by-hop overload control (RFC 7339/7415).

The honest qualification: part of sipx's lead is **shelf inventory rather than reachable capability**.
The N-party conference mixer and the media bridge exist and cannot be reached from a `Call` (`C-6`).
The RFC 6665 notifier and three event packages exist and nothing in the workspace receives a
SUBSCRIBE from a socket (`S-35`, new). Off-media coupling is specified and unbuilt (`C-7`). From a
user's seat these are indistinguishable from absent — worse, in fact, because the registry and the
crate documentation describe behaviour that cannot be invoked. `X-37` already settled the doctrine;
these are the places it has not yet been applied.

Genuine capability gaps against the bar: **no relayed ICE candidate** (`M-24`), and **a single SRTP
protection profile**. `AES_CM_128_HMAC_SHA1_80` is the right floor and RFC 5764's
mandatory-to-implement, but the AEAD-GCM profiles of RFC 7714 are absent, so an AEAD-only peer
cannot negotiate media with sipx at all (`M-41`, new).

Out of scope by design and not counted against sipx here: proxy, registrar-server, routing and
dial-plan roles, which `docs/designs/edge.md` places in a separate platform built on this kernel.

### Conformance accounting — far ahead

72 RFCs tracked with a closed key set and a gate step that fails on an unbacked claim, against
roughly ten claimed in prose at the bar, with no registry and no per-role granularity. The security
layer is 10 of 11 implemented with zero partials.

The limit is stated in `docs/rfc/README.md` and holds: `rfc-report.py --check` verifies that a cited
file exists and a named header is known to the parser. It cannot verify that behaviour is correct.
The registry is a well-maintained claim, not a proof.

### Verification — ahead, with one measurement absent

1,756 test attributes across 79 integration files and 115 source files with test modules. Both RFC
corpora are recovered from their own Appendix A archives with a re-recovery diff, which is the only
mechanism that can distinguish a hand-edited fixture from the RFC's bytes — at the bar the
equivalent corpus is present with roughly half its cases commented out and the enabled ones
asserting only that some error occurred. Five fuzz targets cover the datagram parser, the stream
parser, URIs, round-tripping, and **adversarial timing** through `transaction_sequence`, whose
oracle can fail without panicking and whose driver is shared with the regression harness. CI runs a
matrix the bar's single `go test` job does not approach.

Three gaps:

1. **Coverage is measured nowhere** — no `cargo llvm-cov` or equivalent in the gate or CI (`X-66`,
   new). The bar publishes a number, weakly (a static badge), and sipx publishes none.
2. **Property testing is thin** — two files. Fuzzing carries the adversarial load.
3. **Three input-refusal properties are asserted by design and pinned by no test** (`X-64`, new).
   Bounds are checked before allocation and `Header::build` is fallible, but nothing fails if either
   property is removed. The three classes worth pinning are the ones that recur across independent
   SIP implementations: a request missing the headers response construction reads (RFC 3261
   §8.2.6.1); an allocation sized from a peer-declared length, on **every** framing path
   independently including WebSocket frames (RFC 7118, RFC 6455 §5.2); and a declared length that
   disagrees with the bytes that follow. The same applies to the cryptographic branch and tag RNG
   that `docs/specs/sip-transport.md:110` requires (`X-65`, new).

An earlier draft of this review recorded an RFC 5118 corpus gate gap. That was **stale and is
withdrawn**: `X-16`, `X-51` and `X-56` are all `done` and the 5118 check is a gate step alongside
its 4475 twin.

### Documentation — rigor ahead, teaching behind

The site is 17 pages and roughly 21,500 words, with a CLI reference diffed against the built
binary's own `--help`, four guide samples inlined byte-exactly from example files CI compiles, a
generated RFC compliance table, offline search, a published rustdoc reference built with
`-D warnings`, and a build that throws on a broken link, a broken anchor, a broken relative link, a
duplicate route, or any warning at all. `missing_docs` is enforced workspace-wide, so no
undocumented public item can land. Nothing here can rot silently, and at the bar the equivalent
samples are plain prose code with no compilation guarantee.

Where the bar is ahead, and it is not close either:

1. **It is installable today.** See the executive assessment.
2. **It teaches.** Multiple pages orient a reader in SIP, media and RTP before asking them to write
   code. sipx has none, and one diagram in the entire corpus.
3. **It explains its own architecture.** sipx's central design property is the sans-I/O layering
   — the reason the core is fuzzable, deterministic under virtual time, and runtime-free — and it is
   documented only on contributor-facing surfaces (`AGENTS.md`, crate headers, specs). The site never
   states it (`X-68`, new).
4. **Its guides cover the feature set.** sipx's stop at place, answer and register, while hold,
   transfer, DTMF, playback, recording and coupling all ship and appear only as bullets in
   `does-this-fit.md`. Three of the seven repo examples are never surfaced (`X-69`, new).

One published-material defect: the canonical dispatcher doc example models a detached `tokio::spawn`
with an ignored handle and an `.expect()` — both banned in library code by non-negotiables 3 and 5,
two screens below documentation stating the contract. It is the snippet users copy first (`X-70`,
new).

### Code quality — ahead, with a file-size problem

Sans-I/O core with a coherent dependency direction, per-crate `thiserror` errors with `anyhow`
confined to tests, clippy pedantic with every disabled lint justified inline, and of 1,303
`unwrap`/`expect` occurrences only 19 outside test modules — most of those false positives or
carrying written rationale. **One** TODO/FIXME/HACK across all of `crates/*/src`. At the bar there is
no linter configuration, no race detector in CI, and a single file carries nine TODOs including one
against authentication.

Against that: nine files exceed 1,500 lines, led by `crates/sipx-call/src/call.rs` at 6,560 with
roughly 6,100 of production code bundling hold, transfer, session timers, re-INVITE and ICE restart
(`X-67`, new). `crates/sipx-app/src/config/syntax.rs` hand-rolls a TOML subset in a non-protocol
area. Neither is a defect; both are review-cost.

### Security — ahead by construction, unproven by exposure

sipx's advantages are structural rather than procedural, which is the durable kind: TLS verification
that cannot be disabled and has no insecure flag to find; `sips:` that returns no candidate rather
than inventing a cleartext one; parser bounds ahead of allocation; server-side digest defaulting to
SHA-256 with challenge selection by strength rather than server order, explicitly against the
RFC 8760 §3 downgrade; HMAC-keyed nonces with a bounded replay window; CRLF injection prevented by a
fallible constructor with no bypass; SRTP that authenticates before it decrypts and before the
replay window updates; `subtle::ConstantTimeEq` on every secret path; a cryptographic branch;
`unsafe_code = "forbid"` with zero unsafe blocks; and cargo-deny in CI with one documented,
feature-gated exception.

Every one of those is a property the bar either lacks or holds only by convention. The bar has
carried, and fixed, three High-severity remote denial-of-service advisories in the classes `X-64`
addresses, and ships a media path whose peer-fingerprint verification does not reject a mismatch.

**The honest counterweight.** sipx has no published advisory history and no third-party audit. That
is absence of evidence, not evidence of absence, and a stack that has never been attacked in public
has not demonstrated the property it designs for. This is the security face of the adoption-maturity
gap and it closes the same way. Separately, `forbid(unsafe)` is a workspace lint that does not reach
the dependency graph: the optional `dtls` feature pulls in OpenSSL and `opus` a C shim, both
reintroducing memory-unsafe code. Both are off by default, which is the right call, and neither is
covered by the workspace's own guarantee.

### Adoption maturity — behind

The bar has a stable major version, years of releases, and third-party production use. sipx has
eight days of history, 180 of 197 stories done, 7 of 7 alpha predicates met, 3 of 5 beta
announcement predicates met, no published crates, and no external user. The roadmap's own words:
"We stop at the alpha deliberately… the API has not yet been used by anyone outside this repository."

Nothing in this review shortens that distance. `A-10`, `A-11` and `A-12` are the path.

## Work derived from this review

Four epics, nine stories. Beta-1 items are cheap, published, or both; follow-ups are real
engineering that falsifies nothing currently claimed.

| Story | Epic | Wave | Why |
|---|---|---|---|
| `X-64` pin the malformed-input refusals with named tests | `input-hardening` | **beta-1** | three recurring classes held by design, pinned by nothing |
| `X-65` assert the branch and tag RNG is cryptographic | `input-hardening` | **beta-1** | spec requires it; nothing detects its loss |
| `X-70` make the doc examples model the rules the workspace enforces | `docs-depth` | **beta-1** | published code contradicts the stated contract |
| `X-68` explain the layering on the public site | `docs-depth` | **beta-1** | the central design property is undocumented publicly |
| `X-66` measure coverage and publish the number | `conformance` | follow-up | no measurement exists; generated, never transcribed |
| `S-35` accept an inbound subscription from a socket | `event-reachability` | follow-up | the notifier has no caller; unblocks `S-24` |
| `X-69` guide every shipped call verb | `docs-depth` | follow-up | shipped work is indistinguishable from unbuilt work |
| `M-41` negotiate AEAD SRTP protection profiles | `media-security-profiles` | follow-up | RFC 7714; AEAD-only peers cannot negotiate at all |
| `X-67` split the call module along its seams | `depth` | follow-up | 6,560 lines, five concerns, pure-move refactor |

Already in flight and not re-filed: `A-10` and `A-11` (publication — they close the broken install
path), `A-12` (announcement — it closes the stale site), `C-6` (bridge and conference reachable from
a call), `C-7` (off-media coupling), `M-24` (relayed candidate), `S-24` (registration event package,
which `S-35` unblocks).

## What this review does not establish

- **It is not an interoperability campaign.** No claim here rests on a live call against another
  implementation. `T-13` and the interop matrix carry that.
- **It is not an audit.** The security section reports properties held by construction and reviewed
  in source. An adversary was not engaged, and no conclusion should be read as one having been.
- **It does not measure test quality**, only presence and mechanism — the limit `docs/maturity.md`
  already states, and which `X-66` bounds without removing.
- **It does not revisit scope.** Proxy, registrar-server and PBX roles are out of scope for this
  repository by an existing decision, and their absence is not a finding.
