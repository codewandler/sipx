# Spec: Supported Rust API compatibility

**Status:** normative for the `1.0.0` release baseline. This specification turns the Supported
classification in each published crate's rustdoc into a deterministic source-compatibility check.
It does not enlarge that classification and it does not promise compatibility for an item the
same rustdoc calls Experimental.

## 1. Scope and terms

The checker covers every workspace package whose Cargo manifest permits publication. A package
with no library target has an explicit empty Rust API entry; it is not silently omitted. The
WebAssembly-only package remains outside the registry set because its manifest sets
`publish = false` and its separate ABI specification governs it.

For this specification:

- a **path** is a fully qualified Rust path beginning with the published crate name;
- an **Experimental root** is a public module or item path whose item and descendants are excluded;
- a **Supported item** is a reachable public item that is not at or below an Experimental root;
- the **candidate** is the public API extracted from the current release tree; and
- the **baseline** is the canonical Supported API recorded from the final v1 release candidate.

The default is Supported. A newly public path therefore enters the candidate unless the crate's
visible Stability declaration explicitly places it below an Experimental root.

## 2. One classification, read by people and the checker

Every publishable library crate MUST carry one delimited list inside the crate-level `# Stability`
rustdoc section. The delimiter is an HTML comment so it does not distract a reader; the list
between the comments is ordinary rendered Markdown:

```rust
//! <!-- BEGIN sipx-api-classification -->
//! **Experimental Rust API roots:**
//!
//! - [`sipx_example::draft`](crate::draft)
//! - [`sipx_example::Options::unstable_builder`](crate::Options::unstable_builder)
//! <!-- END sipx-api-classification -->
```

A crate with no Experimental Rust API says `- None.`. Each other entry contains exactly one full
Rust path in backticks and MAY make it a rustdoc link whose target is the same path written from
`crate`. Prefixes, globs, prose-only names and a path in another crate are invalid. The path MUST
exist in the all-feature public API; an enum variant and a trait item count as exact nested public
paths even though rustdoc stores them inside their parent record. Duplicate or overlapping roots
are invalid because one exact, minimal list is easier to review than two equivalent
classifications.

The checker MUST parse this visible list from the crate source. No second allowlist may decide
what is Experimental. Moving a baseline Supported item below a new Experimental root is a breaking
change, not a way to hide the removal. Moving an Experimental item into Supported is an additive
graduation and enters the next explicitly reviewed baseline.

## 3. Deterministic extraction

`docs/api/v1-supported.json` records the extraction schema, workspace version, pinned rustdoc
toolchain identity, target triple, feature mode, classification hash and canonical item map for
every publishable package. Object keys and item paths are sorted; JSON uses UTF-8, two-space
indentation and one final newline.

The extractor invokes the repository-pinned nightly rustdoc in JSON mode with the lockfile,
`--offline`, and all features. It MUST NOT install a toolchain, update an index or resolve an
unlocked package. A missing pinned toolchain is a named prerequisite failure. The ordinary check
therefore has no network edge. CI installs that exact toolchain before invoking the same command.

Raw rustdoc numeric item identifiers are not stable identities and MUST NOT enter the baseline.
Every referenced identifier is resolved to its fully qualified path before canonicalisation.
An `unresolved::<kind>::<name>` fallback in a Supported record is a hard extraction failure rather
than a collision-prone identity.
Source spans, documentation prose, formatting, item order, generated identifiers and function-body
presence outside a trait default are excluded. The canonical record retains every source-compatibility
property rustdoc exposes, including:

- item kind, visibility, generics, bounds and where predicates;
- function inputs, output, receiver, safety, constness, asyncness, variadic form and ABI;
- public struct and union field name and type, plus whether a struct has stripped private fields;
- enum variant name, shape and discriminant;
- type aliases, constants, statics, macros and foreign items;
- trait supertraits, associated items and whether each associated function/constant has a default;
- direct trait implementations reachable from a public type, positive source-nameable auto-trait
  implementations (`Send`, `Sync`, `Unpin`, `UnwindSafe`, `RefUnwindSafe`), and the declared
  implementation set of a public trait; and
- compatibility attributes including `non_exhaustive` and every `repr` form.

Public modules are walked from the crate root. Public `use` items are recorded at the path a
downstream writes as well as the target's canonical definition path, so removing a root re-export
cannot be hidden by leaving the defining module public. A re-export inherits the target's
classification: a target below an Experimental root does not become Supported through an alias.
An implementation whose trait, target type or generics resolve below an Experimental root is
excluded. Any other Supported signature or field that exposes an Experimental type is a
classification error: the exposing operation must be classified Experimental or the minimal type
closure must graduate.

Rustdoc projects every applicable external blanket implementation onto every type page. Those
projections, negative auto-trait facts, compiler-internal auto traits, provided-default method
lists and function overrides are derivative compiler output rather than source commitments and
are excluded. Implementation keys are stable SHA-256 digests of their readable canonical identity,
so the identity is not duplicated in a large JSON map key.

## 4. Compatibility relation

For every baseline Supported path, the candidate MUST contain the same kind and canonical record.
A missing path, changed parameter, changed result, changed bound, removed implementation, narrowed
visibility, new safety requirement, or loss of a compatibility attribute is a breaking change.
The diagnostic names the crate, path and first differing field.

New top-level items and inherent methods are compatible. New child items use these rules:

| Parent | Candidate addition | Result |
|---|---|---|
| enum without baseline `non_exhaustive` | variant | breaking |
| enum with baseline `non_exhaustive` | variant | compatible |
| public-field struct without baseline `non_exhaustive` | field | breaking |
| struct with baseline `non_exhaustive` | field | compatible |
| struct which already had a stripped/private field | public or private field | compatible |
| union | field | compatible |
| enum variant without variant-level `non_exhaustive` | payload field | breaking |
| enum variant with variant-level `non_exhaustive` | payload field | compatible |
| trait | required function, constant or associated type | breaking |
| trait | function or constant with a default | compatible |

Removing `non_exhaustive` is breaking even if no variant or field changes in the same release.
Adding it to an already exhaustive type is also reported for review: downstream code may construct
or exhaustively match the type today. Private field names and types are not API, but whether a
struct already has any stripped/private field is retained: adding the first private field prevents
downstream construction, while removing private fields or adding fields behind an existing
construction reservation is additive. Union field additions do not invalidate construction or
access through an existing public field. Losing trait object compatibility is breaking; gaining it
is additive.

The checker does not infer binary compatibility, behavioural equivalence or wire compatibility.
Those remain owned by crate specifications and protocol tests.

The schema currently retains generic and lifetime parameter spellings. A pure alpha-renaming may
therefore require compatibility review even though it does not break downstream source. This is a
conservative false positive: it cannot allow an incompatible candidate or weaken the v1 boundary.

## 5. Baseline update and a future major version

`scripts/check-api-compat.py --update` is the only baseline writer. It first performs a normal
candidate extraction and classification validation. If a baseline exists, every old commitment
must remain compatible before the writer may roll additive paths, graduations or publishable
packages into the next reviewed v1 baseline. A check reports a candidate-only package for this
explicit review rather than silently accepting it. The canonical file records the exact workspace
release-candidate version. A check accepts later versions only while their major version still
matches the v1 selector. Thus the mechanical `1.0.0-rc.N` to `1.0.0` release bump preserves the
reviewed v1 evidence instead of demanding a rewrite after the release content was frozen; a move
to v2 fails before extraction can be mistaken for v1 evidence.

During the v1 line, an incompatible candidate fails and the baseline MUST NOT be edited to make it
green. The change is redesigned compatibly or deferred. A future major release creates
`docs/api/v<major>-supported.json` only after the workspace major version, migration guide and
changelog have moved together. The old baseline remains release evidence; selecting the new major
is a reviewable checker change, never an automatic consequence of the current source tree.

## 6. Packaged downstream proof

The registry-shaped endpoint consumer is a second check. It keeps exact registry dependencies and
contains no workspace, path or Git dependency. The rehearsal packages every workspace crate in its
dependency closure in one package-set invocation, extracts the resulting `.crate` archives into
owned temporary directories, and patches the disposable consumer to those extracted archives. The
single invocation is normative: packaging members separately would ask the registry for the other
not-yet-published release-candidate crates and would not prove the candidate set. The rehearsal
then compiles with the lockfile and offline mode.

Patching to the live workspace is not accepted by this check: Cargo metadata must contain exactly
the expected package/version nodes, each with no registry source and with its manifest under the
corresponding extracted archive. Both archive bytes and deterministic extracted-tree digests are
checked before and after compilation. Scratch directories live under this worktree's `target/`;
commands have a failure bound, and interruption terminates and reaps the whole Cargo process group.

## 7. Test vectors

Checker tests use small synthetic canonical baselines and candidates, independent of rustdoc's
own implementation:

| Vector | Mutation | Expected result |
|---|---|---|
| API-1 | remove a Supported free function | breaking; names its path |
| API-2 | change one function parameter type | breaking; names `inputs` |
| API-3 | add a required trait method | breaking; names the new method |
| API-4 | remove `non_exhaustive` from an enum | breaking; names the attribute |
| API-5 | add a free function, a defaulted trait method and a variant to a non-exhaustive enum | compatible |
| API-6 | remove a root re-export while retaining its defining module | breaking; names the alias |
| API-7 | classify a baseline Supported path as Experimental | breaking; names the classification change |
| API-8 | mutate a file after packaging but before consumer compilation | the archive proof is unchanged; a rehearsal pointed at the live source is refused |

The focused checker suite, current baseline check, packaged consumer rehearsal, crate rustdoc and
feature-off builds run before the complete repository gate. Additional adversarial vectors cover
enum payload types, struct/union and variant-level construction reservations, trait-object safety,
positive and negative auto-trait direction, projected blanket impl removal, cross-package
classification closure, baseline roll-forward, exact archive resolution and extracted-tree
immutability.
