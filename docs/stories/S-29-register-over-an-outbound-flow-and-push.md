---
id: S-29
title: Register over an Outbound flow, and let a registration wake on push
pillar: Signalling
status: done
priority: 4
design: docs/designs/sip-ua.md
epic: sip-core
areas: [sipx-cli, sipx-ua]
note: with_outbound and with_push have no caller outside sipx-ua's own tests — the eighth instance of the recurring defect — so X-37 demoted RFC 5626 and 8599 to no roles; wiring them is what makes the roles honest again
---

# Register over an Outbound flow, and let a registration wake on push

## Goal
Give `sipx-ua`'s Outbound and push support a caller, so RFC 5626 and 8599 can claim their `uac` role
again on the strength of a registration that actually uses them.

## Acceptance
- [x] **A registration can be placed over an Outbound flow.** `Config::with_outbound` exists
      (`crates/sipx-ua/src/agent.rs:109`) and has no caller outside `crates/sipx-ua/tests/outbound.rs` —
      verified by grep when `X-37` demoted the row. The natural caller is `sipx-cli`'s `register`
      (`crates/sipx-cli/src/register.rs`), which constructs a `UserAgent` with a plain `Config`.
      → `register.rs` now calls it under `--outbound` (`--instance <URN>` adopts a persisted
      identity rather than generating one), and reports §6's answer as the `flow` field.
- [x] **A registration can declare push parameters and be woken.** `Config::with_push`
      (`crates/sipx-ua/src/agent.rs:156`) and `UserAgent::woken` (`agent.rs:421`) are exercised by
      `crates/sipx-ua/tests/push.rs` and by nothing above them.
      → `--push-provider`/`--push-prid` (+ optional `--push-param`) build the `Device`; `--wake`
      calls `woken` after registering and reports the refresh, including `purr` when assigned.
- [x] **RFC 5626 and 8599 go back to `uac` in the same commit that makes it true**, and
      `docs/compliance.md` regenerates with it. `X-37` demoted both to no roles as the honest state —
      not as the verdict. Restoring them without the wiring would be the exact defect the reachability
      check exists to catch, and `sipx-ua`'s `# Stability` section already marks these experimental for
      the same reason, which must move with it.
      → Roles restored, both notes rewritten to name the caller, evidence gains `register.rs` and
      `cli.rs`; `./scripts/rfc-report.py --check` is green. One correction to this item's premise:
      the `# Stability` section did **not** mark Outbound or push experimental — it listed Outbound
      as Supported (a claim `X-37`'s demotion left behind) and did not classify push at all. The
      section now lists both as Supported and says on the strength of which caller.
- [x] The CLI flags follow the existing vocabulary and are documented in
      `website/docs/reference/cli.md`. `--outbound` and `--push` (or the forms that fit) must not be
      accepted-and-dropped the way `--password` was before `P-7`.
      → Every combination that cannot work is a usage error (exit 2): half a push pair,
      `--push-param` alone, `--wake` without the push flags, `--instance` without `--outbound`,
      a non-URN `--instance`, and a `pn-*` value RFC 3261's `pvalue` cannot hold. Asserted in
      `register.rs`'s unit tests; the flags are in `VALUED_FLAGS` so their values are never read
      as the AOR.
- [x] Failing-first test: a registration over a flow, asserted to keep the flow and be woken. It cannot
      pass today because no caller builds the config. Name it.
      → `register_over_a_flow_keeps_it_and_a_push_wakes_it` in `crates/sipx-cli/tests/cli.rs`.
      Before the wiring it failed with: "RFC 5626 §4.2's flow number is missing — nothing built
      the Outbound config: Contact: <sip:alice@127.0.0.1:52382>".

## Progress
- Not started. Filed at `X-37`'s close, which demoted the two rows to no roles as the honest state and
  found that `sipx-ua`'s own crate doc already said why.
- **Implemented on `impl/S-29`.** `sipx register` gains `--outbound`, `--instance`, the
  `--push-provider`/`--push-prid`/`--push-param` triple, and `--wake`; the report gains `flow`,
  `push` and the `woken` line. The registry rows and `docs/compliance.md` moved in the same
  commit, as did `sipx-ua`'s `# Stability` section and `website/docs/reference/cli.md`. The
  failing-first test is named above. Deliberately out of scope, per the story: the registrar
  side of both RFCs, and driving flow keep-alives from `register --keep-alive` — the CLI
  reports whether a flow exists; holding it open across a long-lived registration is the
  keep-alive loop's question, not this story's.
- **Resumed after the first implementor was interrupted, and its work re-verified rather than
  trusted.** The commit that reads as finished (`67bd291`) had never been gated: a run on it fails
  `clippy`, `test` and `msrv` identically, because `reachability` elided a lifetime that
  `-D elided-lifetimes-in-paths` rejects, so **`sipx-cli` did not compile under CI's flags and the
  acceptance test above had never once executed**. Two functions also broke
  `-D clippy::too_many_lines` (`register::run` at 106, the test at 109). `965372d` fixes all three
  by naming the lifetime the way `dial.rs` already does and by extracting `report_wake`,
  `next_register` and `assert_outbound_push_register` — no change to what the command sends or
  reports.
- **The failing-first test is now verified on a real run**, not asserted. With the merge base's
  implementation (`5363747`) restored under this branch's test file,
  `cargo test -p sipx-cli --all-features --test cli register_over_a_flow_keeps_it_and_a_push_wakes_it`
  fails at `cli.rs:218` with "RFC 5626 §4.2's flow number is missing — nothing built the Outbound
  config: Contact: <sip:alice@127.0.0.1:57738>" — the missing capability, not a compile error. It
  passes on the branch.
- **`docs/maturity.md` is left alone deliberately**, and the gate's `maturity` and `maturity tests`
  steps are red because of it. The drift is *not* this branch's: `./scripts/maturity.py --check`
  fails identically at the merge base and on `main` (`7b8e5e1`). The drifting line is the
  "Discovery versus closure" table's row for **today**, a whole-board daily Filed/Closed aggregate
  that every concurrent story perturbs — regenerating it here would bake in a snapshot that is
  stale the moment the next story lands, and would collide with the sibling implementor on the same
  line. It belongs to whoever integrates, not to one story. (Resolved on `main` in `cffb6ed`, after
  this branch's base — CI's docs job had indeed been failing independently of any story.)
- **Review round 1: the notes under-disclosed what Outbound still lacks a caller for.** The code was
  confirmed good; the fix was entirely in what the claims say. RFC 5626's `uac` is earned by the
  *registration* — `+sip.instance`, `reg-id`, the `outbound` option tag, and §6 acceptance read back
  — and by nothing more. Verified by grep that three parts of the UA side remain reached only from
  `sipx-ua`'s own tests: `UserAgent::keepalive_after` (§4.4, `agent.rs:281`), `Flows`/`Attempt`
  (§4.5 backoff and one registration per proxy) and `UserAgent::dialog_contact`'s `ob` (§4.3). The
  registry note now separates "has a caller" from "implemented and reached by nothing above the
  crate" instead of listing both as the UA side, and `sipx-ua`'s `# Stability` section names that
  remainder Experimental under the crate's own rule rather than letting one `Supported` line cover
  it. `website/docs/reference/cli.md`'s refusal list gained the non-URN `--instance` case it had
  omitted. No behaviour changed in this pass.

## Notes
- **The eighth instance of the recurring defect** — after ICE (`M-27`), UPDATE (`S-22`), DTLS-SRTP
  (`M-28`), the SDES answer check (`M-29`), RFC 8122, Opus (`M-30`) and digest for calls (`S-28`). The
  capability is implemented and tested in `sipx-ua` and nothing above it can select it.
- **It is the case the *path* check could not see, and the reason `X-37` exists.** RFC 5626 and 8599
  satisfied `unreachable_claims` by citing `crates/sipx-cli/src/register.rs`, a genuine caller above
  the call layer — for a plain registration. The rows were honest and the capability still unreachable.
- The registrar side is deliberately out of scope on both rows and stays out: flow tokens in Path
  (`5626` §5), and minting PURRs / the push-bucket hold (`8599` §4.2, §5.6). sipx is a UA, not a
  registrar.
- Priority 4: Outbound is the common answer to "a NAT eats my registration", which is most real-world
  SIP. It matters to users sooner than the other M-series media stories.
