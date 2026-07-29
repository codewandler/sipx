---
id: C-5
title: The application contract crate and its sans-IO interpreter
pillar: Application
status: in-progress
priority: 2
design: docs/designs/app-sdk.md
epic: app-sdk
areas: [sipx-app-protocol]
note: app-sdk · parallel to C-3/C-4/M-17/M-18 · spec is docs/specs/app-contract.md · size M
---

# The application contract crate and its sans-IO interpreter

## Goal
A new crate — working name `sipx-app-protocol` — that owns the `sipx.app.v1` types and an
instruction interpreter that is a pure state machine: call events and an instruction program in,
effects out. The primitive every host is a driver over.

## Acceptance
- [x] The crate defines the event, instruction and envelope types of
      [`docs/specs/app-contract.md`](../specs/app-contract.md), with serialization confined to
      this crate — `sipx-call` and the rest of the workspace gain no new dependencies.
- [x] The interpreter consumes `CallEvent`s (`C-3`) and instruction documents, and yields typed
      effects that map one-to-one onto `sipx-call`/`sipx-media` operations. It contains no
      socket, no clock read, and no async runtime; time enters as fired-timer inputs.
- [x] The spec's continuation rule is enforced by construction: at most one outstanding
      callback per call, and a document accepted in response to event E **replaces** the pending
      instruction queue. The spec's vectors for this rule pass.
- [x] Every vector table in the spec has a derived test, and the interpreter passes all of them.
- [x] An `examples/` binary drives the interpreter over a real call with a canned program
      (answer → play → gather → hang up), and a shell script asserts the outcome — the epic's
      end-to-end proof, with no host and no workspace-wide dependency additions.
- [x] The crate is marked experimental in its README and crate docs, matching the spec's status.
- [x] Failing-first test: the spec's vector `AC-1` (event in, no program loaded → the defined
      default effect, not a panic).

## Progress
Done. `crates/sipx-app-protocol` exists, speaks `sipx.app.v1`, and passes all nine of §11's
vectors. Where each Acceptance item landed:

| Item | Code | Test |
|---|---|---|
| types, serialization confined | `src/{event,document,json,base64,time,tagged}.rs` | `document::tests`, `event::tests` |
| interpreter, sans-IO | `src/interpreter.rs`, `src/call.rs` | `tests/vectors.rs` |
| continuation rule by construction | `Callback`, `Interpreter::accept` | `ac_3`, `ac_4`, `the_interpreter_never_issues_a_second_callback…` |
| every table derived | `tests/spec_tables.rs` | 7 tests, one per table plus §5.3's inline lists |
| end-to-end proof | `examples/canned_program.rs` | `tests/canned_program.sh` |
| experimental | `README.md`, `src/lib.rs` | `the_spec_and_the_crate_agree_that_this_is_experimental` |
| failing-first `AC-1` | — | `ac_1_no_program_and_an_unreachable_app_takes_the_declared_effect` |

### The predecessor's scaffolding was kept, hand-rolled codecs and all

An interrupted run left `Cargo.toml` and six modules written and **nothing committed**. It was
read in full, judged, and committed verbatim as the first commit on this branch before any other
work, so that a second session exit could not cost it twice. It did not compile — there was no
`lib.rs`, and the modules forward-referenced a `testing` module that did not exist — but the
question worth deciding was not whether it built, it was whether a hand-rolled `json.rs` and
`base64.rs` were the right reading of the Acceptance.

They are, and the reasoning is the Acceptance's own sentence: *serialization confined to this
crate — `sipx-call` and the rest of the workspace gain no new dependencies*. What was rejected:

- **Take a serialization framework** (a derive-macro crate and a JSON backend). Rejected because
  this workspace has no serialization framework to borrow, so it would be a **new workspace
  dependency** — two of them, with a proc-macro build step — to serve one leaf crate. That is the
  thing the Acceptance sentence exists to forbid, and the dispatch says a new third-party crate is
  a BLOCKED report rather than a judgement call.
- **Put the wire types in `sipx-call`** so its existing types could be reused. Rejected: it makes
  the kernel crate depend on the application contract, which is backwards, and the design's
  "Alternatives considered" already settled that this crate is what a *remote* SDK generator and
  the host both consume.
- **Ship no codec and make bindings bring their own.** Rejected because it moves §6.4's "rejected
  whole" into every binding, where each one gets to be subtly different about it. The interpreter
  parses the response body itself (`Response::Body`), so there is one reader of hostile input in
  the workspace and one place that rule is enforced.

The cost is roughly 760 lines of JSON, base64 and RFC 3339 arithmetic to own. They are bounded
(`MAX_DEPTH`), total on every input, and tested against the RFCs' own vectors, which is what makes
that cost payable.

### Two decisions the spec left open

- **An empty `instructions` array does not replace the program.** §6.3 says a document *replaces*
  the pending program and also that an empty array means "keep going"; those only reconcile if the
  empty document is the one that does not replace. Implemented that way, with the reasoning at the
  branch (`interpreter.rs`, `accept`).
- **§6.5's `dial` allowlist is host configuration, so it lives on `Policy`** and is checked when a
  document is accepted rather than when it is parsed. Empty by default — a host that has not said
  which fields an app may set has said none.

### Not done here, deliberately

The interpreter names every verb in §6.2, but the example implements four effects. The rest are
the *host's* to perform (`A-2`), and several name operations that are still stories: `bridge` and
`unbridge` are `C-6`, `record`'s completion is `M-17`'s. `Effect` is `#[non_exhaustive]`, so those
arrive without breaking a driver.

## Notes
- The interpreter is the primitive every binding drives — remote or in-process — which is why
  it lives in its own crate and not inside `sipx-app`. See the design's "Alternatives
  considered" for the placements weighed.
- The host (`crates/sipx-app`, the [app-host](../designs/app-host.md) epic) implements the
  webhook, socket and embedded-runtime bindings of this same vocabulary; `A-2` needs this story.
