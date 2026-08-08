---
id: S-51
title: Expose lossless Warning-agent editing
pillar: Signalling
status: done
priority:
design: docs/specs/lossless-presentation-editing.md
epic: sip-core
areas: [sipx-sip, privacy]
predicate:
announcement:
note: requested by sipx-clstr CX-17 — parser-owned warn-agent replacement retaining code and text
---

# Expose lossless Warning-agent editing

## Goal

Let a forwarding consumer replace one complete RFC 3261 Warning agent with a validated pseudonym
without parsing the list locally or changing its warning code, quoted text or surrounding bytes.

## Acceptance

- [x] A shared Warning parser validates comma-joined and repeated `warning-value` rows, hostport or
      pseudonym agents and escaped quoted text, retaining the complete agent span from that pass.
- [x] One public flattened-index operation replaces only the selected agent with a validated token
      pseudonym. It cannot emit the agent-less shape forbidden by RFC 3261 §25.1.
- [x] Code, text, separator spaces, folding, list delimiters, other values and other rows remain
      byte-identical; malformed syntax, bad replacement and out-of-range index are typed, atomic
      failures.
- [x] Public failing-first integration tests derive from all `LP-W` vectors in
      [`docs/specs/lossless-presentation-editing.md`](../specs/lossless-presentation-editing.md), and
      focused formatting, clippy, unit, feature-off and documentation checks pass.

## Progress

- 2026-08-05: Prepared from the downstream CX-17 integration gap. The original request repeated
  RFC 5379 §5.1.16's advice to delete an identifying hostname, but the reproduction found that an
  agent-less `Warning` violates RFC 3261 §25.1. Because that grammar explicitly permits a
  pseudonym, the contract replaces the complete agent with a validated token instead of filing an
  API that would generate malformed SIP.
- 2026-08-05: The required public tests are named below. Source reproduction confirms that
  `Headers::replace_warning_agent_with_pseudonym` is absent, so their first implementation run must
  fail at that missing public surface before a Warning parser or mutation code is added. They cover
  folding, comma-containing and escaped quoted text, IPv6 hostports, repeated rows, the malformed
  agent-less shape and hostile replacements.

## Required failing-first tests

- `warning_agent_becomes_anonymous_without_touching_code_or_text` — `LP-W-1`.
- `warning_agent_edit_retains_fold_and_escaped_text` — `LP-W-2`.
- `flattened_warning_list_edit_handles_quoted_commas_and_ipv6` — `LP-W-3`.
- `already_anonymous_warning_is_byte_identical` — `LP-W-4`.
- `agentless_warning_is_malformed_not_already_anonymous` — `LP-W-5`.
- `bad_code_text_or_later_row_makes_edit_atomic` — `LP-W-6`.
- `hostile_pseudonyms_are_refused_atomically` — `LP-W-7`.
- `warning_index_past_complete_field_is_typed` — `LP-W-8`.

The eight tests first failed to compile on the absent error type and operations. The shared parser
now validates every repeated/list row, retains each complete agent span through folding, accepts
hostport or token pseudonym syntax, and understands escaped quotes and commas in warning text. The
editor preflights all rows, splices only a validated non-empty token and reparses before assignment;
all 17 combined presentation/Warning vectors pass. The complete all-feature `sipx-sip` suite,
strict Clippy, no-default-feature targets and warning-denied rustdoc pass. Status remains in progress
only until the integrated full gate is run once.

## Notes

- Requested by downstream
  [sipx-clstr CX-17](https://github.com/codewandler/sipx-clstr/blob/main/docs/stories/CX-17-file-lossless-presentation-and-warning-editing.md)
  through its [upstream ledger](https://github.com/codewandler/sipx-clstr/blob/main/docs/upstream.md).
- Considered for the kernel: yes. Warning list syntax, quoted-string handling and trustworthy source
  ranges belong to `sipx-sip`; deciding that an agent identifies a UAS and selecting the
  non-identifying pseudonym remain consumer policy.
