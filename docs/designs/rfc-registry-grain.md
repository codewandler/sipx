# Design: the grain of the RFC registry

**Status:** decided · **Pillar:** Build · **Epic:** `conformance` · **Stories:** X-15 (X-7 built
the registry this decides the grain of), X-30 and X-33 (the reachability rule the grain carries)

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

## X-30: reachability, and why the rule was scoped to media

*(`X-33` widened the scope to `{media, security}` and to `status` as well as `roles`. This section is
kept as `X-30` wrote it — including the measurement the widening was tested against — and the
`X-33` section that follows says what changed and why. The check is now `unreachable_claims`.)*

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

**Selection is the rule; `layer = "media"` is a proxy for it.** On every media row that *claims a
role* the two agree exactly, and the agreement is checked rather than asserted —
`ClaimReachability.test_the_scope_tracks_selection_not_the_layer_string` holds both halves
(`X-33` renamed the class when the check stopped being only about roles):

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

### The known limit: the gate is on `roles`, not on `status`

*(Closed by `X-33`. Kept as filed, because the measurement below is what the fix was measured
against, and because the shape of the hole is the reusable part.)*

`unreachable_role_claims` returns early for any row without a `roles` list, so a media row can say
`status = "implemented"` about something no call can reach and the check will pass it. That is not
hypothetical. **RFC 6716 and 7587 are both `layer = "media"`, both `status = "implemented"`, and
neither carries `roles`** — and Opus is unreachable from a call: `sipx-call` hardcodes
`Capabilities::g711` at `call.rs:606`, `:752`, `:955`, `:1728`, `:2860` and `:3161`,
`Codec::from_payload_type` (`crates/sipx-media/src/session.rs:115-124`) deliberately never returns
Opus, `Capabilities::with_opus` has no caller outside `sipx-sdp`'s own tests, and no `sipx-call`
entry point takes caller-supplied `Capabilities` at all — every mention of the type in that crate is
`pub(crate)` or private. So the exact failure this story exists to stop is still reachable one field
over, by claiming a status instead of a role. Filed as `X-33`; deliberately **not** fixed here,
because binding `status` to reachability is a different rule needing its own measurement, and this
story's own result is that adopting such a rule unmeasured is how you get 19 false rejections.

## X-33: the scope, widened along both axes it was measured to be narrow on

`X-30` left a rule that was right about one layer and one field. `X-33` asked what the honest
generalisation of it is, and the answer is **not** "every layer" — that was already measured and
rejected — but the two specific widenings the measurement supports. Nothing about the grain decision
changes; no key was added.

### The property, restated so it can be applied

A capability is in scope when it must be **selected** before a call can use it, and when selecting
nothing is *silent*. Both halves matter. The second is what makes the over-claim survivable: the
call still connects, and every test in the crate below still passes, so nothing goes red.

`layer` remains the proxy. The question this story had to answer is which layers have the property,
and it is answered by measurement per layer rather than by an argument about layers in general.

### Axis one: `security` has the property, and three of its four rows were merely uncited

| Half of the property | Media | Security |
|---|---|---|
| Selected by something above | `Capabilities::with_srtp`, `with_dtls_srtp`, `MediaSession::start_with_ice` | `Config::with_credentials`; a `Target` built with a secure `TransportKind` |
| Selecting nothing is silent | the call connects unencrypted | the REGISTER succeeds unauthenticated; the call goes out in plaintext |

The silence is not asserted, it is read off the code: the only `with_credentials` caller above the
call layer is `crates/sipx-cli/src/register.rs:95`, and it sits inside `if let Some(password)` at
`:94`. `sipx register` with no password registers, and nothing fails.

All four security rows claiming a role failed the widened rule. Sorted by what is true of the code:

| Rows | What the rejection meant |
|---|---|
| 2617, 7616, 8760 | Reachable; the row never cited the selection. Now cite `crates/sipx-cli/src/register.rs`. |
| 5922 | Reachable; the row never cited a call. Now cites `crates/sipx-call/tests/wss.rs`. |

So the security half of the widening yields four corrected citations and **zero demotions** — which
is the shape `X-30`'s first false justification denied was possible ("those rows cannot satisfy the
rule at any price"). The trigger it inverted had already fired, and this is it firing.

Two honest residuals came out of doing it, and both are now in the rows themselves rather than only
here:

- **There is no authenticated-REGISTER test at or above the call layer.** `crates/sipx-cli/tests/`
  contains no `401`, no `407`, no `Authorization` and no `password`; the digest test is one crate
  below, at `crates/sipx-ua/tests/register.rs:265`. The `uac` claim rests on the *selection* being
  reachable, which it is, and on the behaviour being tested, which it is — one crate down.
- **`sipx dial --password` silently discards the credential.** `--password` is a registered valued
  flag (`crates/sipx-cli/src/main.rs:168`) and `crates/sipx-cli/src/dial.rs` never reads it, so a
  call challenged with 407 fails rather than retrying. RFC 2617's row now says so.
- **The shipped binary cannot select TLS at all.** `dial` and `register` choose between UDP and
  `--tcp`; `target_of` in `crates/sipx-cli/src/dial.rs` strips `sips:` exactly as it strips `sip:`
  and defaults the port to 5060, so a `sips:` URI is dialled in plaintext rather than refused. RFC
  5922's role claim is reachable from the *library*, proved by `crates/sipx-call/tests/wss.rs`, not
  from `sipx`.

### Axis two: `status = "implemented"` is a claim in the same table

A row with no `roles` claims nothing about a role. It still says `implemented`, in a generated table
whose heading reads "What sipx implements", and a reader seeing "RFC 6716 Opus ✅ implemented"
concludes a call can be placed with Opus. It cannot, on four independent grounds — no `with_opus`
caller outside `sipx-sdp`'s own tests, `Codec::from_payload_type` deliberately never returns it, no
`sipx-call` entry point takes caller-supplied `Capabilities`, and the `opus` feature is off at every
level from `sipx-audio` up to `sipx-cli`.

Measured at the media layer, the status rule rejects 5 of the 5 `implemented` rows:

| Rows | What the rejection meant |
|---|---|
| 8866, 3550, 4733 | Reachable; the row never cited the call. SDP is built and answered in `call.rs`; audio crosses in `tests/call.rs`; `1234#` crosses in the same file. |
| 6716, 7587 | Unreachable. **Demoted to `partial`**, with the note naming the gap. |

**Why it stops at media, and why that is a measurement rather than a preference.** Every media row
claiming `implemented` names a capability a call either carries or does not. At the security layer
three of the seven do not: 6125 (a non-matching SAN is refused rather than falling back to the CN),
8446 (1.3 preferred) and 8996 (1.2 is the floor and not configurable downward) state *policies* of
the TLS stack. A policy holds on every connection and is proved by the **absence** of an API, so
"which call reaches it" is not the question those rows answer, and asking it would reject three
honest rows to catch nothing. That limit is asserted, not just written:
`test_the_status_gate_is_media_only_and_the_reason_is_measured`.

**`partial` is exempt, and that is a demotion rather than an exception.** This is the sharpest
objection to the rule and it deserves the direct answer: a suppression list leaves the claim intact
and hides the objection somewhere only a maintainer reads. `partial` changes what the *published*
table says — from "✅ implemented" to "🟡 partial" — and the table's own stated rule is that a
`partial` names what is missing. Five rows already used exactly that form for exactly this fact
(5763, 5764, 8122, 8445, 8839); Opus makes seven. The residual hole is an author writing `partial`
without naming the gap, which is prose and unenforceable — but it is conspicuous, because the status
column changes in a generated document.

### The four package rows, resolved one at a time

`X-30` gave RFCs 3680, 3856, 3903 and 4235 one collective argument, which the story filing `X-33`
called out as the "rule fitted to the data it was tested on" risk. Taken row by row the argument
turns out to be the same argument four times and to hold — but it rests on a fact nobody had run:

**No crate in this workspace receives a SUBSCRIBE or a PUBLISH off a socket.**
`Subscriptions::on_subscribe` and `Compositor::apply` take an already-parsed `sipx_sip::Request` and
are handed one by `sipx-ua`'s own tests. `sipx-call`'s dispatcher routes ACK, BYE, NOTIFY, PRACK,
REFER and UPDATE, and unit-tests that SUBSCRIBE and PUBLISH are *not* on `Allow`. `sipx-ua/src/`
contains neither `Method::Subscribe` nor `Method::Publish` anywhere.

That is precisely what makes `sipx-ua` the crate that **serves** the role rather than a crate below
one that must select the capability. There is no `sipx-call` for subscriptions; `sipx-ua` is the top
of that stack, and asking these rows to cite the call layer would ask them to cite a crate that does
not and should not depend on them. Row by row:

| Row | Resolution | The fact it rests on |
|---|---|---|
| 3903 (PUBLISH, `uas`) | Role kept | `Compositor::apply` decides what a publication means and what to answer; the application supplies the request. |
| 3856 (presence package, `uas`) | Role kept | Joined to the notifier and driven from outside the crate: `packages.rs` publishes and asserts the NOTIFY body changes with it. |
| 3680 (`reg` package, `uas`) | Role kept | Registered under the name a subscriber asks for, asserted from outside the crate. The missing registrar join was already in the note. |
| 4235 (`dialog` package, `uas`) | Role kept | The same, plus the missing dialog-store join, also already in the note. |

Each note now carries the limit as well as the claim, and `X-33` added the `tests/packages.rs`
citation to 3680 and 4235, which had cited only the source.

**The trigger that takes these roles away is now a test, not a paragraph.**
`test_the_services_rows_keep_their_roles_only_while_nothing_dispatches_to_them` asserts that nothing
dispatches on SUBSCRIBE or PUBLISH. The moment something does — a server mode, an application host,
a request router — there *is* a crate above `sipx-ua` that must select the package, these rows
acquire the media shape exactly, and that test goes red before anybody has to remember this section
exists. `X-30`'s version of this said "if sipx ever grows an application layer … this section is
wrong", which is true and which nothing would have enforced.

### Why `transport` is *not* in the scope, measured rather than asserted

This is the layer that could plausibly have joined, and the reason it does not is the most useful
result of this story, because it is the proxy failing rather than the proxy being conservative.

The transport layer **mixes both kinds of row**:

- *Selected*: RFC 7118 (an application chooses the WebSocket transport), 5626 (outbound is opted
  into per registration), 8599 (push parameters are supplied by the application).
- *On the path of every call*: 3263 (every call resolves), 3581 (`rport` is always observed and
  echoed).

Widening to `transport` would reject 3263 and 3581, and **no citation could honestly fix them** —
they are the "rule asking the wrong question" bucket, and putting them in it is the same 19-out-of-22
error one layer along. An evidence-path check cannot separate the two kinds, because the distinction
is about callers and not about paths.

So the scope stops, and the two rows it cannot adjudicate are named rather than quietly counted as
false positives: **RFC 5626 and 8599 may be over-claims of exactly the ICE shape** — a `uac` surface
in `sipx-ua` that nothing above it opts into — and this check cannot tell. That question needs the
successor below. RFC 7118 was adjudicated by hand and turned out reachable, so it now cites
`crates/sipx-call/tests/wss.rs`, which is a whole call over the transport.

### Closing the two escape hatches, and correcting the count they were recorded with

**The `layer` dodge is pinned where it can be pinned.** `sipx-media`, `sipx-rtp` and `sipx-audio`
implement nothing but media, so a row citing one of them is a media row whatever its `layer` says
(`misdeclared_layer`). This is not a general layer classifier and does not try to be: `sipx-sdp` is
cited by RFC 3264, which is legitimately `core`, and `sipx-transport` is cited by rows at three
layers. What it does is make the dodge unavailable to the rows that would want it — an unreachable
media capability lives in one of those three crates, and leaving the check would now mean not citing
your own implementation, which the evidence-existence check already forbids. The residual: a row that
implements a media capability somewhere else entirely could still relabel.

**The non-`.rs` hatch is closed.** `reaches_the_call_layer` now requires the path to end in `.rs`, so
`crates/sipx-call/README.md` no longer proves anything.

**And the fact that hatch was recorded with was wrong in both halves.** This document said "of the
registry's 80 evidence paths exactly one is not a `.rs` file, RFC 5922's `docs/specs/sip-tls.md`".
Measured: **117 paths, and two are not `.rs`** — `docs/specs/sip-tls.md`, cited by 5922 *and* by
8996. The conclusion survives (both are outside `crates/`, so nothing relied on the hatch), but this
is the fourth crisp-sounding fact in this document's lineage to fail when run, which is why the story
that fixed it was told to try falsifying its own sentences first. See below.

### A third false justification, and the same shape again

`X-30`'s replacement for its first false claim cited `crates/sipx-cli/tests/cli.rs:116` as exercising
the credential path. **It does not.** That line is
`register_advertises_this_client_in_via_and_contact`, which passes no password, and the whole
`crates/sipx-cli/tests/` tree contains no `password`, `401`, `407` or `Authorization`:

```
$ grep -rn '401\|407\|WWW-Authenticate\|Authorization\|password\|Credentials' crates/sipx-cli/tests/
(no output)
```

The claim it was supporting — that RFC 2617, 7616 and 8760 could satisfy the rule for the price of
one honest citation — is *true*, and this story acted on it. Only the evidence offered for it was
invented. That is now three false facts in a row, all of them the same failure: a checkable-sounding
sentence written from memory of the codebase rather than from the codebase. The countermeasure that
worked here was to run every negative claim before writing it, and to put the ones that survive into
`scripts/test-rfc-report.py` so they fail the gate when they stop being true rather than being
believed for another story.

### What `X-33` deliberately did not build

**The cross-crate caller check is the successor, and it is not started.** It is the only honest
answer to the transport layer, to the dead-branch limit and to the `layer` dodge at once, because it
binds to reachability itself instead of to evidence paths. It needs caller resolution across crates
— reading the source, not the manifests and not the registry — which is a different check on a
different input, and half-building it would produce exactly the unmeasured rule this file spent two
stories arguing against. It wants a story of its own, and these three rows are its first test cases:
RFC 5626, 8599 (possible over-claims the path check cannot adjudicate) and 8122 (a dead branch the
path check would accept).

**Wiring Opus to a call has no story either.** `M-13` is `done` and it built the codec, not the
selection. The demotion says so.

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

*(A third was found by `X-33` — the citation offered for the correction below. It is recorded under
"A third false justification" in the `X-33` section.)*

The scope is right and it has now twice been defended with a claim that is not. Both are written
down because in both cases the false version made a *chosen* scope look *forced*, and a future
author would have had no reason to re-examine it.

**Twice is a pattern, and it is this story's own failure mode: reaching for a mechanically appealing
fact to stand in for a judgement, and not checking the fact.** Both false claims were the kind of
thing that sounds checkable — "cannot satisfy at any price", "has no integration test in any crate"
— which is exactly why neither was checked. The judgement they were substituting for is the same
one both times, and it is not mechanical: *which crate serves the claimed role*. A reviewer of the
next revision of this section should treat any crisp-sounding negative claim about the workspace as
unverified until it has been run.

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

*(Written by `X-30`, and three of the five are now resolved. Struck items keep their original text
with the outcome named, because "this was foreseen and then happened" is worth more than a tidy
list.)*

- ~~**A non-media over-claim.**~~ **Fired, and acted on by `X-33`.** The scope was empirical — five
  instances, four of them media. It now takes a layer set of `{media, security}`, and the widening
  cost four citations and no demotions. Two rows the path check cannot adjudicate (RFC 5626, 8599)
  are named under `X-33` above rather than counted as false positives.
- ~~**`layer` is author-chosen, and the check keys on it.**~~ **Pinned where it can be pinned.**
  Nothing validated a row's layer beyond membership of the enum, so relabelling a media row
  `security` exited the check entirely. `misdeclared_layer` now closes that for any row citing
  `sipx-media`, `sipx-rtp` or `sipx-audio`. Residual: a media capability implemented outside those
  three crates could still relabel, and the layer of a non-media row is still unchecked.
- **Reachability becoming directly enumerable.** The check binds to evidence paths, which is a
  proxy: a row could satisfy it by citing a call-layer file containing a dead branch — which is
  precisely what `sipx-call`'s `a=fingerprint` rendering is today, and `a=setup` with it (RFC 4145's
  row now says so). A cross-crate caller check, or coverage data from the call-layer tests, would
  bind to the fact itself rather than to a path, and would be strictly better. It would also replace
  the layer scope, since it could ask the question of every row without false positives.
- **Checking selection directly, which would replace the layer scope entirely.** The property the
  scope stands in for is "this capability is opt-in, and the opt-in has a caller a call runs".
  Resolving that means finding callers across crates, not reading evidence paths — but it would ask
  every row the right question rather than exempting four layers by name, and it would be immune to
  the dodges above. **This is the successor to this check, not a refinement of it**, `X-33`
  deliberately did not start it, and its first three test cases are named above.
- ~~**A path under `crates/` that is not code.**~~ **Closed by `X-33`.** The narrower version had the
  same hole one directory in: `crates/sipx-call/README.md` would have satisfied the check.
  `reaches_the_call_layer` now requires `.rs`. The measurement this was recorded with was itself
  wrong — 117 paths, two of them not `.rs` — and the correction is under `X-33` above.

## Consequences

- The registry keeps one row per RFC, and gains an enforced key set.
- A `[[rfc.requirement]]` row is now a gate failure with a message naming this document, so the
  next person to want the grain finds the argument instead of rediscovering it.
- `docs/rfc/README.md` documents the schema as a consumable contract, which is what a downstream
  pins against. `X-33` added the `spec` key to that table, which the script had accepted since
  `M-25` while the document promising "these keys and no others" omitted it.
- `scripts/test-rfc-report.py` is the first test for the report script. Wiring it into CI is a
  loose end this story leaves deliberately, since the gate's composition is not X-15's to change.
