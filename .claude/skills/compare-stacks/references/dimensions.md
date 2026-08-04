# Per-dimension derivation recipes

What to look for, and what counts as evidence at each tier. This is the part that rots if it stays
in an agent's head: without it, two refreshes six months apart ask different questions and the page
silently changes what it means.

Every recipe below assumes a **pinned clone** at `$CLONE` and a subject id read from
`docs/comparison/stacks.json`. Name no subject in any note you write outside `docs/comparison/`.

A general rule for all six: **prefer a command over a reading.** If a claim can be turned into
something that greps, counts or parses, it can be `measured` and it can be re-run next time. If it
cannot, be honest and drop a tier.

---

## `language-safety`

**Asks:** what is it written in, and which class of defect is ruled out by construction?

Look for:

- The implementation language of the **parser**, not of the bindings. A C core with a Python
  wrapper is a C stack for this row.
- Whether the language rules out memory-safety defects at all. For a memory-safe language, whether
  the project opts out — `unsafe` blocks, `cgo`, `unsafe { }`, JNI, P/Invoke, `ctypes` — and how
  much.
- What the parser does on malformed input: an exception it documents, or undefined behaviour.

`measured` looks like:

```sh
# how much of a memory-safe codebase opts out
grep -rn "unsafe" "$CLONE/src" --include='*.rs' | wc -l
grep -rn "\"C\"\|cgo\|unsafe.Pointer" "$CLONE" --include='*.go' | wc -l
```

…with the count in the summary and the command in `reproduce`. Note the flags exactly: a count
taken over a different directory next refresh is a different measurement wearing the same number.

`documented` looks like: the project's own README or manual stating the language and any safety
posture. `assessed` looks like: a judgment about how exposed the unsafe surface is to network
input, which no grep can answer — and it needs a rationale saying which files you read.

**Do not** turn this row into "memory-safe language wins". A twenty-year-old C stack with a fuzzed
parser and a published advisory history may be a better bet than a memory-safe one nobody has
attacked; that argument belongs to `security-posture` and `maturity-adoption`, and this row should
state the property, not the conclusion.

---

## `transports`

**Asks:** which transports can carry signalling, and which are secure?

Look for:

- UDP, TCP, TLS, WebSocket, secure WebSocket, and anything newer.
- Whether each is a build-time option, a runtime option, or absent.
- Whether the secure ones verify certificates **by default** — this is where stacks differ most and
  advertise least.

`measured` looks like: grepping the transport enum, factory or registration table and listing what
it names.

```sh
grep -rn "TRANSPORT_\|transport_type\|enum .*Transport" "$CLONE/src" | head -40
```

Prefer the type that the code switches on over a list in the documentation — a README can promise a
transport the build no longer produces. Where a subject spells transports as strings on the wire,
grep for the spellings.

`documented` is acceptable here and often correct: transport support is one of the few things
projects document accurately, because users hit it immediately.

Record **secure defaults** in the summary when you find them either way. "Supports TLS" and
"verifies the peer certificate unless told not to" are different claims, and only the second one
is worth anything to a chooser.

---

## `media`

**Asks:** does it terminate media at all, and if so what can it encode, carry and protect?

The first question is the important one, and it is binary: a signalling-only stack and a
media-terminating stack solve different problems, and a feature list saying "SIP" hides the
difference. Establish it before anything else.

For a signalling-only subject, the summary says so plainly and the rest of the row is that fact —
not a list of absences. It is not a deficiency; it is a different product.

For a media-terminating subject, look for:

- Codecs, by finding the encode **and** decode paths, not by finding the name. A payload-type
  constant is not a codec.
- SRTP, and how keys are exchanged — SDES, DTLS-SRTP, or nothing.
- ICE, and whether it is a real agent or only the SDP attributes.
- A jitter buffer, and whether it adapts.

`measured` looks like: listing the source files under the media directory that implement a codec,
or grepping for the encoder entry points.

```sh
ls "$CLONE/src/media/codec/"
grep -rln "srtp_protect\|dtls" "$CLONE/src" | head
```

**The trap on this row** is claiming a capability from the presence of a header, a constant or a
dependency. The whole reason this repository has a `check-audio-claims.py` is that a package
description promised a codec the crate did not implement. Apply the same standard to a subject:
find the code that does the work, or drop a tier.

---

## `security-posture`

**Asks:** what does the project do to find its own vulnerabilities, and what did it publish when it
found them?

Look for:

- A published advisory feed — GitHub Security Advisories, a CVE history, a security page. **A long
  advisory history is a sign of maturity, not of weakness.** A project that publishes CVEs is a
  project that receives reports and acts on them; silence usually means nobody is looking.
- Fuzz targets in the tree, and whether anything runs them.
- Sanitisers, race detection or static analysis in CI.
- A stated reporting route and a disclosure policy.

`measured` looks like:

```sh
ls "$CLONE/fuzz" "$CLONE/tests/fuzz" 2>/dev/null
grep -rln "asan\|-fsanitize\|-race\|valgrind" "$CLONE/.github/workflows" 2>/dev/null
```

Advisory counts are `documented` — cite the advisory index URL, not a summary of it, and give the
count as of `evaluated_at`.

**Never write this row as a scoreboard of CVE counts.** The number is a function of age, attack
surface, popularity and honesty in unknown proportions, and comparing raw counts across projects of
different ages is exactly the unfalsifiable claim this page refuses. State what the project does,
cite the feed, and let the reader weigh it.

---

## `testing-ci`

**Asks:** what has to be green before a change lands, and is conformance measured or asserted?

Look for:

- The CI configuration, and which jobs are required rather than merely present.
- Whether protocol conformance is checked by anything, or claimed in a table.
- Torture corpora, and whether the cases are enabled or commented out. **Check this specifically.**
  A test file listing every case in a conformance corpus with half of them disabled is a common and
  entirely honest state — projects do it while working through them — but it means something very
  different from a green suite over the whole corpus, and only reading the file tells you which.
- Whether the test suite runs against a real peer, or only against itself.

`measured` looks like:

```sh
ls "$CLONE/.github/workflows"
grep -c "func Test\|#\[test\]\|TEST(" -r "$CLONE" --include='*.go' --include='*.rs' --include='*.c'
grep -rn "t.Skip\|#\[ignore\]\|DISABLED_" "$CLONE" | wc -l
```

A raw test count is weak evidence and should be stated as what it is — a count of test functions,
not of assertions or of coverage. The count of **skipped** tests is often the more interesting
number, and almost nobody publishes it.

---

## `maturity-adoption`

**Asks:** how long has it existed, who runs it in production, and has anyone outside audited it?

Look for:

- First release date and release cadence — read the tag list, not the README.
- Named downstream users, especially ones the project did not choose to advertise: packaging in
  distributions, other projects depending on it, an ecosystem of bindings.
- Any third-party audit, and its date.
- Whether the project is maintained now: recent commits, issue response, whether releases still
  ship.

`measured` looks like:

```sh
git -C "$CLONE" log -1 --format=%ad
git -C "$CLONE" tag --sort=-creatordate | head -5
git -C "$CLONE" rev-list --count HEAD
```

**This is the row where this repository loses**, and the run is not finished until it says so. Age
in production against real peers is evidence no test suite substitutes for, and a stack that has
been carrying calls for fifteen years has been tested by traffic in ways nothing here has. Write
that plainly. A comparison page whose every row favours its author is the artifact this whole
mechanism exists to avoid producing.

Be careful in the other direction too: "last commit six months ago" is not abandonment for a mature
library that has finished being written. Say what you found — the dates — and let the reader judge.
