---
id: S-10
title: Implement attended transfer
pillar: Signalling
status: ready
priority: 6
design: docs/designs/sip-core.md
epic: depth
areas: [sipx-call]
note:
---

# Implement attended transfer

## Goal
Attended transfer: the transferor speaks to the target first, then joins the two.

## Acceptance
- [ ] `Replaces` (RFC 3891) is parsed and honoured, matching the dialog it names by `Call-ID`
      and both tags — matching on `Call-ID` alone would let one party replace another's call.
- [ ] A `Replaces` naming a dialog that does not exist, or one the sender is not part of, is
      refused. This is the security-relevant case: it is a call-hijack primitive otherwise.
- [ ] The replaced dialog is terminated with BYE and its media torn down.
- [ ] Failing-first test: `a_replaces_naming_someone_elses_dialog_is_refused`.

## Progress
- Not started.
