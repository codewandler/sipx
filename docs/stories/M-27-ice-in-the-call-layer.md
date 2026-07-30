---
id: M-27
title: Offer and answer ICE from a call
pillar: Media
status: done
priority: 3
design: docs/specs/ice.md
epic: ice
areas: [sipx-call, sipx-media]
note: found by M-22 — ICE works and is reachable only through sipx-media's API; no call places one with it
---

# Offer and answer ICE from a call

## Goal
Let a call actually use ICE. `M-19` through `M-22` built gathering, the agent, the STUN codec and
the driver, and nothing in `sipx-call` offers or answers a candidate — so the NAT traversal the
epic delivered is unreachable from the layer applications program against.

## Acceptance
- [x] `sipx-call`'s offer/answer path gathers candidates and puts them in the offer, and answers an
      offer that carries them, using `MediaPort::gather` and `sipx_media::ice::negotiate` as they
      exist. This story wires what `M-22` built; it does not re-decide it.
- [x] Whether a call uses ICE is the application's choice, with a stated default. `MediaPolicy`
      carries `IcePolicy::{Disabled, Host, Stun}`, and `Disabled` is the default. A stack that
      required ICE would regress every peer that does not speak it; one that never offers it leaves
      `M-19`…`M-22` dead code.
- [x] **The no-ICE path stays byte-identical in behaviour** — nothing offered, no checks, no timers,
      symmetric RTP. Held by `the_default_call_path_puts_no_ice_on_the_wire`, which asserts the offer
      carries no `a=ice-` and no `a=candidate:`, and by the existing media suites passing unchanged.
- [x] A STUN server is configuration, not a constant, and its absence degrades to host candidates
      rather than failing the call — `an_unavailable_stun_server_degrades_to_host_candidates`.
- [x] The RFC 8445 and 8839 registry notes are updated to say a call can now do this — both rows now
      carry `roles = ["uac", "uas"]` and cite `crates/sipx-call/tests/ice_call.rs`. Both stay
      `partial`: restart (`M-23`), relayed candidates (`M-24`) and the lite role are still absent.
- [x] Failing-first test: a call placed between two endpoints whose host candidates cannot reach
      each other completes over a nominated pair —
      `a_call_uses_a_nominated_pair_when_both_host_candidates_are_silent`.

## Progress
- 2026-07-30: started from `ice.md` §13.4. The call-layer policy and initial-exchange lifecycle are
  specified before the failing-first call test and wiring.
- 2026-07-30: **done.** Closing it needed three defects fixed that the implementation left behind,
  all found by running the gate rather than the crate's own tests:
  - `clippy::too_many_lines` on `dial_with` (105) and `answer_negotiated` (127). Extracted
    `answer_gathering`, `ok_with_answer`, `ack_then_bye` and `rejection`. The first two of those
    also removed real duplication: the answerer's "when do I gather?" rule existed in two places
    that had already drifted to two spellings (`matches!(.., Ice { .. })` and `runs_ice()`), and the
    acknowledge-then-BYE block existed twice verbatim.
  - `an_unavailable_stun_server_degrades_to_host_candidates` failed under a full-workspace run with
    1280 of 1600 samples. Not a timing window — the test played exactly the clip it required, so it
    asserted **lossless UDP delivery**, and a dropped packet does not arrive later however long the
    bound. Every test in the file now plays a longer clip and asserts on a `REQUIRED` prefix, which
    is the shape the two nomination tests already had. Reads with `X-44`.
  - `clippy::similar_names` on `caller`/`callee`, latent because clippy stopped at the lib errors
    before it ever compiled the test target. Allowed with the same wording as `call.rs`.

## Notes
- Found by `M-22`, which recorded it plainly: "`MediaPort::gather` is not wired into `sipx-call`'s
  offer/answer — ICE is reachable through `sipx-media`'s API and no call places one with ICE yet."
  That was correct scoping — `sipx-call` belonged to `S-23` in that wave — not an oversight.
- **Reads with `C-2`.** Both change what `sipx-call` puts in an offer and both touch the media
  session's lifecycle; if they run near each other, one of them is rebasing.
- The remaining ICE gaps are separate and already filed: restart is `M-23`, relayed candidates are
  `M-24`. This story is only the call-layer wiring.
- Priority 3: it is what turns four merged stories into a feature a user can reach, which makes it
  worth more than its position in the ICE epic suggests.
