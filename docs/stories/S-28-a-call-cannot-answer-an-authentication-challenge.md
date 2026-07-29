---
id: S-28
title: Answer a 401 or 407 on an outbound INVITE
pillar: Signalling
status: ready
priority: 5
design: docs/specs/sip-auth.md
epic: sip-core
areas: [sipx-call, sipx-cli]
note: found while closing P-7 — sipx-call has no credential type and no 401/407 path at all, so a challenged call fails outright; the digest machinery exists in sipx-ua and nothing above the registration path can reach it
---

# Answer a 401 or 407 on an outbound INVITE

## Goal
Let a call authenticate. A challenged INVITE currently fails, because `sipx-call` has no way to hold
credentials and no path that recognises a challenge.

## Acceptance
- [ ] **A call challenged with 407 (or 401) retries with credentials and connects.** Verified absent
      today: `grep -rn "Credentials" crates/sipx-call/src` returns **nothing**, and so does a grep for
      `407`, `Unauthorized` or `ProxyAuth` in `crates/sipx-call/src/call.rs`. There is no partial
      implementation to finish — the concept does not exist in that crate.
- [ ] **The digest machinery is reused, not rewritten.** `sipx-ua` already answers challenges for
      REGISTER (`Credentials`, `challenge::Authenticator`), and RFC 2617/7616/8760 are cited against it
      in the registry. This story is about *reaching* that from a call, which is the project's recurring
      shape: a capability implemented in one crate with no caller above it. Note that `sipx-call` does
      **not** depend on `sipx-ua` — they are siblings — so where the shared code lives is the first real
      decision, not an implementation detail.
- [ ] **Credentials are the application's to supply, and never logged.** The `register` path already
      states the rule that matters and should be copied verbatim in spirit: a password in argv is
      world-readable, so `SIPX_PASSWORD` is the documented route and the flag is the convenience
      (`crates/sipx-cli/src/register.rs:22,51-53`).
- [ ] **`sipx dial --password` starts working, and its refusal is removed in the same commit.** `P-7`
      made the flag an explicit `Usage` error rather than letting it be silently dropped; that refusal
      is a placeholder for this story and must not outlive it. The test
      `the_dial_command_refuses_a_password_it_cannot_use` is the one to delete.
- [ ] **A challenge that cannot be answered still fails cleanly**, with `Exit::Unauthorized` (4) rather
      than a timeout. Today a 407 produces neither — the call just ends.
- [ ] Failing-first test: a call against a peer that challenges, asserted to connect. It cannot pass
      today because credentials cannot be supplied. Name it.

## Progress
- Not started. Filed while closing `P-7`, which found the flag/handler asymmetry and could only refuse
  the flag honestly, because the feature behind it does not exist.

## Notes
- **This is the seventh instance of the recurring defect** — after ICE (`M-27`), UPDATE (`S-22`),
  DTLS-SRTP (`M-28`), the SDES answer check (`M-29`), RFC 8122, and Opus (`M-30`): a capability
  implemented in one crate that nothing above it can select. Digest authentication is implemented,
  tested and cited in the compliance table; a *call* cannot use it.
- **It is also the case `X-33`'s reachability check cannot see**, and worth reading as a worked example
  before `X-37`: RFC 2617, 7616 and 8760 satisfy the check by citing
  `crates/sipx-cli/src/register.rs`, which is a genuine caller above the call layer — for
  registration. The rows are honest and the capability is still unreachable from a call. A path check
  cannot tell those apart; a caller check could.
- `X-33`'s implementor also found that `crates/sipx-cli/tests/cli.rs` contains no `password`, `401`,
  `407` or `Authorization` test anywhere, so there is no CLI coverage of authentication to extend —
  only coverage to write. A design claim once cited `cli.rs:116` as exercising it; that line is
  `register_advertises_this_client_in_via_and_contact`.
- Priority 5: it is a real feature rather than a correction, and the honest refusal `P-7` put in place
  means nobody is currently misled. It matters most for anyone whose provider challenges calls, which
  is most commercial SIP trunks.
