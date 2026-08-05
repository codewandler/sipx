---
id: S-50
title: Expose lossless address-presentation editing
pillar: Signalling
status: ready
priority: 3
design: docs/specs/lossless-presentation-editing.md
epic: sip-core
areas: [sipx-sip, routing, privacy]
predicate:
announcement:
note: requested by sipx-clstr CX-17 — one atomic display-name and URI splice retaining all header parameters
---

# Expose lossless address-presentation editing

## Goal

Let a forwarding consumer replace one address's display name and URI atomically without rebuilding
or normalising the parser-owned header-parameter tail or any surrounding field bytes.

## Acceptance

- [ ] The common address parser retains the complete presentation span for name-address and bare
      addr-spec forms; editing never searches bytes or serializes `Address::params`.
- [ ] One public flattened-index operation replaces display name, brackets and URI atomically in
      every address field already supported by `replace_address_uri`, while retaining every byte
      outside that span.
- [ ] Display-name quoting and escaping are kernel-owned; malformed UTF-8, controls, line breaks and
      invalid candidate syntax return typed errors without mutation.
- [ ] Repeated rows, comma lists and a malformed later row preserve the collection-level atomicity
      of the existing address edits.
- [ ] Public failing-first integration tests derive from all `LP-A` vectors in
      [`docs/specs/lossless-presentation-editing.md`](../specs/lossless-presentation-editing.md), and
      focused formatting, clippy, unit, feature-off and documentation checks pass.

## Progress

- 2026-08-05: Prepared from the downstream CX-17 integration gap. Beta.7 can replace the nested URI
  but cannot replace its display name, and two sequential mutations would expose a partial edit.
  The normative contract therefore specifies one presentation span and one atomic operation before
  implementation.
- 2026-08-05: The required public tests are named below. Source reproduction confirms that
  `Headers::replace_address_presentation` is absent, so their first implementation run must fail at
  that missing public surface before parser spans or mutation code are added. They deliberately
  cover folding outside the splice, quoted display names, escaped generic-parameter values,
  bare-to-name-address conversion, repeated rows and hostile malformed input.

## Required failing-first tests

- `anonymous_presentation_retains_tag_exactly` — `LP-A-1`.
- `fold_and_escaped_parameter_tail_survive_presentation_edit` — `LP-A-2`.
- `bare_address_becomes_name_address_without_rebuilding_parameters` — `LP-A-3`.
- `flattened_list_edit_quotes_replacement_display_name` — `LP-A-4`.
- `malformed_later_row_makes_collection_edit_atomic` — `LP-A-5`.
- `unterminated_old_display_name_is_typed_and_atomic` — `LP-A-6`.
- `hostile_replacement_display_names_are_refused_atomically` — `LP-A-7`.
- `candidate_reparse_rejects_invalid_enclosing_syntax` — `LP-A-8`.

## Notes

- Requested by downstream
  [sipx-clstr CX-17](https://github.com/codewandler/sipx-clstr/blob/main/docs/stories/CX-17-file-lossless-presentation-and-warning-editing.md)
  through its [upstream ledger](https://github.com/codewandler/sipx-clstr/blob/main/docs/upstream.md).
- Considered for the kernel: yes. Address syntax, display-name quoting and the trustworthy mutation
  span belong to `sipx-sip`; the identity or routing policy that chooses a replacement stays with
  the consumer.
