# Release workflow specification

**Status:** normative target · **Story:** `A-12` · **Boundary:** GitHub Actions orchestration

## 1. Scope

This specification defines the repository-hosted orchestration for the current public beta
publication. It does not replace `scripts/release.py`: the helper remains the authority for package
order, normalized bytes, registry visibility, partial-publication recovery and the exact consumer
proof. The workflow supplies the approved environment, exact annotated tag, registry credential,
bounded repetition, Pages evidence and one GitHub prerelease around that helper. The GitHub Release
is part of the release record, not a broader publicity campaign; the workflow MUST NOT post broader
publicity to issues, pull requests, webhooks or other services.

Normative words **MUST**, **MUST NOT**, **SHOULD** and **MAY** are used as in RFC 2119 and RFC 8174.

## 2. Entry and authority

The ordinary workflow MAY start from a pushed version tag or from `workflow_dispatch`. A manual run MUST be
dispatched with that existing tag selected as the GitHub ref and MUST require the same exact tag as
a redundant confirmation input. Selecting a branch and supplying an arbitrary tag input is not a
release entry: checkout alone cannot change the event or workflow-source authority recorded by
GitHub. Both entries MUST pass through the `release` environment, whose repository or organization
policy supplies required approval. Concurrent runs for one tag MUST serialize and MUST NOT cancel
an in-progress publication.

One separate `workflow_dispatch` recovery workflow MAY operate after a partial publication only
under §4's `recovering` state. It MUST run in the same protected `release` environment and serialize
with the ordinary workflow by release tag. Its exact `main` workflow commit is controller authority,
not package authority: package bytes MUST come from a separate clean checkout of the immutable tag.
The required tag and failed ordinary-run ID are explicit inputs and are both validated before a
registry probe or write.

One historical first-publication replay MAY operate only for `v1.0.0-beta.1` under §4's
`replaying-first-publication` state. It is not parameterized: its annotated tag object, peeled
commit and failed ordinary-run ID are constants held by structural tests. It uses a distinct helper
authorization and a distinct manual-only workflow from exact `main`; neither ordinary publication
nor partial recovery gains authority from an empty registry. The replay MUST rerun the immutable
tag's complete gate and locked rehearsal successfully before the Cargo credential is exposed.

The Cargo credential is the environment or repository secret named `CARGO_REGISTRY_TOKEN`, exposed
only to the presence check and publication step. An empty value MUST fail before the gate or any
registry probe. The gate/publication job has read-only repository contents permission and does not
persist the checkout credential. A dependent GitHub-prerelease job alone has repository contents
write permission; its checkout also does not persist a credential, and its GitHub token is exposed
only to the create-or-verify step. No credential is printed.

The external provenance denylist is the repository or organization secret named
`SIPX_DENYLIST`. It MUST be exposed only to the complete-gate step so that the release gate makes
the same mandatory provenance claim as ordinary CI. An absent denylist MUST stop the release
before package rehearsal or publication; its contents MUST NOT be stored in this repository.

## 3. Immutable input

Before running the release helper, the workflow MUST establish all of these facts:

1. `GITHUB_REF`, `GITHUB_REF_TYPE` and `GITHUB_REF_NAME` describe the selected version tag, and a
   manual confirmation input exactly equals that selected tag;
2. the requested value has the version-tag shape `v<semver>` and equals `v` plus the workspace
   version;
3. checkout `HEAD`, `GITHUB_SHA` and `GITHUB_WORKFLOW_SHA` are the commit peeled from that tag, and
   `GITHUB_WORKFLOW_REF` names this workflow at that tag;
4. the ref is an annotated tag, not a lightweight tag;
5. that is the only tag pointing at `HEAD`;
6. the checkout is clean, including untracked files;
7. the commit is contained in `origin/main`; and
8. `docs/releases/<version>.md` exists as the reviewed release record.

The checkout fetches full history and tags. A tag push alone does not prove the corresponding main
push or Pages deployment, so ancestry is necessary but not sufficient for a verified beta cut.

## 4. State machine and bounds

| State | Input | Required output | Bound |
|---|---|---|---|
| approved | tag push or tag-selected manual resume with matching confirmation | one approved `release` job | one serialized job per tag |
| validated | clean full-history checkout | immutable tag facts from §3 | job timeout |
| gated | validated tag | full local gate and locked publication dry-run pass | job timeout plus helper command bounds |
| publishing | gated tag and non-empty Cargo secret | one dependency-ready frontier becomes visible | helper visibility and command bounds |
| publishing | visible frontier | rerun helper on unchanged tag | public-package count plus one invocations total |
| recovering | failed ordinary run for this tag whose gate and rehearsal passed and publication step failed | fixed controller reproduces every visible checksum, then advances at most one unchanged frontier per bounded invocation | protected job timeout and public-package count plus one invocations total |
| replaying-first-publication | exact beta.1 failed run whose tag validation passed, gate failed and every later release step skipped | fixed controller reruns the immutable tag's complete gate and locked rehearsal, then may publish the empty registry under beta.1-only authority | protected job timeout and public-package count plus one invocations total |
| distributed | every package visible | exact registry consumer and Opus-enabled installed CLI loopback pass | helper consumer and visibility bounds |
| documented | distributed | successful `ci.yml` Pages deployment job whose `head_sha` equals the tag commit; public guide and API URLs answer | finite HTTP retries and job timeout |
| released | documented | one non-draft GitHub prerelease for the exact tag and reviewed notes | one create, or exact verification of an existing release |

The workflow MUST call `scripts/release.py --publish` rather than `cargo publish` directly. It MUST
provide both the exact tag confirmation and the CI authorization `<tag>@<full commit SHA>` derived
from the validated tag. Each successful helper call either makes a frontier visible or reports
that all packages are visible. The loop limit is derived from the number of public packages plus
one final observation; exhausting it is failure. A rerun is safe because the helper compares
already-visible registry checksums with the tagged archives before advancing.

The recovery workflow MUST verify the named failed run through the Actions API before exposing the
Cargo credential to publication. The run's repository, original workflow path, release SHA and
conclusion must agree; the `Run the complete release gate` and
`Rehearse the locked registry packages` steps must be successful and
`Publish dependency-ready frontiers under a finite bound` must be failed. Recovery tooling and
release bytes live in separate checkouts. The helper receives distinct recovery authorization
binding the workflow source SHA, exact tag and release SHA, failed run ID and current run identity.
The ordinary tag-source authorization remains unchanged.

The beta.1 replay workflow MUST verify failed run `30906820031` through the Actions API before any
release secret is exposed. It MUST require that immutable-tag validation succeeded, the gate failed,
and rehearsal, publication, consumer, Pages and GitHub-prerelease work were skipped. Its separate
release checkout MUST be annotated tag object `b0bcadcc2a69a5824ec4a9549f7800c88c4f13fa`, peeling to
`3ab81709c7a235831638c62eba5fe73ce9eb7773`; its controller is the exact `main` workflow commit.
After the current complete gate and rehearsal pass, the distinct helper authority may classify zero
visible packages and begin one bounded dependency-ready frontier. No other tag, commit, run or
workflow may exercise that exception. Once a frontier exists, the ordinary archive-reproduction and
checksum rules apply unchanged.

## 5. Documentation and GitHub prerelease proof

The successful Pages evidence MUST come from the push-triggered `ci.yml` run for `main` whose
`head_sha` equals the peeled release tag commit, and that run's `deploy docs site` job MUST have
conclusion `success`. The workflow then probes both the public getting-started guide and the
generated `sipx-call` API index. A tag workflow or HTTP 200 by itself cannot establish the commit
that supplied the page.

The late beta.1 replay MUST NOT roll the live site backward from beta.2. Instead it verifies the
unexpired `github-pages` artifact from exact-SHA CI run `30906258443`: the run and its deployment job
must be successful and bound to the beta.1 commit, the artifact identity must be exact, its
getting-started page must name beta.1 and its generated `sipx-call` API index must exist. The current
public guide and API are then probed separately and MUST continue to name and serve beta.2.

Only after registry consumer and Pages proofs pass may the workflow create the GitHub Release. It
MUST use the existing tag (`--verify-tag`), mark the release as a prerelease and take its body from
`docs/releases/<version>.md`. A resume that finds the release already present MUST verify that it is
non-draft, prerelease, names the same tag and has the reviewed body; it MUST NOT publish a second
release or silently rewrite the first one. This repository-native prerelease is part of the beta
cut. Broader publicity remains hypothetical and requires separate explicit authorization; the
workflow MUST NOT post broader publicity.

The beta.1 replay takes its reviewed body from
`docs/releases/1.0.0-beta.1-replay.md` at the exact controller commit. The body MUST lead with the
superseded status, name beta.2 as current and disclose the documentation defect corrected by
`X-70`. The replayed Release remains a non-draft prerelease for the immutable beta.1 tag and is
never selected as latest. Its later creation time does not make it the recommended release.

## 6. Static vectors

| ID | Mutation | Required result |
|---|---|---|
| `RWF-1` | remove tag-push or tag-selected manual entry, or let its confirmation differ | static check fails |
| `RWF-2` | remove environment, serialization, finite timeout or split read/write authority | static check fails |
| `RWF-3` | rename or stop checking the Cargo secret, or remove the gate's provenance-secret input | static check fails |
| `RWF-4` | accept a lightweight/dirty/non-main tag | static check fails |
| `RWF-5` | call Cargo publication directly or omit exact helper tag/commit authorization | static check fails |
| `RWF-6` | make frontier repetition unbounded or omit exact consumer proof | static check fails |
| `RWF-7` | accept Pages without matching `head_sha`, deploy job and two probes | static check fails |
| `RWF-8` | make the GitHub prerelease non-idempotent, unverified or inline-noted; or add broader posting | static check fails |
| `RWF-9` | recovery omits protected environment, separate checkouts, failed-run step evidence, exact controller/tag/SHA binding, visible-byte proof or bounded frontier loop | static check fails |
| `RWF-10` | beta.1 replay accepts another tag/run/object, exposes a secret before current gate and rehearsal, changes recovery's zero-visible refusal, omits historical Pages artifact proof or presents beta.1 as latest | static check fails |

These are structural tests, not evidence that GitHub or crates.io accepted a write. Actual release
acceptance remains the run records and registry bytes produced only after explicit authorization.
