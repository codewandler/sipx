---
id: X-2
title: Import the RFC 4475 torture corpus and its harness
pillar: Core
status: ready
priority: 2
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
- [ ] Every message from RFC 4475 §3.1 (valid) and §3.2 (invalid) transcribed into
      `crates/sipx-testkit/corpus/rfc4475/{valid,invalid}/`, named after the RFC's own case
      names, byte-exact including deliberate malformations.
- [ ] Cases whose bytes the RFC encodes rather than states literally (`escnull`, `esc02`,
      `intmeth`) are decoded correctly — verified against the octet counts in the RFC text.
- [ ] A harness in `sipx-testkit` enumerates the corpus and exposes it as test cases with an
      expected outcome per file.
- [ ] A test asserts the corpus is complete: the expected number of valid and invalid cases
      are present, so a missing file can't quietly weaken the suite.
- [ ] Until the parser exists, the harness self-tests on corpus loading only.

## Progress
- Not started.

## Notes
- RFC 4475 is IETF-published test data and is cited directly.
- Some §3.2 cases are *unparseable* while others are *parseable but semantically invalid*;
  the harness must distinguish these, since they exercise different layers.
