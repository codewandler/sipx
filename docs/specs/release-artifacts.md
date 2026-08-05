# Stable CLI release artifacts

**Status:** normative target · **Stories:** `A-10`, `P-14` · **Boundary:** release packaging and
GitHub Actions orchestration

## 1. Scope

This specification defines the portable `sipx` command artifacts attached to a stable release. It
does not authorize a release, replace the registry publication contract in
[`release-rehearsal.md`](release-rehearsal.md), or weaken the immutable-tag requirements in
[`release-workflow.md`](release-workflow.md). The artifact jobs consume the same exact annotated
tag and peeled commit as registry publication, and acquire no registry or release-write credential.

The key words **MUST**, **MUST NOT**, **SHOULD** and **MAY** are requirements as described by RFC
2119 and RFC 8174.

## 2. Artifact matrix and names

One release produces exactly these five binary archives:

| Target | Runner architecture | Archive |
|---|---|---|
| `x86_64-unknown-linux-musl` | x86-64 Linux | `.tar.gz` |
| `aarch64-unknown-linux-musl` | Arm64 Linux | `.tar.gz` |
| `x86_64-apple-darwin` | x86-64 macOS | `.tar.gz` |
| `aarch64-apple-darwin` | Arm64 macOS | `.tar.gz` |
| `x86_64-pc-windows-msvc` | x86-64 Windows | `.zip` |

The archive basename is `sipx-<version>-<target>`. It contains one directory of that name with:

- `sipx` or `sipx.exe`;
- `build-manifest.json`;
- `LICENSE-APACHE`; and
- `LICENSE-MIT`.

Each target also produces `sipx-<version>-<target>.spdx.json`. The release carries one
`SHA256SUMS` whose sorted entries name every archive and every SPDX document. No target alias,
moving URL or unversioned asset is a release artifact. A sidecar copy of `build-manifest.json`
travels between the matrix and aggregator as Actions evidence; it is validated against the archive
copy and is not a second release asset.

## 3. Immutable build inputs

The packager MUST receive the workspace version, full release commit, exact Rust target and the
commit timestamp through explicit arguments. The version MUST equal the package version embedded in
the executable, and the commit MUST be forty lowercase hexadecimal digits. The timestamp supplies
`SOURCE_DATE_EPOCH`; wall-clock time MUST NOT enter an archive, manifest or SBOM.

`build-manifest.json` is a closed object carrying:

```text
schema, version, target, release_sha, source_date_epoch, rustc, cargo,
features, binary, binary_sha256, static_linked
```

`features` is the exact sorted Cargo feature list used for the published executable. The v1 matrix
uses no optional features: device audio and the native Opus codec are absent and MUST be stated as
absent rather than inferred from an archive name. `static_linked` is `true` only for the two Linux
musl artifacts and `false` elsewhere.

## 4. Build, linkage and smoke proof

Every executable is built once from the release checkout with:

```text
cargo build --locked --release -p sipx-cli --target <target> --no-default-features
```

The two Linux artifacts MUST be native musl builds. A linkage check MUST inspect the executable
format and refuse an ELF interpreter or dynamic dependency; accepting a filename, target triple or
successful compiler exit is not evidence of static linkage. Running `ldd` MAY add evidence, but an
`ldd` diagnostic alone is not the parser of record.

The native executable on every runner MUST:

1. print exactly `sipx <version>` for `sipx version`;
2. start a bounded JSON answerer on an operating-system-selected loopback port;
3. complete one bounded JSON dial through that answerer; and
4. exit both process groups cleanly after an answered call.

The smoke supervisor owns both process groups, bounds readiness, call completion and output, and
terminates and joins the groups after success, failure, exception or signal. A help command or a
process that merely starts cannot substitute for the call.

The macOS and Windows jobs additionally compile-check `sipx-cli` with `device-audio` enabled. This
check does not put device audio into the published executable. Linux artifacts deliberately omit
device audio and Opus so that their static-link claim does not hide a native shared-library
dependency.

## 5. SPDX software bill of materials

The packager emits SPDX 2.3 JSON for the exact normal dependency closure of `sipx-cli` under the
target and feature selection used to build the executable. It derives that closure from locked
Cargo metadata filtered for the target; it MUST NOT list the whole workspace or silently include an
optional dependency whose feature is off.

The document carries a deterministic namespace bound to version, target and release commit; the
creation time comes from `SOURCE_DATE_EPOCH`. Every package records name, version, declared licence,
source identity and a package URL where available. Registry checksums from `Cargo.lock` are included
when present. Relationships name the document's executable package and every direct normal
dependency edge in the selected closure. Missing identity, a duplicate SPDX identifier, an edge to
an absent package or a dependency graph that does not contain `sipx-cli` is a packaging failure.

## 6. Aggregation and publication

Each matrix job uploads only its own archive, SPDX document and build-manifest sidecar as an Actions
artifact. A separate aggregation job downloads all five, rejects an absent or additional target,
checks each manifest against the release tag and commit, re-hashes the executable from the archive,
validates each SPDX document, and writes the sorted `SHA256SUMS`. The release asset set is exactly
five archives, five SPDX documents and that checksum file.

The GitHub Release is created only after the registry consumer, Pages proof and artifact aggregation
have succeeded. A stable version creates a non-draft, non-prerelease release; a prerelease keeps the
existing prerelease behavior. A retry that finds a release or asset already present compares its
metadata and bytes and MUST NOT overwrite, delete or silently replace it.

## 7. Rehearsal and local coverage

Native matrix builds are CI-only because one local host cannot execute all five targets. The local
gate runs packager, SPDX, archive, linkage-refusal and smoke-supervision tests, plus the release
workflow structural check. The CI jobs are named in `gate.py`'s non-local registry with this reason;
the commands that validate their generic logic remain local gate steps.

Release rehearsal assembles fixture artifacts for all five targets, proves exact-set aggregation,
checksum stability and retry byte comparison, and performs no upload. That rehearsal is the
`A-11` coverage required by `P-14`; running a real matrix only after a tag would be the first test of
the publication path and is therefore refused.

## 8. Static vectors

| ID | Mutation | Required result |
|---|---|---|
| `ART-1` | omit, duplicate or add a target | aggregation refuses |
| `ART-2` | archive version, commit or target differs from the tag | aggregation refuses |
| `ART-3` | Linux ELF carries an interpreter or dynamic dependency | linkage check refuses |
| `ART-4` | manifest claims an optional feature not used by the build | packager refuses |
| `ART-5` | `sipx version` differs or the loopback call does not answer | smoke proof refuses and joins both groups |
| `ART-6` | SBOM omits the root, carries an absent edge or uses wall-clock time | SBOM validation refuses |
| `ART-7` | archive or SPDX byte changes after `SHA256SUMS` | aggregation refuses |
| `ART-8` | existing release asset has different bytes | retry refuses without an upload |
| `ART-9` | device-audio branch does not compile on macOS or Windows | the target job fails |
