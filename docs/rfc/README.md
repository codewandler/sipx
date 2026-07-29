# The RFC registry format

`registry.toml` is the source of truth for what sipx implements. `scripts/rfc-report.py`
generates [`docs/compliance.md`](../compliance.md) from it and, with `--check`, holds its claims
against the code. Both run in the gate.

This file documents the format — for contributors adding an entry, and for **downstream registries
that inherit kernel rows by reference** rather than re-stating the claim.

## The grain is one row per RFC

Decided in [`designs/rfc-registry-grain.md`](../designs/rfc-registry-grain.md), which records the
evidence and what would reopen it. Finer-grained `[[rfc.requirement]]` rows were considered and
declined for the kernel.

The decision is enforced: an entry carrying a key the schema does not name is a gate failure. That
is deliberate — a checker that ignored unknown keys would let a claim sit in the source, never
reach the generated table, and tell nobody.

## Schema

One `[[rfc]]` table per document. **These keys and no others.**

| Key | Required | Type | Meaning |
|---|---|---|---|
| `number` | yes | integer | The RFC number. Unique across the registry — it is the row's identity. |
| `title` | yes | string | The document's title. |
| `layer` | yes | string | `wire`, `transport`, `core`, `security`, `media`, `services`. Groups the generated table. |
| `status` | yes | string | See below. |
| `evidence` | yes | list of strings | Repo-relative paths to the code or tests backing the claim. Every path must exist. May be empty only when nothing is claimed. |
| `note` | yes | string | Prose. For `partial`, it must name what is missing. |
| `roles` | no | list of strings | Which roles the claim covers — `uac`, `uas`. Absent renders as an em dash. |
| `headers` | no | list of strings | Header variants the entry claims. Each must be known to the parser's name table. |
| `methods` | no | list of strings | Method tokens the entry claims. Each must be known to the parser. |

### Status values

| Value | Meaning |
|---|---|
| `implemented` | Behaviour present and tested for the roles listed. |
| `partial` | Some of the normative behaviour; `note` says which part is missing. |
| `syntax` | The parser represents it; nothing acts on it. |
| `none` | Tracked as a target, not started. |
| `n/a` | Obsoleted by a tracked successor, or a notation rather than a behaviour. |

`syntax` is not a half-measure but a distinct state: sipx parses `RAck` and `RSeq` and sends no
PRACK, so both "we support RFC 3262" and "we reject it" would be false.

## What is checked

`rfc-report.py --check` fails on any of:

- an entry whose keys are not exactly the schema above (missing required, unknown, or wrong type);
- a `headers` or `methods` value the parser does not know;
- an `evidence` path that does not exist;
- an `implemented` or `partial` entry citing no evidence;
- a duplicate RFC number;
- an unknown `status` or `layer`;
- `docs/compliance.md` differing from what the script would generate;
- prose elsewhere in the repo stating an RFC count the registry no longer agrees with.

It deliberately does **not** verify behaviour. No script can read a transaction machine and decide
whether Timer A is right — the tests do that, and each entry points at them. What it stops is the
table drifting from the code, which is how a compliance document actually becomes untrue.

`scripts/test-rfc-report.py` tests the checker itself, including the guarantees below.

## Adding or changing an entry

Update the registry **in the same commit as the code it describes**, then run
`./scripts/rfc-report.py` to regenerate the table. Never hand-edit `docs/compliance.md`.

Adding a row changes the tracked count, which several documents state in prose; `--check` names
each one that needs updating.

## Inheriting kernel rows downstream

A downstream registry — one measuring a different role set over a different RFC set — should
extend this schema locally rather than re-stating kernel claims, and reference kernel rows instead
of copying them. So that a reference stays valid, the kernel guarantees:

- **`number` identifies exactly one row.** Uniqueness and integer typing are enforced by the
  checker and tested, so `inherits = 3261` resolves unambiguously.
- **The key set is closed and enforced**, so a pinned schema is the real schema. A row cannot
  quietly gain or lose a field.
- **Status vocabulary is stable within a major version.** Adding a status value or changing what
  one means is a breaking change to anything that pins this schema, and belongs in the CHANGELOG.
- **Pin a released tag, not `main`.** "Which kernel is this claim true of?" must have an answer;
  a tag gives it one.

What the kernel does not promise is that a given RFC keeps its `status` — that is the whole point
of the file, and it is why the reference is pinned.

A downstream extension is expected to keep the same two rules that make this registry worth
reading: the report is generated, never hand-maintained, and every claim is checked against
something the checker can actually see.
