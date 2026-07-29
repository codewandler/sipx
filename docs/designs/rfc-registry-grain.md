# Design: the grain of the RFC registry

**Status:** decided · **Pillar:** Build · **Epic:** `conformance` · **Stories:** X-15 (X-7 built
the registry this decides the grain of)

## The decision

**`docs/rfc/registry.toml` stays at one row per RFC.** Requirement-grain rows —
`[[rfc.requirement]]` carrying section, requirement reference, applicability, status and proving
tests — are **declined for the kernel**, and the decision is enforced rather than merely written
down: `schema_problems` in `scripts/rfc-report.py` rejects any key the schema does not name, so a
requirement row fails the gate loudly instead of being parsed and silently dropped.

Downstream registries that want the finer grain keep it locally and inherit kernel rows by
reference. `docs/rfc/README.md` is the contract that makes that safe.

## Why

The registry exists to be a *measurement*. Its two rules — generated, and checked — are the whole
value; a compliance table that is neither is marketing. So the question X-15 asks is not "is finer
grain more informative?" (obviously yes) but "does finer grain survive those two rules?" The
evidence says it does not, and that the coarse grain is not currently costing anything.

**Nothing is currently overclaiming because of the grain.** Of 69 entries, 15 are `partial` and
every one of them names its gap in prose; the generated table states the rule explicitly
("*Partial always says what is missing.* An entry that cannot name the gap should be `none`").
Ten of the 33 `implemented` entries also name a limitation. The question is whether those
limitations are hidden by the coarse grain, and they are not: the escape valve is *another row*.
Twelve distinct RFCs are cross-referenced from inside notes, and nine of them have their own
registry row — RFC 3311 (UPDATE), named as the gap in both 3262 and 4028, is itself a `syntax`
row and the top unstarted item on the RFC roadmap. The three without rows (5769, 3857, 4480) are
not concealed claims: 5769 is a test-vector document cited as the source of decoding vectors, and
3857 and 4480 are extensions the registry's stated bounding rule deliberately excludes. The gap
is visible, tracked and ordered — it is simply tracked one row over.

**The `syntax` status already makes the distinction the coverage kinds would make.** Acceptance
item 3 asks for syntax and behavioural coverage to be reported separately because "parses it" and
"behaves per it" differ. That distinction is the registry's existing `syntax` state, which X-7
introduced for exactly this reason and which seven entries currently sit in. Role coverage is the
existing `roles` column. Of the four proposed kinds, only *interop* is genuinely absent, and
interop is a property of a test run against another implementation, not of a row in a table.

**A requirement row could not be checked the way the existing claims are, and that is
disqualifying.** The header and method checks bind to enumerable facts in the code: the parser's
name table, the wire spellings. You cannot satisfy them without the parser actually knowing the
header. Nothing comparable exists for a section number. Sections live in prose — 1053 `§`
citations across 104 Rust files, against exactly 2 function names encoding a section — so there is
no artifact a checker could bind `section = "4.4.2"` to.

The strongest verifiable version was tested rather than assumed: *could a checker require that a
cited section appear in the entry's own evidence files?* Measured against today's registry, 30 of
32 distinct (entry, section) pairs already trace into the cited files, so mechanically it would
almost work. It is still the wrong check. It binds to a comment, not to behaviour — satisfiable by
typing a section number into a comment, which the header check is not. And the two pairs that
*fail* it are 5626 §5 and 9001 §9.2, which are precisely the sections cited as **not**
implemented. A naive bind flags exactly the honest negative claims, so it would need
status-awareness to be usable — which is the requirement-row proposal arguing for itself. The
result would be hundreds of rows carrying a status weaker than any claim the registry makes today,
in a file whose entire premise is that claims are checked.

**The cost is not hypothetical.** Evidence citations are 85 bare file paths, none in a
`file::test` form — so even at per-RFC grain, "proving tests" means "this file exists". The
registry is touched by 12 of 85 commits, roughly one in seven; requirement rows multiply the edit
surface of every one of those. RFC 3261 alone carries hundreds of normative requirements. AGENTS.md
is blunt about which way that error runs: "a measurement that lags the code is worse than no table
at all."

**The offer is not load-bearing downstream.** X-15 was filed as an offer, not a dependency. The
downstream project's own ledger already records the decision — requirement grain is a *local*
extension of this schema, an independent instance in that repo, because it claims different roles
(proxy, registrar) over a different RFC set, and its checker verifies its own artifacts (harness
scenario names, vector IDs) where this one verifies parser tables. Adopting the extension here
would unblock nothing there, and would put rows in the kernel that the kernel cannot check.

Two registries measuring different things with different checkers is the correct shape. What must
not happen is the same claim being made in both.

## Inheriting kernel rows by reference

This is the part that matters regardless of the decision, and it needs nothing new from the
registry's *content* — only a promise about its *form*. A downstream inherits a row by naming an
RFC number at a pinned kernel version, and its own checker verifies that the row exists. For that
to be safe, the RFC number must identify exactly one row, entries must have a stable shape, and
changes to that shape must be visible in a release.

`docs/rfc/README.md` states those guarantees, and they are tested rather than asserted:
`scripts/test-rfc-report.py` holds row-number uniqueness and integer typing, and
`schema_problems` holds the key set on every entry. Before this story the schema was implicit —
a key could be added, misspelled or dropped and the checker would not notice — which is a poor
thing to ask a downstream to pin against.

## Alternatives considered

- **Adopt requirement rows now, populate them lazily.** Rejected: a schema that permits a grain
  nobody fills produces a registry where absence is ambiguous — is there no requirement row
  because the requirement is unmet, or because nobody has written it yet? The per-RFC `status`
  has no such ambiguity, and partially-populated conformance data is the failure mode the
  generated-and-checked rules exist to prevent.
- **Requirement rows for `partial` entries only.** Tempting — 15 entries, gaps already named in
  prose — and the closest call here. Rejected on the same verifiability ground: it would convert
  15 checked prose notes into perhaps a hundred unchecked rows. The prose note is currently doing
  this job well and is read by humans in the generated table.
- **A fifth status, or a structured `gap` field.** Rejected as unnecessary: `note` already carries
  it, and the table's stated rule already forbids a `partial` that cannot name its gap.
- **Free-form extension keys, ignored by the kernel checker.** Rejected: that is today's
  behaviour, and it is what makes a divergence silent. Declining the grain while still accepting
  the rows would be the worst of both.

## What would reopen this

Any one of these is enough to revisit — this is a decision about present cost, not a principle.

1. **A row cannot honestly state a status.** The concrete trigger: an entry where the UA behaviour
   is split such that neither `implemented`, `partial` nor `syntax` is true, and no separate RFC
   row can carry the missing half. That is the grain actually failing, and it has not happened yet.
2. **A section-level claim becomes mechanically checkable.** If proving tests are cited as
   `path::test_name` and section references become a binding in code rather than a comment — a
   test attribute, a table, anything enumerable — then a requirement row could carry a claim as
   strong as the header check. The verifiability objection is the load-bearing one, and this
   removes it.
3. **sipx claims a role it does not claim today.** The registry is UA-shaped. A proxy or registrar
   role in the kernel would multiply per-RFC applicability and is the case where per-RFC grain
   plausibly stops being expressive enough.
4. **The downstream extension proves itself and converges.** If the local extension runs long
   enough to show the rows stay honest under real churn, adopting a proven schema is a much
   cheaper decision than adopting a proposed one — and it would then arrive with evidence this
   story did not have.
5. **Formal conformance reporting is required** — a certification programme or an interop matrix
   demanding per-requirement statements. External obligation beats internal cost.

## X-30: reachability, and why the rule is scoped to media

`X-30` asked whether the registry can distinguish *implemented in a crate* from *reachable from a
call* — after the same over-claim landed five times in two days. The answer is yes, and it needs
no new grain: it is a **constraint on the existing `roles` key**, not a third axis. A role is a
claim about what a user agent *does*, so a row claiming one whose capability nothing above the
implementing crate can select is making a claim the code does not support. No key was added, and
the decision above stands unchanged.

### The rule from `M-28`, measured

**The rule proposed by `M-28` was measured before it was adopted, and it does not hold as
stated.** "An entry may not claim `uac` or `uas` unless its `evidence` cites a file at or above
`sipx-call`" rejects **22 of the 29 role-claiming rows**.

That count is of `57857c6`, the commit this story branched from, and it is the number to reproduce:
the registry there has 70 entries, 29 with `roles`, and the reachable set is
`{sipx-call, sipx-cli, sipx-app-protocol}`. Re-run against this story's own result and the unscoped
rule rejects **18 of 26** instead — three rows lost their roles here and RFC 3711 gained a
call-layer citation, so the later number measures the corrections rather than the rule. A
measurement without the commit it is of is the failure mode this whole file is about, so: 22 of 29,
at `57857c6`.

Sorting those 22 by what is actually true of the code:

| | Rows | What the rejection means |
|---|---|---|
| Genuine over-claims | 8122, 8445, 8839 | Nothing in `sipx-call` selects the capability. Roles removed. |
| Reachable, evidence incomplete | 3711, 2617, 7616, 8760 | Reachable; the row simply never cited the consumer. |
| The rule asking the wrong question | 3263, 3264, 3581, 3680, 3856, 3903, 4235, 4475, 5389, 5626, 5627, 5922, 6026, 7118, 8599 | Reachable, and no citation would make the rule's premise true of them. |

Two counts come out of that table and they answer different questions, so both are stated rather
than one being rounded into the other:

- **7 of 22 rejections point at something true about the row** — 3 false claims plus 4 rows whose
  evidence was merely incomplete. That is the useful yield of the rule.
- **19 of 22 rejected rows were making a claim that is correct.** Only 8122, 8445 and 8839 were
  over-claiming. On the question the check exists to answer — is this row lying? — the unscoped rule
  is wrong 19 times out of 22.

`evidence` cites the code that *implements* a behaviour — that is its job — and a citation in
`sipx-transport` says nothing about whether a call reaches it. Every call reaches the transaction
layer, DNS resolution, offer/answer and `rport`. Those rows are honest and the unscoped rule calls
them liars.

That second number is the real result of this story and it stands whatever scope is adopted:
`M-28`'s rule cannot be turned on as written. Recording that was an explicit outcome the Acceptance
allowed for, and it would have been the story's whole result even if no narrowed version had been
adoptable.

### The property the check is about is *selection*

The scope is a **choice**. Nothing in the workspace forces it, and the reason to make it is a
property of the code rather than a limit of the checker.

A media capability has to be **selected** before a call can use it. SRTP is carried only because
something built `Capabilities::with_srtp`; DTLS-SRTP only because something built
`with_dtls_srtp`; ICE only because something called `MediaSession::start_with_ice`. Selecting
nothing is the default, and the default is *silent*: the call still connects, unencrypted and with
no candidates, and every test in the crate below still passes. That is precisely how ICE and
DTLS-SRTP came to be built, tested, and claimed for both roles with no call able to ask for either.

The other layers have no such gap, because their capabilities are not selected at all. There is no
`with_transactions`, no `with_dns` — a call reaches the transaction machine, the resolver, `rport`
and offer/answer on the way to existing. "Can a call reach the transaction layer" is a question
that cannot come out `no`, which is why the 15 rows above are honest and the unscoped rule rejects
them anyway.

**Selection is the rule; `layer = "media"` is a proxy for it.** They agree exactly on today's
registry, and the agreement is checked rather than asserted —
`RoleReachability.test_the_scope_tracks_selection_not_the_layer_string` holds both halves:

- every media row that keeps `uac`/`uas` (3711, 4568) is selected by a call — `with_srtp` has
  callers in `crates/sipx-call/src/call.rs` (three of them);
- every media row whose roles this story removed is selected by nothing a call runs —
  `with_dtls_srtp` has no caller outside `sipx-sdp`'s own unit tests, and `sipx-call` does not
  contain the string `ice` as a word anywhere in its source.

The proxy is used instead of the property because the check reads *evidence paths* and nothing
else. Deciding "is this capability opt-in, and does the opt-in have a caller a call runs" means
resolving callers across crates, which is a different check on a different input; it is filed under
"what would widen this" below, where it belongs, because it would replace the layer scope rather
than refine it. The proxy's cost is real and is the second item in that list: `layer` is set by the
author.

Scoped to media the rule rejects four rows: three genuine over-claims, and RFC 3711, whose SRTP
transform *is* keyed on a live call but whose evidence had never said so. That one was corrected
by citing the call-layer tests; the other three lost their roles.

### Why the four `sipx-ua` service rows keep their roles

RFCs 3680, 3856, 3903 and 4235 implement a `uas` surface in `sipx-ua` that **nothing in
`sipx-cli` calls** — `sipx-cli` offers `register`, `dial`, `answer` and `peers`, and mentions
presence, publication and event packages nowhere. Under this story's own thesis that is the media
over-claim one layer over, so it needs an argument rather than a place in the "false positives"
column. (RFC 5627 is a fifth row of the same shape, cited from `sipx-sip` and `sipx-ua`.)

**The distinguishing fact is which crate serves the claimed role, and it is a manifest fact.**
`sipx-call` depends on `sipx-media` and `sipx-sdp`; it does not depend on `sipx-ua`, which is its
sibling. So:

- For a media row, the crate that serves `uac`/`uas` — `sipx-call`, because that is where an
  application places and answers a call — is a *different crate* from the one implementing the
  capability, and it sits above it. Something has to select the capability, and for ICE nothing
  does. `sipx-call` does not mention ICE at all.
- For a services row, `sipx-ua` **is** the crate that serves the role. It is the notifier: a
  SUBSCRIBE arrives at `sipx_ua::subscribe::Subscriptions` and the package produces the body the
  NOTIFY carries. There is no crate above it that must select anything, and asking such a row to
  cite `sipx-call` would ask it to cite a crate that does not depend on it and should not.

`crates/sipx-ua/tests/packages.rs` shows the surface being driven and not merely compiled: it
links against the crate from outside, imports `sipx_ua::presence::{Compositor, Pidf, Publish,
Published, Tuple}` and `sipx_ua::packages`, and joins them to `Subscriptions` with a real
SUBSCRIBE. `scripts/test-rfc-report.py` asserts both the manifest fact and that test's contents, so
that a change to either surfaces as a gate failure rather than as prose going quietly stale.

**The honest residual:** the shipped binary cannot subscribe or publish, so nothing exercises these
rows end to end outside the test suite. That is a gap in `sipx-cli`'s feature set and a real one —
but a `uas` claim about a library is a claim about the library's API, and that API is public,
documented and driven from outside its crate. If sipx ever grows an application layer that *must*
be gone through to serve a subscription, the way `sipx-call` must be gone through to have a call,
these rows acquire the media shape and this section is wrong.

#### Why "has a cross-crate integration test" is *not* the distinguishing fact

The first attempt at this section argued the difference was that `packages.rs` proves a caller
across the crate boundary, "exactly what `Capabilities::with_dtls_srtp` and
`MediaSession::start_with_ice` have none of, in any crate, including their own integration tests".
**That is false for ICE.** `crates/sipx-media/tests/ice.rs:149-150` calls `start_with_ice` twice,
from an integration test that links `sipx_media` from outside exactly as `packages.rs` links
`sipx_ua`. If a cross-crate integration test were the criterion, 8445 and 8839 would pass it and
this story's central correction would be wrong.

It is recorded because it is the second of two false justifications this scope has been given (see
below), and both have the same shape: reaching for a mechanically appealing fact that turns out not
to be true, when the real reason is a judgement about which crate serves the role. The
test-existence argument is kept above only as a statement that the surface is *driven* — never as
the reason it differs from ICE.

### Two false justifications, recorded

The scope is right and it has now twice been defended with a claim that is not. Both are written
down because in both cases the false version made a *chosen* scope look *forced*, and a future
author would have had no reason to re-examine it.

**One — "those rows cannot satisfy the rule at any price."** The first version of this section
claimed seven `sipx-ua` rows could not, because `sipx-ua` is a sibling of `sipx-call`. **False.**
`crates/sipx-cli/Cargo.toml:21-22` names both `sipx-call` and `sipx-ua`, so `sipx-cli` is already in
the set `call_layer_crates()` computes, and it is a real consumer of the auth path:
`crates/sipx-cli/src/register.rs:95` calls `with_credentials(Credentials::new(…))`, exercised by
`crates/sipx-cli/tests/cli.rs:116`. RFCs 2617, 7616 and 8760 could satisfy the unscoped rule today
for the price of one honest citation, which is why they are filed above as incomplete evidence
rather than as the rule misfiring. They were left uncited because the check does not ask them for
it; adding those citations would improve the table and is a loose end, not a defect.

That error also inverted this document's own widening trigger, which filed "*if* an application
crate came to sit above both `sipx-ua` and `sipx-call`" as a future condition. `sipx-cli` is that
crate and has been throughout. The trigger had already fired when it was written.

**Two — "ICE has no cross-crate integration test."** Corrected in the subsection above.

There is **no suppression list**, deliberately. Every rejected row was corrected in the same
commit. A check with an exceptions file stops working the first time somebody would rather add a
line to the file than fix the row, and this check exists because five rows were wrong at once.

The reachable set is computed from the workspace manifests (`call_layer_crates`) rather than
listed in the script, for the reason `gate.py` exists one directory over: a hand-kept list of
facts about the build drifts, and drifts silently.

### What would widen this

- **A non-media over-claim.** The scope is empirical — five instances, four of them media. A
  sixth outside the media layer is evidence the narrowing was too tight, and the check takes a
  layer set rather than a constant so that widening it is one edit and a test.
- **`layer` is author-chosen, and the check keys on it.** Nothing validates a row's layer beyond
  membership of the enum, so relabelling a media row `security` exits the check entirely. This is
  the strongest argument against scoping by layer at all, and the honest reason to accept it for
  now is that moving a row between layers is conspicuous in the generated table, which groups by
  layer — but it is not *checked*, and a rule that can be left by editing one field is weaker than
  one that cannot.
- **Reachability becoming directly enumerable.** The check binds to evidence paths, which is a
  proxy: a row could satisfy it by citing a call-layer file containing a dead branch — which is
  precisely what `sipx-call`'s `a=fingerprint` rendering is today. A cross-crate caller check, or
  coverage data from the call-layer tests, would bind to the fact itself rather than to a path,
  and would be strictly better. It would also replace the layer scope, since it could ask the
  question of every row without false positives.
- **Checking selection directly, which would replace the layer scope entirely.** The property the
  scope stands in for is "this capability is opt-in, and the opt-in has a caller a call runs".
  Resolving that means finding callers across crates, not reading evidence paths — but it would ask
  every row the right question rather than exempting five layers by name, and it would be immune to
  the two dodges above and below. This is the successor to this check, not a refinement of it.
- **A path under `crates/` that is not code.** The repo-root `tests/` hatch was removed because
  `evidence` may legitimately cite markdown (RFC 5922 cites `docs/specs/sip-tls.md`) and
  `tests/interop/README.md` would have proved reachability. The narrower version has the same hole
  one directory in: `crates/sipx-call/README.md` would satisfy the check. Nothing relies on it — of
  the registry's 80 evidence paths exactly one is not a `.rs` file, RFC 5922's
  `docs/specs/sip-tls.md`, and it is outside `crates/` — and restricting to `.rs` would close it.
  Left open rather than fixed because it is one condition on a path and the successor check above
  makes the whole path-based approach redundant; recorded so it is a known hole and not a
  discovered one.

## Consequences

- The registry keeps one row per RFC, and gains an enforced key set.
- A `[[rfc.requirement]]` row is now a gate failure with a message naming this document, so the
  next person to want the grain finds the argument instead of rediscovering it.
- `docs/rfc/README.md` documents the schema as a consumable contract, which is what a downstream
  pins against.
- `scripts/test-rfc-report.py` is the first test for the report script. Wiring it into CI is a
  loose end this story leaves deliberately, since the gate's composition is not X-15's to change.
