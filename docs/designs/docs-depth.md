# Design: documentation depth

**Status:** proposed · **Pillar:** Build · **Epic:** `docs-depth` · **Stories:** X-68, X-69

## Why

The published site (`website/`, `X-11`–`X-13`) is strong where it is machine-checked: a CLI
reference diffed against the built binary's own `--help`, four guide samples inlined byte-exactly
from example files CI compiles, a generated RFC compliance table, and a build that fails on a dead
link, a dead anchor, a duplicate route or any warning at all. Nothing can rot silently.

What it does not do is *teach*. A 2026-08-04 capability review against the external bar found two
specific shapes of absence, and neither is a rot problem — both are content that was never written:

1. **No concepts page and effectively no diagrams.** sipx's central design property is the sans-I/O
   layering — parser and SDP hold no socket and read no clock, drivers own I/O, `sipx-call`
   composes them (`AGENTS.md` non-negotiable 2). That property is the reason the core is fuzzable,
   deterministic under a virtual clock, and testable without a network. It is stated in `AGENTS.md`,
   in crate `lib.rs` headers and in specs — all of which are for contributors. A prospective user
   reads the site, and the site never explains it. There is one Mermaid diagram in the whole corpus.

2. **Guide coverage stops at three verbs** — place, answer, register — while hold/resume, blind and
   attended transfer, DTMF send and collect, playback, recording, and two-leg coupling are all
   shipped and reachable from `Call`. They appear only as bullets in `does-this-fit.md`. Three of
   the seven repo examples are never surfaced on the site at all. A reader cannot tell from the
   guides that the feature exists, which makes shipped work indistinguishable from unbuilt work.

The evaluated bar teaches the protocol itself as well, on the theory that a developer new to
telephony lands there first. That is real and worth matching in part, but it is a much larger
commitment than either item above and is deliberately scoped out of this epic; see Alternatives.

## Approach

- **One concepts page**, in the Start section, that explains the layering and *why* it is shaped
  that way: what sans-I/O buys (fuzzing, virtual time, no runtime in the core), where the seam
  between protocol and driver falls, and which crate a reader should reach for. It carries the
  diagram — a Mermaid layer diagram, since Mermaid is already enabled and renders in both themes
  without an asset pipeline.
- **A guide per shipped call verb not yet covered**, each following the established pattern rather
  than inventing one: prose, then a sample inlined by `sync-website.py` from a real example file
  under `crates/*/examples/`, so the guide's code is compiled by CI like the existing four. Where
  no example file exists yet, the story writes one — that is the cost, and it is also how the
  sample stays true.
- Surfacing the three unsurfaced examples falls out of the same work rather than being separate.
- Every page added here is subject to the existing `build-docs.sh` contract with no exceptions
  added to `WARNING_EXCEPTIONS`.

## Alternatives considered

- **Write protocol-teaching material (what SIP is, how a dialog works, RTP basics).** Deferred, not
  rejected. It is the largest content gap against the external bar, but it is general telephony
  education rather than sipx documentation, it dates slowly but broadly, and it competes for the
  same effort as making shipped features discoverable. Making shipped features discoverable wins
  first because a user who cannot find `transfer` is lost regardless of how well SIP was explained.
- **Document the extra verbs in the API reference only.** Rejected: rustdoc already documents every
  public item — `missing_docs` is enforced — and it has not made the features findable. Discovery
  is the guide layer's job.
- **Hand-write the samples in Markdown instead of inlining example files.** Rejected outright; it
  is exactly the rot the inline check exists to prevent.
- **Enable docs versioning** so a reader of an installed release sees that release's docs. Real
  problem, wrong epic — it belongs with distribution (`A-10`, `A-11`), and while the API is
  explicitly not frozen, tracking `main` is the honest default.

## Risks and open questions

- One example file per verb grows `cargo build --examples`, which the docs build already runs, and
  every example is compiled on every gate run. If the count grows enough to be felt, the story says
  so rather than quietly dropping examples from the site.
- A concepts page states architecture in prose, and prose drifts from code with nothing to catch it
  — the failure `check-pool-key.py` and the compliance generator exist to prevent. The mitigation
  is to keep the page's claims at the level the non-negotiables already pin (`sipx-sip` and
  `sipx-sdp` gain no runtime, socket or clock read), so a violation fails review against
  `AGENTS.md` rather than only against a page nobody diffs.
