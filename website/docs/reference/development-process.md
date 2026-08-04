---
title: How sipx is built
description: The specifications, failing-first tests, generated measurements, and release gate behind sipx.
---

# How sipx is built

sipx is being built from protocol boundaries inward, with evidence attached to each claim. The
method is meant to make difficult SIP behavior reproducible: bytes and fired timers enter pure
state machines, asynchronous drivers perform I/O above them, and a shell command checks that the
two still agree.

This page describes the engineering method, not the current feature list. For the product surface,
use the live [Rust library guide](../guides/as-a-library.md), [CLI reference](cli.md),
[security boundaries](security.md), and [RFC compliance report](compliance.md). Those pages are
checked closer to the code and are the authority when this account and a capability claim appear
to disagree.

## The path from an RFC to running code

```mermaid
flowchart LR
    R[Primary RFC text] --> S[Repository specification]
    S --> V[Concrete vectors and state tables]
    V --> F[Failing-first behavioral test]
    F --> I[Smallest implementation change]
    I --> G[One release gate]
    G --> M[Generated public evidence]
```

The order matters. A non-trivial subsystem starts with a specification under
[`docs/specs/`](https://github.com/codewandler/sipx/tree/main/docs/specs). It records the normative
RFC sections, types, state transitions, timer inputs, and byte-level vectors before an
implementation decides them accidentally. Design records explain why a boundary has its shape;
specifications say what behavior that boundary must provide.

Development has moved through four broad layers. First came the wire and state-machine boundary:
parsing, validation, transactions, and offer/answer without sockets or clock reads. Transport,
registration, call, and media drivers then wrapped those deterministic cores. A complete endpoint
and diagnostic command connected the layers into behavior observable outside a Rust test. The
beta's final engineering phase attaches release evidence: run that endpoint through its complete
shell matrix, check the claimed transports with independent peers, reproduce the packages a user
will install, and make the public pages match the proven surface. The
[roadmap](https://github.com/codewandler/sipx/blob/main/docs/roadmap.md) carries the detailed history
and current boundary.

The SIP parser is a compact example:

1. The [parser specification](https://github.com/codewandler/sipx/blob/main/docs/specs/sip-parser.md)
   defines datagram and stream framing and classifies the messages imported from RFC 4475.
2. The [corpus tests](https://github.com/codewandler/sipx/blob/main/crates/sipx-sip/tests/rfc4475_corpus.rs)
   turn those vectors into observations: valid messages round-trip byte for byte, invalid messages
   fail at the specified layer, and stream framing is exercised with every possible split point.
3. A new behavior is first expressed as a test that fails for the missing or wrong behavior. Only
   then does the [parser implementation](https://github.com/codewandler/sipx/blob/main/crates/sipx-sip/src/parser.rs)
   change. The red run is evidence that the test can detect the defect; the later green run is
   evidence that the implementation satisfies that observation.
4. The RFC corpus is recovered from the RFC's own archive, and the gate compares the checked-in
   bytes with that source. A locally edited fixture therefore cannot silently redefine success.

The same shape applies above parsing. A call or media feature gets externally observable vectors:
which event occurs, which packet is emitted, which timer fires, what shuts down, and what must remain
silent. Real sockets belong in the transport and media drivers; the SIP and SDP cores receive bytes
and fired-timer inputs. That separation lets tests choose hostile timing directly instead of hoping
the operating system happens to produce it.

Browser-compatible audio exposed the cost of keeping that order at a product boundary. The work
was split into a profile before a runtime: first the exact SDP and fail-closed downgrade rules,
then one owner for ICE, DTLS, SRTP and multiplexed RTCP on the nominated component, and finally an
independent native-browser proof in both SIP roles. The browser page is intentionally small. It
uses the browser's own peer-connection, WebSocket, audio and statistics interfaces and contains no
sipx SDP, ICE, DTLS, RTP or codec implementation that could agree with itself.

That final proof was built adversarially before it was allowed to make a compatibility claim. Its
self-test kills an owner with a forking grandchild, reverses every structured media fact, supplies
malformed and oversized evidence, removes one call role, and presents the wrong WSS public-key pin.
Only the real proof may promote that infrastructure into evidence: Opus must be non-silent in both
directions, the selected component and SRTP profile must come from runtime statistics, dialog
teardown must finish, and fingerprint, nomination and weaker-media refusals must each reach the
layer they name. A green harness self-test by itself says only that the measuring instrument fails
closed.

The public [native-browser proof](browser-audio-proof.md) describes the two roles, independent
runtime facts, negative cases, and the boundary that result does not widen.

## Work survives the session that discovered it

Every unit of work is a Markdown story in the repository. A story carries its goal, acceptance
observations, priority, status, relevant design, and progress notes. A discovered defect becomes
its own story rather than an aside in a test or a private task list.

The generated board reads that frontmatter. A later work session can therefore recover both the
next action and the reason it exists without reconstructing an earlier conversation. Closing a story means satisfying its
acceptance observations, recording a release-facing change when appropriate, regenerating the
board, and passing the same gate used by continuous integration.

This is deliberately more than issue accounting. Stories can declare which release predicate they
affect. Filing a defect against a predicate reopens that predicate automatically; there is no
second hand-maintained blocker list for somebody to remember.

## Measurements are generated, not announced by prose

Three generated artifacts answer three different questions:

- The [RFC registry](https://github.com/codewandler/sipx/blob/main/docs/rfc/registry.toml) feeds the
  public [compliance report](compliance.md). An implementation or partial-support entry must cite
  Rust source in a workspace crate, so a design paragraph alone cannot claim shipped behavior.
- The [maturity report](https://github.com/codewandler/sipx/blob/main/docs/maturity.md) derives
  release predicates from story frontmatter and repository history. The public beta threshold is
  all-or-nothing: alpha integrity still holds, the diagnostic phone is proven from a shell, claimed
  transports have independent-peer evidence, registry distribution is reproduced, and the public
  adoption surface is current.
- The [roadmap](https://github.com/codewandler/sipx/blob/main/docs/roadmap.md) explains why those
  predicates are the threshold. It intentionally does not turn an RFC percentage or a test count
  into a maturity score.

Generation makes drift visible. A story that names an unknown predicate, a compliance entry whose
evidence file vanished, or a checked-in report that no longer matches its sources fails the gate.
It does not become stale green prose.

## One gate defines a candidate

[`./scripts/gate.py`](https://github.com/codewandler/sipx/blob/main/scripts/gate.py) is the local
release check and the source of truth for the checks mirrored in continuous integration. Among
other things it verifies formatting, lints, tests, examples, the minimum supported Rust version,
feature combinations, provenance, RFC corpus bytes, generated reports, the application surface,
and this documentation site's links. The gate checks its own agreement with the CI workflow, so a
new CI job cannot quietly become a check contributors never run locally.

The core rules sit underneath that automation:

- `sipx-sip` and `sipx-sdp` do no I/O and read no clock.
- Workspace code forbids `unsafe`; malformed network input returns typed errors.
- Background work is bounded, cancellation-safe, and joined on shutdown.
- Public rationale cites RFCs or sipx's own specifications.
- If behavior cannot be asserted from a shell or deterministic harness, it is not considered
  finished.

## What this evidence does not prove

sipx remains pre-1.0. Breaking API changes are still permitted before 1.0 and are recorded in the
[changelog](https://github.com/codewandler/sipx/blob/main/CHANGELOG.md). The website is built from
`main`, so it may describe work newer than the latest tag; install a named release when
reproducibility matters.

A green gate proves only the observations it runs. It cannot prove that no unknown defect exists,
that every possible deployment topology has been exercised, or that a story was correctly attached
to every predicate it should affect. Independent-peer tests widen the evidence, but they are still
a finite set of peers and scenarios.

Test count is intentionally not a maturity score: a test can be present yet fail to detect the
condition named by its title. RFC coverage is not a maturity percentage either; RFCs differ in
scope, role, and reachability, and partial support is not a fraction of complete support. The
reports preserve those limits instead of compressing them into a reassuring number.

The generated beta predicates and reproduced registry artifacts define release readiness. They do
not promise a separate public announcement, and they are not an API-stability promise. Stable
`1.0.0` remains a separate decision.
