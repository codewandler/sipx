# Registry release rehearsal

This specification defines the local release helper used before the public prerelease. It does not
define the version policy or the announcement gate; those live in
[`roadmap.md`](../roadmap.md). Its concern is narrower: prove that Cargo can turn the release commit
into registry packages, in an order that never assumes an unpublished workspace dependency exists.

## 1. Inputs and modes

The helper reads the workspace and package records from `cargo metadata --locked --no-deps`. Package
names, versions, publication policy and dependency edges therefore come from the same manifests Cargo
will package. Git supplies the checkout state and the exact annotated tag at `HEAD`.

There are four release modes, one recovery-authorized form of `publish`, and one dirty-candidate
diagnostic:

| Mode | Registry writes | Required checkout | Action |
|---|---:|---|---|
| `check` (default) | no | clean | validate the release graph and print dependency order |
| `dry-run` | no | clean | stage and compile the RTP echo package pair in a clean consumer, then run one locked Cargo workspace dry-run covering every public package |
| `publish` | yes | clean, at the exact annotated `v<workspace-version>` tag | publish one dependency-ready frontier to crates.io after exact typed confirmation; CI additionally needs the GitHub tag/commit authorization below |
| `verify-consumer` | no | clean, at the exact annotated release tag | build exact crates.io crates, install the exact CLI, and run one bounded loopback call |
| `inspect-dirty-contents` | no | may be dirty | diagnose listings and normalized archives without declaring a release candidate |

The workspace form is deliberate: Cargo stages the packages in a temporary registry in dependency
order, so dependants are verified before this new version exists on the public registry. Eleven
independent `-p` commands would make the first dependant fail on the very absence the rehearsal is
supposed to precede. The helper still prints its independently derived dependency order, which is the
order explicit publication follows.

The helper never creates or moves a tag and has no announcement integration. `CI` may run `check` or
`dry-run`. Publication always refuses a lightweight tag and refuses unless `--confirm-publish` is
exactly the annotated release tag, so selecting the mode alone is not authority to write to the
registry. Every dry-run and publication command names `--registry crates-io`; a private default
registry cannot receive a confirmed release by surprise.

### 1.1 GitHub Actions publication authority

A generic CI process still cannot publish. A GitHub Actions run may publish only when every fact
below agrees:

- `CI` and `GITHUB_ACTIONS` are exactly `true`, the server is `https://github.com`, and the
  repository is `codewandler/sipx`;
- the event is either a tag `push`, which starts the first frontier, or `workflow_dispatch`, which
  resumes later frontiers after registry propagation;
- `GITHUB_REF`, `GITHUB_REF_TYPE` and `GITHUB_REF_NAME` identify the exact
  `refs/tags/v<workspace-version>` tag. A manual dispatch selected on a branch is not a release run;
- `GITHUB_SHA` and `GITHUB_WORKFLOW_SHA` are the same full 40-character object ID as the checked-out
  `HEAD`, and `GITHUB_WORKFLOW_REF` is exactly
  `codewandler/sipx/.github/workflows/crates-io.yml@refs/tags/v<workspace-version>`. Thus neither an
  unrelated workflow, nor the release workflow from `main` operating on a tag, nor a tag-named run
  operating on arbitrary branch bytes has authority;
- the run ID and attempt are positive integers, and `CARGO_REGISTRY_TOKEN` is present;
- in addition to `--confirm-publish v<version>`, the invocation supplies
  `--authorize-ci-publish v<version>@<full-HEAD>`. GitHub context alone is not a write instruction,
  and the tag-only confirmation alone cannot authorize a different commit.

The token value is never printed or accepted as a command argument. The workflow should obtain it
from the protected `CARGO_REGISTRY_TOKEN` secret. Each authorized invocation retains the ordinary
frontier, visibility and byte-identity rules below and advances at most one frontier. Consequently a
tag push can begin publication and an explicitly tag-selected manual dispatch of `crates-io.yml` can
resume it without making another tagged workflow, branch dispatch, pull request, scheduled job,
generic CI runner or mismatched workflow an upload path. Outside CI, `--authorize-ci-publish` is
refused rather than silently ignored.

The workflow invocation is derived entirely from the GitHub-provided tag and commit after those
facts have been checked:

```sh
./scripts/release.py --publish \
  --confirm-publish "$GITHUB_REF_NAME" \
  --authorize-ci-publish "$GITHUB_REF_NAME@$GITHUB_SHA"
```

A manual resume selects the tag as the workflow ref; selecting `main` and passing a tag-shaped input
does not satisfy the contract.

### 1.2 Protected controller recovery

An immutable tag can outlive a defect in the controller stored at that tag. After a release run has
passed the complete gate and locked rehearsal, published at least one frontier, and then failed in
the publication step, a separate protected recovery workflow MAY use fixed controller tooling from
its exact `main` workflow commit against a separate clean checkout of the release tag. This is not a
second first-publication path. Before any write it MUST establish all of these facts:

- the supplied failed-run ID names `crates-io.yml` at the same release commit, with conclusion
  `failure`; its complete-gate and locked-rehearsal steps succeeded and its publication step failed;
- the release checkout is the clean, unique annotated version tag, contained in `main`, while the
  controller checkout is the exact workflow-source commit; neither checkout contributes files to
  the other;
- every visible exact package is reproduced from the release checkout and has the same canonical
  crates.io SHA-256 checksum before a missing frontier is dispatched; and
- CI recovery authority names the recovery workflow, repository, workflow-dispatch event, protected
  run identity, exact tag, release SHA, failed-run ID and controller SHA. Ordinary branch CI and an
  unprotected or differently sourced workflow remain unable to publish.

The recovery workflow retains the ordinary frontier, timeout, consumer, Pages and GitHub-prerelease
proofs. It obtains no authority to change package bytes: a mismatch stops before another upload, and
a required package-content change still requires a new version.

`verify-consumer` first polls all exact versions under the same finite visibility rule. It creates a
temporary Cargo project whose dependencies use `=<workspace-version>` and name `registry =
"crates-io"` explicitly. Its isolated Cargo home removes inherited source replacements, registry
overrides and both Cargo spellings of an external target path; its lockfile must resolve every sipx
library from Cargo's canonical crates.io source. It then installs the exact `sipx-cli` version from
the explicitly named crates.io registry, with the `opus` feature and Cargo's lockfile enforcement,
under a temporary installation root. This exact-feature install proves that the
normalized registry manifests preserve the optional native-codec path; a workspace build cannot
make that claim. The installed binary starts an answerer on an operating-system-
selected loopback port, reports one complete newline-terminated readiness record under a byte-read
deadline, completes one bounded dial, and exits both processes cleanly.

Every subprocess has a failure bound and an owned process group. The group leader remains unreaped
until its output is drained and every descendant has been terminated, so a compiler or build script
cannot survive success or timeout and a reused process ID is never signalled. `SIGINT` and `SIGTERM`
first terminate and join every registered group. Nothing from this mode writes to the repository or
the registry.

## 2. Public package graph

A package is public unless Cargo metadata says its registry allow-list is empty (`publish = false`).
Normal, optional and build dependencies on another workspace member are graph edges; development
dependencies are not part of the published manifest and do not order publication. A deterministic
topological sort uses the package name to break ties. A cycle is an error.

Every workspace member must have the workspace version. Every public-to-workspace dependency must:

1. name that same version as its Cargo requirement,
2. point to a member inside the workspace when it has a path, and
3. target another public package.

Any Git dependency in a public package is refused. A path dependency outside the workspace, a path
dependency without a registry version, or a normal dependency on an unpublished workspace package is
also refused: Cargo could otherwise build locally and produce a package no registry consumer can
resolve. The supported downstream call harness makes `sipx-testkit` part of the public graph; its
development-only uses by other workspace crates do not create publication-order edges.

## 3. Package metadata and bytes

Every public package must declare a non-empty SPDX license expression and resolve a README that
exists. Each license identifier in the workspace expression must have the corresponding root license
file. `cargo publish --dry-run --locked` remains the byte-level authority: it builds the `.crate`
archive, verifies the normalized manifest and compiles its contents without workspace paths.

Before that clean-candidate rehearsal is possible, `--inspect-dirty-contents` may audit a dirty working tree
without making it releasable. For every public package it runs Cargo's locked, allow-dirty package
listing, then creates the same unverified local `.crate` archive solely to read the normalized
manifest. The listing must contain `Cargo.toml`, `Cargo.toml.orig` and the declared README; every name
must be a relative path that stays inside the package. The normalized manifest must retain the exact
package version, README and SPDX license expression, must contain no path or Git dependency, and must
not turn an unpublished workspace package into a registry dependency. Local archives are deleted
after inspection. This diagnostic does not waive the clean-checkout rule in any release mode and is
not a substitute for `cargo publish --dry-run --locked` compiling the archive.

The local package-set proof covers a dependency-ordered pre-publication case that a single-package
verification cannot: `sipx-testkit` may consume unreleased APIs anywhere in its transitive public
workspace dependency closure at the same version. The helper derives that complete closure from
Cargo metadata and packages it in deterministic publication order through Cargo's temporary
registry. A second clean consumer compiles the RTP echo example copied from the exact staged testkit
archive; its manifest patches every other closure member to its extracted archive and its lockfile
must resolve every member, including `sipx-sip` and `sipx-transport`, from those staged sources. Every
`dry-run` MUST complete this proof before Cargo's workspace rehearsal, so the tag workflow cannot
publish after exercising only the single-package path. `--verify-local-package-set` remains a
focused way to run the same proof while developing a dirty candidate. Both paths are bounded and
temporary and say nothing about registry visibility.

The same example is the manifest-declared proof for the test-product reachability class enforced by
`check-app-surface.py`. That class backs only `sipx-testkit`'s Supported test API: it does not add
the testkit or its dependency closure to the shipped application's production surface. The surface
check proves the example target and import exist; this rehearsal proves the archived bytes resolve
and compile as a clean external consumer.

## 4. Partial registry availability

Publishing is restartable. Before a write, the helper asks Cargo whether each public
`name@workspace-version` is already available. Already available packages are skipped. Among missing
packages, only those whose public workspace dependencies are all available form the ready frontier.
Only Cargo's exact `could not find name@version` result means absent. A timeout, authentication
failure, index failure or any other registry diagnostic aborts the invocation before an upload; an
unavailable registry is not evidence that a version is unpublished.

The first frontier has no prior bytes to compare. Once any package is visible, the helper reproduces
all public `.crate` archives from the clean tagged checkout in a temporary target directory, with no
`--allow-dirty`. It also creates a temporary exact-version project under an isolated Cargo home and
uses a bounded `cargo generate-lockfile` to obtain each already-visible package's canonical
crates.io SHA-256 checksum from a fresh index. Every reproduced visible archive must embed `HEAD` in
Cargo's VCS record and must be clean according to Cargo's encoding: `git.dirty` omitted or boolean
`false` means clean, while boolean `true` means dirty. A present value of another type is malformed
and refused. Its SHA-256 must equal the registry checksum. This check
runs before a later frontier, before `publish` reports an all-visible release, and before
`verify-consumer` installs anything. A mismatch stops before success or another publish command, so
moving the annotated tag or changing bytes cannot make a later frontier or the announcement proof
come from another release commit. The temporary archives, index and lockfile are removed on every
exit path.

One invocation publishes that frontier, then polls every exact uploaded version under a finite
wall-clock bound; each individual registry query is bounded by the smaller of its own limit and the
time remaining. It advances no further in that invocation. A later invocation observes the visible
frontier and advances. A visibility timeout names the still-absent packages and requires visibility
verification before publication resumes. If no missing package is ready, the helper fails and names
the unavailable dependencies. It never guesses that a successful upload is already visible.

## 5. Test vectors

| Vector | Workspace shape or invocation | Required result |
|---|---|---|
| R1 | `core <- call <- cli`, names presented out of order | `core, call, cli` |
| R2 | one package or internal requirement has another version | refuse before Cargo publication |
| R3 | public CLI depends normally on unpublished test support | refuse and name both packages |
| R4 | `core` available; `call` and `cli` missing | only `call` is ready |
| R5 | dirty checkout, detached untagged commit, or wrong tag | `publish` refuses |
| R6 | generic CI, branch dispatch, wrong repository/ref/SHA/workflow, missing token, or missing CI commit authorization | refuse before probing or publishing |
| R7 | default or `dry-run` | command trace contains no `cargo publish` without `--dry-run` |
| R8 | README absent, license absent, Git dependency, or escaping path | refuse before Cargo publication |
| R9 | archive list has `../secret`, omits README, or normalized manifest retains `path` | content inspection refuses |
| R10 | dirty tree uses `--inspect-dirty-contents` | report archive facts, but do not call the release candidate clean |
| R11 | uploaded frontier remains invisible | stop at the finite visibility deadline and name it |
| R12 | exact registry consumer or installed loopback exceeds its bound | stop, clean the process group and retain no temporary project |
| R13 | partial readiness line or descendant process | deadline fires; the complete process group is joined |
| R14 | private default registry, source replacement, or lightweight tag | refuse or isolate it; crates.io and an annotated tag remain explicit |
| R15 | first frontier, matching partial frontier, all-visible state, or moved-tag mismatch | first proceeds without prior evidence; matching bytes resume; all-visible and consumer proofs recheck; mismatch dispatches no upload or install |
| R16 | registry probe times out or reports anything except exact not-found | refuse before treating the package as absent or dispatching an upload |
| R17 | GitHub tag push or tag-selected manual dispatch, exact annotated tag/HEAD/workflow SHA, both confirmations and token | retain the ordinary frontier/checksum rules and permit at most one ready frontier |
| R18 | protected recovery names a failed same-tag release whose gate/rehearsal passed and publication failed, with matching visible bytes | fixed controller may advance one missing frontier; wrong run/workflow/step/commit/controller or byte mismatch dispatches no upload |
| R19 | Cargo VCS record omits `git.dirty`, sets a boolean, or gives a non-boolean value | omitted/false is clean; true is dirty; malformed is refused |
| R20 | local package-set verification derives and stages testkit's complete public workspace dependency closure, then compiles the archived RTP echo example | every exact closure member resolves from staged bytes; never substitute an older registry package or claim publication |
