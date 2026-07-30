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
| `roles` | no | list of strings | Which roles the claim covers — `uac`, `uas`. Absent renders as an em dash. On a `media` or `security` row, at least one `evidence` path must be Rust source at or above `sipx-call`: see below. |
| `headers` | no | list of strings | Header variants the entry claims. Each must be known to the parser's name table. |
| `methods` | no | list of strings | Method tokens the entry claims. Each must be known to the parser. |
| `spec` | no | string | Repo-relative path to the normative document for the RFC — AGENTS.md non-negotiable 4. One path, not a list: a subsystem has one spec. Must exist. Renders as the `Spec` column. |

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
- a `spec` path that does not exist;
- a `media` or `security` entry claiming a role while citing no Rust source at or above `sipx-call`;
- a `media` entry claiming `status = "implemented"` on the same terms;
- an entry citing `sipx-media`, `sipx-rtp` or `sipx-audio` while declaring a layer other than
  `media`;
- `docs/compliance.md` differing from what the script would generate;
- prose elsewhere in the repo stating an RFC count the registry no longer agrees with.

### Reachability: a claim may not outlive its caller

A role is a claim about what a *user agent does*, and some capabilities are **selected** by the
call layer rather than reached automatically — so a row claiming one must cite at least one Rust
file in `sipx-call` or a crate depending on it. That set is read from the workspace manifests, not
listed anywhere; only `crates/….rs` paths count.

This exists because the same over-claim landed five times in two days: a keying or a NAT strategy
built and tested inside one crate, claimed for both roles, with no caller above the crate — and
every other check on this list passes for such a row, since the header is known, the file exists,
and evidence was cited. To drop a role, say in the `note` what is missing, as RFC 5763, 5764,
8122, 8445 and 8839 do.

**The property is selection, and the layer is a proxy for it.** A capability at a selection layer
is carried only because something asked for it — `Capabilities::with_srtp`, `with_dtls_srtp`,
`MediaSession::start_with_ice`, `Config::with_credentials`, a `Target` built with a secure
`TransportKind` — and asking for nothing is both the default and silent: the call still connects,
unencrypted and unauthenticated, and every test in the crate below still passes. Nothing is
*selected* in the other layers: there is no `with_transactions` and no `with_dns`, so "can a call
reach the transaction layer" is a question that cannot come out `no`.

Unscoped, the rule rejects 22 of the 29 role-claiming rows (measured at `57857c6`); only 7 of those
rejections point at anything true of the row, and only 3 rows were over-claiming at all — so on the
question the check exists to answer, the unscoped rule is wrong 19 times out of 22. **The scope is
therefore a deliberate choice, and it is `{media, security}`, not a limit of the workspace.**
`X-33` added `security` after measuring it: all four security rows claiming a role failed the rule,
three needed one honest citation each (`sipx register --password` is the selection), and the fourth
needed a call over a secure transport. `transport` was measured and declined — it mixes selected
capabilities (RFC 7118, 5626, 8599) with plumbing every call runs (3263, 3581), and an
evidence-path check cannot tell them apart.

**`status = "implemented"` is held to the same rule at the media layer.** A row with no `roles`
claims nothing about a role, but it still makes a claim in the same table: RFC 6716 and 7587 said
`implemented` about Opus, which no call can select, and the check returned before asking. It is
media-only because three of the seven `implemented` security rows state *policies* of the TLS stack
— 6125, 8446 and 8996 — which hold on every connection and are proved by the absence of an API, so
reachability is not the question they answer.

`partial` is exempt, and that is the demotion rather than an exception: it changes what the
published table says, and the note must then name the gap. **There is no suppression list**, under
any name — a row that cannot be made true is demoted, visibly, in the generated table.

Two limits remain and are recorded rather than fixed. `layer` is author-set, so relabelling can
still leave the check — pinned only for rows citing `sipx-media`, `sipx-rtp` or `sipx-audio`, the
crates an unreachable media capability actually lives in. And evidence paths are a proxy for
callers, so a row can satisfy the rule by citing a call-layer file containing a dead branch.
`docs/designs/rfc-registry-grain.md` carries the full counts, the false justifications this scope
has been given, and the successor check that would replace path matching with caller resolution.

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
