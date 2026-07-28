---
id: S-10
title: Implement attended transfer
pillar: Signalling
status: done
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
- [x] `Replaces` (RFC 3891) is parsed and honoured, matching the dialog it names by `Call-ID`
      and both tags — matching on `Call-ID` alone would let one party replace another's call.
- [x] A `Replaces` naming a dialog that does not exist, or one the sender is not part of, is
      refused. This is the security-relevant case: it is a call-hijack primitive otherwise.
- [x] The replaced dialog is terminated with BYE and its media torn down.
- [x] Failing-first test: `a_replaces_naming_someone_elses_dialog_is_refused`.

## Progress
- Done. `transfer::Replaces` for the header (parse, render, match), `answer_replacing` for the
  UAS side, `Call::refer_attended` for the transferor's.
- **The check lives in the library, not in the caller.** `answer_replacing` refuses unless the
  header names the call it was handed — all three of `Call-ID`, to-tag and from-tag. Trusting
  the application to have checked would make the hijack one forgotten call site away.
- The refusal is 481 for every kind of mismatch. Distinguishing "the Call-ID matched but the
  tags did not" would tell a caller how far their guess got, which is the one thing a guesser
  needs.
- Tag orientation is the subtle part and has its own test. The `to-tag` is the *local* tag of
  whoever receives the INVITE, because the header was written by a party looking at that dialog
  from the other side. Getting it backwards fails every legitimate transfer while leaving the
  hijack case exactly as open — and only the success case would have shown it up.
- `Refer-To` percent-escapes the embedded `Replaces`. Unescaped, the URI header is truncated at
  the first `;`, the transferee places an ordinary call, and the transfer *appears* to work
  while the original call is never replaced.
