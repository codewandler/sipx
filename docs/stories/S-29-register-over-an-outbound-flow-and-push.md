---
id: S-29
title: Register over an Outbound flow, and let a registration wake on push
pillar: Signalling
status: in-progress
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
- [ ] **A registration can be placed over an Outbound flow.** `Config::with_outbound` exists
      (`crates/sipx-ua/src/agent.rs:109`) and has no caller outside `crates/sipx-ua/tests/outbound.rs` —
      verified by grep when `X-37` demoted the row. The natural caller is `sipx-cli`'s `register`
      (`crates/sipx-cli/src/register.rs`), which constructs a `UserAgent` with a plain `Config`.
- [ ] **A registration can declare push parameters and be woken.** `Config::with_push`
      (`crates/sipx-ua/src/agent.rs:156`) and `UserAgent::woken` (`agent.rs:421`) are exercised by
      `crates/sipx-ua/tests/push.rs` and by nothing above them.
- [ ] **RFC 5626 and 8599 go back to `uac` in the same commit that makes it true**, and
      `docs/compliance.md` regenerates with it. `X-37` demoted both to no roles as the honest state —
      not as the verdict. Restoring them without the wiring would be the exact defect the reachability
      check exists to catch, and `sipx-ua`'s `# Stability` section already marks these experimental for
      the same reason, which must move with it.
- [ ] The CLI flags follow the existing vocabulary and are documented in
      `website/docs/reference/cli.md`. `--outbound` and `--push` (or the forms that fit) must not be
      accepted-and-dropped the way `--password` was before `P-7`.
- [ ] Failing-first test: a registration over a flow, asserted to keep the flow and be woken. It cannot
      pass today because no caller builds the config. Name it.

## Progress
- Not started. Filed at `X-37`'s close, which demoted the two rows to no roles as the honest state and
  found that `sipx-ua`'s own crate doc already said why.

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
