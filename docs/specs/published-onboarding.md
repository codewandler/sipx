# Published onboarding consumer

This specification makes the first public Rust-library example and generated release prose
executable. It complements the registry release rehearsal: package staging proves publishable
bytes, while this contract proves that the dependency list shown to a newcomer is sufficient for
the source shown beside it.

## 1. Canonical consumer

`tests/published-answer-consumer/` is one archived external-consumer fixture. Its `Cargo.toml` and
`src/main.rs` are the complete inputs a reader receives: no workspace inheritance, development
dependency, hidden build step or ambient package is part of the example.

The manifest MUST:

- use the workspace edition and a private `0.0.0` package identity;
- declare `sipx-call`, `sipx-sip` and `sipx-transport` at the exact workspace prerelease plus the
  minimal runtime features used by the source;
- contain no `path`, Git or workspace-inherited dependency; and
- declare every package imported directly by `src/main.rs`.

The archived source MUST equal `crates/sipx-call/examples/answer_a_call.rs` byte for byte. The
public answer guide inlines that source. README, the as-a-library guide and the answer guide render
their dependency block from the archived manifest through one generated-region kind, so changing a
dependency in one front door cannot leave another behind.

The repository check copies the fixture into a clean temporary project, verifies the original
manifest has the registry shape above, and compiles it with temporary local registry patches for
the three exact same-version packages. Patches exist only in the disposable consumer and MUST NOT
be written into the archived or displayed manifest. This lets an unpublished `main` validate direct
dependency visibility without pretending its next version is already registry-visible; the tagged
release rehearsal remains the authority for packaged registry bytes.

## 2. Generated Markdown placement

Generated regions use HTML comments as source delimiters. A region whose opening marker is the
first non-whitespace content on a Markdown line MUST own that complete line: only whitespace may
follow its closing marker before the newline. A scalar may instead occur inside ordinary prose when
non-whitespace Markdown precedes its opening marker on the same line. Complete Markdown blocks,
examples and dependency snippets always use standalone markers.

`scripts/sync-website.py --check` scans every public generated region against this placement rule
before accepting synchronized content. The getting-started version sentence uses the supported
inline scalar form. After the site build, visible text extracted from
`website/build/docs/getting-started.html` MUST contain this exact whitespace-normalized sentence:

```text
Confirm which version was installed. This documentation build covers <workspace-version>:
```

Checking the complete sentence, rather than searching for the version alone, detects a paragraph
split or prefix truncation even when some other page element still contains the release number.

## 3. Vectors

| ID | Input | Required result |
|---|---|---|
| `ONB-1` | remove `sipx-sip` from the archived manifest | clean consumer compile fails on the direct import; documentation sync is stale |
| `ONB-2` | add a path/Git dependency or change one sipx exact version | source-shape check refuses before compilation |
| `ONB-3` | change the archived source or package example independently | synchronization check refuses |
| `ONB-4` | line-initial scalar marker followed by prose on the same line | placement check names the public file and line |
| `ONB-5` | supported inline version scalar and built getting-started page | built visible text contains the complete sentence and exact workspace version |
