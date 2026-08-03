---
id: S-28
title: Answer a 401 or 407 on an outbound INVITE
pillar: Signalling
status: done
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
- [x] **A call challenged with 407 (or 401) retries with credentials and connects.** Verified absent
      today: `grep -rn "Credentials" crates/sipx-call/src` returns **nothing**, and so does a grep for
      `407`, `Unauthorized` or `ProxyAuth` in `crates/sipx-call/src/call.rs`. There is no partial
      implementation to finish — the concept does not exist in that crate.
- [x] **The digest machinery is reused, not rewritten.** `sipx-ua` already answers challenges for
      REGISTER (`Credentials`, `challenge::Authenticator`), and RFC 2617/7616/8760 are cited against it
      in the registry. This story is about *reaching* that from a call, which is the project's recurring
      shape: a capability implemented in one crate with no caller above it. Note that `sipx-call` does
      **not** depend on `sipx-ua` — they are siblings — so where the shared code lives is the first real
      decision, not an implementation detail.
- [x] **Credentials are the application's to supply, and never logged.** The `register` path already
      states the rule that matters and should be copied verbatim in spirit: a password in argv is
      world-readable, so `SIPX_PASSWORD` is the documented route and the flag is the convenience
      (`crates/sipx-cli/src/register.rs:22,51-53`).
- [x] **`sipx dial --password` starts working, and its refusal is removed in the same commit.** `P-7`
      made the flag an explicit `Usage` error rather than letting it be silently dropped; that refusal
      is a placeholder for this story and must not outlive it. The test
      `the_dial_command_refuses_a_password_it_cannot_use` is the one to delete.
- [x] **A challenge that cannot be answered still fails cleanly**, with `Exit::Unauthorized` (4) rather
      than a timeout. Today a 407 produces neither — the call just ends.
- [x] Failing-first test: a call against a peer that challenges, asserted to connect. It cannot pass
      today because credentials cannot be supplied. Name it.

## Progress
- Failing-first test
  `a_call_challenged_by_a_proxy_retries_with_credentials_and_connects` initially failed to compile:
  `sipx_call::Credentials` and `DialOptions::with_credentials` did not exist. It now drives a real
  407 exchange, verifies the digest independently with `Authenticator`, holds Call-ID and From
  stable, increments CSeq, changes the Via branch, connects, and carries G.711 audio.
- Wrote `docs/specs/sip-auth.md` before implementation: shared types, credential ownership, 401/407
  header mapping, bounded initial/authenticated/stale/422 states, retry identity, and byte vectors.
- Moved the pure client digest implementation into `sipx-sip::auth`; `sipx-ua::auth` re-exports it
  and retains only runtime entropy. Registration and calls now use the same challenge parser,
  algorithm selection and response arithmetic without either sibling depending on the other.
- `DialOptions::with_credentials` supplies application-owned credentials. The password is redacted
  from `Debug`, and no authentication path logs it or the rendered authorization value.
- `sipx dial --password` and `SIPX_PASSWORD` now construct credentials from the `--from` username.
  The `P-7` refusal test and branch are removed. Binary tests prove both credential routes connect
  through a 407 and a challenge with no password exits immediately as `Unauthorized` (4).

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
