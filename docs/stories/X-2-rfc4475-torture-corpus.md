---
id: X-2
title: Import the RFC 4475 torture corpus and its harness
pillar: Core
status: done
priority:
design:
epic: sip-core
areas: [sipx-testkit]
note:
---

# Import the RFC 4475 torture corpus and its harness

## Goal
Make the industry's standard cruelty test runnable from day one, so parser work is measured
against it continuously instead of at the end.

## Acceptance
- [x] Every message in `crates/sipx-testkit/corpus/rfc4475/`, under the RFC's own case names,
      byte-exact including deliberate malformations. _Superseded in the doing: the messages are
      recovered from the bit-exact archive in Appendix A rather than transcribed, and the
      valid/invalid split lives in the case table rather than in directory names, because the
      classification turned out to be four-way._
- [x] Cases whose bytes the RFC encodes rather than states literally (`escnull`, `esc02`,
      `intmeth`) are byte-exact — guaranteed by construction, since they come from the archive.
- [x] A harness in `sipx-testkit` enumerates the corpus and exposes it as test cases with an
      expected outcome per file.
- [x] A test asserts the corpus is complete: the expected number of valid and invalid cases
      are present, so a missing file can't quietly weaken the suite.
- [x] Until the parser exists, the harness self-tests on corpus loading only.

## Progress
- Done. The RFC embeds a base64 gzip tar of every message in Appendix A, so the corpus is
  recovered from the RFC text by `scripts/import-rfc4475-corpus.sh` rather than transcribed.
  `--check` re-derives it and diffs, so drift is detectable.
- 50 files in the archive; 49 are referenced by a section. `test.dat` is referenced by none —
  it has no SIP-Version on its request line and is not a numbered case. Carried as
  `Unreferenced` so the corpus stays a faithful copy, asserted on by nothing.
- Classification came out four-way rather than two-way, which is the useful part: 27 parse-ok,
  9 structural rejects, 7 value-level (parses, one header is bad), 6 semantic (parses, headers
  parse, the message is still unusable).
- `mcl01` is filed by the RFC under the application layer; sipx rejects it while framing
  instead, per the repeated-`Content-Length` decision in the parser spec. The divergence is
  recorded next to the case.

## Notes
- RFC 4475 is IETF-published test data and is cited directly.
- Some §3.2 cases are *unparseable* while others are *parseable but semantically invalid*;
  the harness must distinguish these, since they exercise different layers.
