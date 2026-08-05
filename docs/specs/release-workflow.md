# Release workflow specification

**Status:** normative target · **Story:** `A-12` · **Boundary:** GitHub Actions orchestration

## 1. Scope

This specification defines the repository-hosted orchestration for a public
publication. It does not replace `scripts/release.py`: the helper remains the authority for package
order, normalized bytes, registry visibility, partial-publication recovery and the exact consumer
proof. The workflow supplies the approved environment, exact annotated tag, registry credential,
bounded repetition, Pages evidence and one GitHub Release around that helper. The GitHub Release
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
| gated | validated tag | full local gate and locked publication dry-run pass, including its staged transport/testkit clean-consumer proof | job timeout plus helper command bounds |
| publishing | gated tag and non-empty Cargo secret | one dependency-ready frontier becomes visible | helper visibility and command bounds |
| publishing | visible frontier | rerun helper on unchanged tag | public-package count plus one invocations total |
| recovering | failed ordinary run for this tag whose gate and rehearsal passed and publication step failed | fixed controller reproduces every visible checksum, then advances at most one unchanged frontier per bounded invocation | protected job timeout and public-package count plus one invocations total |
| distributed | every package visible | exact registry consumer and Opus-enabled installed CLI loopback pass | helper consumer and visibility bounds |
| documented | distributed | successful `ci.yml` Pages deployment job whose `head_sha` equals the tag commit; public guide and API URLs answer | finite HTTP retries and job timeout |
| released | documented and portable artifacts aggregated | one non-draft GitHub Release for the exact tag, reviewed notes and byte-verified assets | one create, or exact verification of an existing release |

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

## 5. Documentation, artifacts and GitHub Release proof

The successful Pages evidence MUST come from the push-triggered `ci.yml` run for `main` whose
`head_sha` equals the peeled release tag commit, and that run's `deploy docs site` job MUST have
conclusion `success`. The workflow then probes both the public getting-started guide and the
generated `sipx-call` API index. A tag workflow or HTTP 200 by itself cannot establish the commit
that supplied the page.

Only after registry consumer, Pages and the portable-artifact aggregation proofs pass may the
workflow create the GitHub Release. It MUST use the existing tag (`--verify-tag`) and take its body
from `docs/releases/<version>.md`. A version containing a prerelease suffix creates a prerelease; a
stable version creates a non-prerelease release. A resume that finds the release already present
MUST verify that it is non-draft, has the expected release kind, names the same tag and has the
reviewed body. Existing asset bytes MUST be compared, missing assets MAY be added, and no existing
asset may be overwritten or deleted. It MUST NOT publish a second release or silently rewrite the
first one. Broader publicity remains hypothetical and requires separate explicit authorization;
the workflow MUST NOT post broader publicity.

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
| `RWF-8` | make the GitHub Release kind or asset set non-idempotent, unverified or inline-noted; or add broader posting | static check fails |
| `RWF-9` | recovery omits protected environment, separate checkouts, failed-run step evidence, exact controller/tag/SHA binding, visible-byte proof or bounded frontier loop | static check fails |

These are structural tests, not evidence that GitHub or crates.io accepted a write. Actual release
acceptance remains the run records and registry bytes produced only after explicit authorization.
