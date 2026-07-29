# Spec: ICE

**Status:** normative, and **partly implemented**. The SDP grammar (§13) is
[`sipx_sdp::ice`](../../crates/sipx-sdp/src/ice.rs) via `M-19`; the STUN profile (§11) is
[`sipx_media::ice::stun`](../../crates/sipx-media/src/ice/stun.rs) via `M-20`; the agent (§2, §4 …
§10, §14) is [`sipx_media::ice`](../../crates/sipx-media/src/ice/mod.rs) via `M-21`. Still unbuilt:
the driver on the media port (`M-22`), restart (`M-23`) and the relayed candidate (`M-24`).
`M-16` was cut as one story, stopped at this spec, and asked to be split; its `## Progress` records
why and along which of these section boundaries. **Three sections have since been corrected by the
stories implementing against them** — §6.2 by `M-19`, §11.1 by `M-20`, §6.5 by `M-21`; each carries
a dated attribution. · **Crates:** `sipx-sdp` (grammar), `sipx-media` (agent and driver) ·
**Story:** [M-16](../stories/M-16-ice.md) · **Design:** [media](../designs/media.md)

## 1. Normative references

- **RFC 8445** — Interactive Connectivity Establishment. §5.1.1 (gathering), §5.1.1.3
  (foundations), §5.1.2.1 (the priority formula), §6.1.1 (roles), §6.1.2.2 … §6.1.2.6 (checklists
  and pair states), §7 (checks), §7.3.1.1 (role conflict), §8.1.1 (nomination), §11 (keepalives),
  §14 (Ta and RTO).
- **RFC 8839** — SDP Offer/Answer Procedures for ICE. §4.2 (initial offer/answer), §4.4.1.1.1 (ICE
  restart), §5.1 … §5.6 (the attributes), §6 (keepalives), §7 (SIP considerations).
- **RFC 5389** — STUN. §6 (header), §15.3 (`USERNAME`), §15.4 (`MESSAGE-INTEGRITY`), §15.5
  (`FINGERPRINT`), §15.6 (`ERROR-CODE`), §15.2 (`XOR-MAPPED-ADDRESS`), §7.2.1 (retransmission).
  RFC 8445 references RFC 5389, not RFC 8489, so `MESSAGE-INTEGRITY` here is HMAC-SHA1 and there is
  no `MESSAGE-INTEGRITY-SHA256`.
- **RFC 5769** — STUN test vectors. §2.1 and §2.2 are the byte-level vectors §13 derives tests
  from; §2.1's sample request is itself an ICE connectivity check.
- **RFC 5764 §5.1.2** — telling STUN from DTLS from RTP on one port. Already implemented as
  [`sipx_media::dtls::classify`](../../crates/sipx-media/src/dtls/mod.rs).
- RFC 8421 — dual-stack local-preference guidance, referenced by §5.1.2.1 for the local preference.

**Out of scope, deliberately:**

- **Trickle ICE (RFC 8838/8840).** A separate document with a separate offer/answer model:
  candidates arrive after the offer, which changes when a checklist may be considered complete and
  adds an `end-of-candidates` signal to the grammar. Nothing has asked for it, and half of it is
  worse than none — an agent that accepts trickled candidates but never sends them advertises a
  capability it does not have. If it is ever wanted it is its own spec, not a section here.
- **Running a TURN relay.** Gathering and *using* a relayed candidate against a configured relay is
  in scope for the story that adds it; being a relay is not. Note that gathering one at all means a
  TURN client — RFC 8656, a third protocol — which is why it is its own story and not a bullet.
- **ICE for the signalling path.** RFC 8839 §7: "ICE is not intended for NAT traversal for SIP
  signaling, which is assumed to be provided via another mechanism [RFC5626]." That mechanism is
  `T-15` and [`sipx_transport::stun`](../../crates/sipx-transport/src/stun.rs); the two keep-alives
  are separate and stay separate.
- **The ICE-lite role for sipx itself.** Deferred with a reason; see §12.

## 2. Sans-IO contract

The agent is a state machine in the shape the transaction machines already use
([sip-transaction](sip-transaction.md) §2). It reads no clock, owns no socket, and holds no
`tokio` types. Time enters as a fired timer; datagrams enter as bytes with a source address.

```rust
enum Input {
    /// The far end's ICE parameters, from an offer or an answer.
    RemoteDescription { ufrag: String, pwd: String, candidates: Vec<Candidate>, lite: bool },
    /// A local candidate the driver gathered (host now, server-reflexive when STUN answers).
    LocalCandidate(Candidate),
    /// Gathering will produce nothing further.
    GatheringDone,
    /// A datagram that `dtls::classify` called `Stun`, and where it came from.
    Datagram { from: SocketAddr, on: LocalBase, bytes: Vec<u8> },
    /// Media went out on the selected pair; resets the keepalive timer (§11).
    DataSent { pair: PairId },
    TimerFired(Timer),
}

enum Output {
    /// Send these bytes from this local base to this address. The driver owns the socket.
    Send { on: LocalBase, to: SocketAddr, bytes: Vec<u8> },
    SetTimer { timer: Timer, after: Duration },
    ClearTimer(Timer),
    /// A component has a selected pair: media goes here now, in both directions.
    Selected { component: ComponentId, local: LocalBase, remote: SocketAddr },
    /// ICE failed for a component. The call layer decides what that means.
    Failed { component: ComponentId },
}
```

Outputs come back in the order the driver must perform them, and `Send` precedes the `SetTimer`
that retransmits it — the same rule and for the same reason as the transaction machines.

`LocalBase` is an index into the sockets the driver bound, not a socket. The agent never learns
what a socket is; it says "the one you called base 0" and the driver knows which.

## 3. Types

| Type | Fields | Source |
|---|---|---|
| `CandidateType` | `Host`, `ServerReflexive`, `PeerReflexive`, `Relayed` | §5.1.1, §5.1.1.2 |
| `Candidate` | `address`, `base`, `type`, `foundation`, `component`, `priority`, `transport`, `related` | §5.1.1, RFC 8839 §5.1 |
| `ComponentId` | 1 = RTP, 2 = RTCP | RFC 8839 §5.1 |
| `Foundation` | 1–32 `ice-char` | §5.1.1.3, RFC 8839 §5.1 |
| `CandidatePair` | `local`, `remote`, `priority`, `state`, `valid`, `nominated` | §6.1.2.2 figure 5 |
| `PairState` | `Frozen`, `Waiting`, `InProgress`, `Succeeded`, `Failed` | §6.1.2.6 |
| `ChecklistState` | `Running`, `Completed`, `Failed` | §6.1.2.1, §7.2.5.4 |
| `Role` | `Controlling`, `Controlled` | §6.1.1 |

Only `transport = UDP` exists. RFC 8839 §5.1's grammar permits a `transport-extension` token, so
the parser must **accept and discard** a candidate naming any other transport rather than reject
the whole description — a peer offering an ICE-TCP candidate alongside UDP ones is offering
something usable.

## 4. Priority (§5.1.2.1)

The formula, exactly as printed, because the ordering it produces is the only thing that makes two
independent implementations agree on which pair wins:

```
priority = (2^24)*(type preference) +
           (2^8)*(local preference) +
           (2^0)*(256 - component ID)
```

`priority` is a positive integer up to 2^31 − 1 (RFC 8839 §5.1), so it is computed and carried as
`u32` and range-checked on parse.

sipx's preferences, from §5.1.2.2's recommendations:

| Type | Type preference | Why |
|---|---|---|
| host | 126 | §5.1.2.2's recommended value |
| peer-reflexive | 110 | MUST be higher than server-reflexive (§5.1.2.1) |
| server-reflexive | 100 | §5.1.2.2's recommended value |
| relayed | 0 | Last resort — it costs a relay's bandwidth |

**Local preference.** 65535 when there is one address (§5.1.2.1 SHOULD). With several, the value
MUST be unique per candidate of the same type and component, so sipx assigns 65535, 65534, …
descending over the interfaces **sorted by address bytes**, not by enumeration order: an ordering
that depends on what the OS hands back first makes the same host produce different priorities on
different runs, and the priorities are what the far end reasons about.

Worked vector (a test asserts on these three numbers, not on a formula re-typed into the test):

| Candidate | Type pref | Local pref | Component | Priority |
|---|---|---|---|---|
| host, single address, RTP | 126 | 65535 | 1 | 2130706431 |
| host, single address, RTCP | 126 | 65535 | 2 | 2130706430 |
| server-reflexive, single address, RTP | 100 | 65535 | 1 | 1694498815 |

The third is the number RFC 8839 §5.1 prints in its own example line, which is why it is here.

**The priority in a check is not the candidate's priority.** §7.1.1: `PRIORITY` in a Binding request
is computed by the same formula for the local candidate "but with the candidate type preference of
peer-reflexive candidates" — 110, whatever the candidate actually is. It has to be, because that is
the priority the *peer* will assign the peer-reflexive candidate it may learn from this very check,
and the two ends have to agree on it. An implementation that sends the candidate's own priority
here produces a peer-reflexive candidate the far end prioritises differently from us, and the two
checklists diverge.

## 5. Foundations (§5.1.1.3)

Two candidates share a foundation when **all** of: same type; bases with the same IP address (ports
may differ); for reflexive and relayed candidates, the same STUN or TURN server IP; same transport
protocol. Anything else is a different foundation.

The value itself is arbitrary — 1–32 `ice-char` — so sipx computes it as a small decimal counter
over the distinct tuples above, allocated in the order candidates are gathered. A hash would also
satisfy the grammar and would be longer on the wire for no gain.

Foundations are not cosmetic: §6.1.2.6 unfreezes exactly one pair per foundation, so getting them
wrong makes ICE either check far too much or check nothing.

## 6. Checklists

### 6.1 Forming pairs (§6.1.2.2)

Each local candidate is paired with each remote candidate **of the same component and the same IP
address family**. IPv6 link-local addresses MUST NOT be paired with anything but link-local
addresses.

If one side offers no RTCP component, the number of components for the stream is reduced to the
minimum across both agents. sipx offers component 2 only when [`MediaPort`](../../crates/sipx-media/src/session.rs) actually got the
control port; when it did not, it offers component 1 alone and the peer's RTCP candidates go
unpaired, which is exactly the case §6.1.2.2 describes.

### 6.2 Pair priority and order (§6.1.2.3)

With `G` the controlling agent's candidate priority and `D` the controlled agent's:

```
pair priority = 2^32*MIN(G,D) + 2*MAX(G,D) + (G>D?1:0)
```

It fits in `u64`, with room to spare, because §4 bounds a priority at 2^31 − 1. The expression is
bounded above by `2^32*(2^31−1) + 2*(2^31−1) + 1` = `2^63 − 1`, and that bound is approached and
never reached: the `G>D` term is zero exactly when `G` and `D` are equal, so the largest value any
pair of in-range priorities actually produces is `2^63 − 2`, at `G = D = 2^31 − 1`.

Accept an unchecked priority from a peer instead — RFC 8839 §5.1's grammar is `1*10DIGIT`, so
`4294967295` parses — and the same expression overflows. The overflow is **not** one step past
2^31 − 1; the arithmetic is still exact for operands up to 4294967294. It is `u32::MAX` on both
sides that breaks it: `2^32*(2^32−1) + 2*(2^32−1)` is `2^64 + 2^32 − 2`, which is past `u64::MAX`.
That is a narrow window, and it is reachable by any peer that can write ten digits.
**The range check on parse is what makes this arithmetic safe**, and in a build without overflow
checks the silent wrap reorders the checklist, which is the whole point of computing it.

*(Corrected by `M-19`, which implements the range check. The section previously stated `2^63 − 1`
as an attained maximum and implied that anything past 2^31 − 1 overflows; both were wrong, the
warning they supported was not. Asserted by
`the_priority_bound_is_what_keeps_the_pair_priority_in_a_u64` in
[`sipx_sdp::ice`](../../crates/sipx-sdp/src/ice.rs).)*

Checklists sort in **decreasing** pair priority. Ties are ordered arbitrarily but must be ordered
*stably*, so that a test asserting on a checklist gets the same answer twice.

A role change (§7.3.1.1) swaps which side is `G` and which is `D`, so **every pair priority is
recomputed and every checklist re-sorted** on a role change. Forgetting this is one of the two ways
role conflict is mishandled; the other is not detecting it at all.

### 6.3 Pruning and limiting (§6.1.2.4, §6.1.2.5)

1. For each pair whose **local** candidate is reflexive, replace that local candidate with its
   base. Checks are sent from a base; there is no socket at a reflexive address.
2. Remove a pair that is redundant with a higher-priority pair in the same checklist. Two pairs are
   redundant when their local candidates have the same base **and** their remote candidates are
   identical.
3. Discard the lowest-priority pairs until the checklist set holds at most **100** pairs
   (configurable; §6.1.2.5 requires it to be). The limit is an attack control, not tidiness: it
   bounds how many packets a hostile candidate list can make sipx send.

### 6.4 Initial states (§6.1.2.6)

All pairs start `Frozen`; every checklist starts `Running`. Then, **for each foundation**, exactly
one pair moves to `Waiting`: the first pair — ordered by lowest component ID, then highest
priority — in the first checklist that has that foundation. A pair is not unfrozen for a foundation
already unfrozen in another checklist.

### 6.5 Pair state transitions

`—` means no change. The driver performs outputs in the order given.

| State | Input | → State | Outputs / effect |
|---|---|---|---|
| Frozen | a pair with the same foundation succeeded (§7.2.5.3.3) | Waiting | — |
| Frozen | Ta fires, its checklist has no `Waiting` pair, and no pair anywhere in the set shares its foundation in `Waiting` or `In-Progress` (§6.1.4.2 step 2) | Waiting | — |
| Waiting | Ta fires and this is the highest-priority `Waiting` pair | In-Progress | send check; set RTO |
| In-Progress | RTO fires, attempt < Rc | — | resend check; set RTO ×2 |
| In-Progress | RTO fires, attempt = Rc | Failed | update checklist state |
| In-Progress | 2xx, mapped address is a known local candidate | Succeeded | add to valid list; unfreeze same-foundation pairs |
| In-Progress | 2xx, mapped address is new | Succeeded | learn a peer-reflexive local candidate (§7.2.5.3.1); the *valid* pair is the one built from it |
| In-Progress | 2xx whose source/destination are not symmetric with the request's | Failed | §7.2.5.2.1 |
| In-Progress | 487 Role Conflict | Waiting | switch role; **new tiebreaker**; recompute all pair priorities; re-sort; enqueue this pair as a triggered check (§7.2.5.1) |
| In-Progress | any other error, or timeout | Failed | update checklist state |
| any | inbound check from an unknown remote address | — | learn a peer-reflexive **remote** candidate (§7.3.1.3); enqueue a triggered check |
| Succeeded | 2xx to a check that carried `USE-CANDIDATE` | — | set `nominated`; component concluded (§7.2.5.3.4) |
| Succeeded | nominated check fails | Failed | remove from valid list; **checklist → Failed** (§7.2.5.3.4) |

Triggered checks (§7.3.1.4) jump the queue: a pair with a triggered check is sent at the next Ta
tick ahead of every `Waiting` pair, whatever its priority. This is what makes ICE converge quickly
rather than in checklist order.

*(Corrected by `M-21`, which implements the machine. The table had **one** Frozen row, naming
§7.2.5.3.3's unfreeze — the one that fires when a pair of the same foundation *succeeds*. §6.1.4.2
step 2 is a second unfreeze and the table did not have it, so a machine written from this section
alone deadlocks: §6.1.2.6 unfreezes each foundation exactly once, so when that one pair fails, every
remaining pair of that foundation stays Frozen for the rest of the session and ICE reports a failure
for a path it never finished checking. The row above is §6.1.4.2 step 2 verbatim, including that the
"is any pair of this foundation busy" test is over the whole checklist set and not over one
checklist. Asserted by `a_foundation_whose_only_unfrozen_pair_failed_is_thawed_again` and
`nothing_is_thawed_while_the_foundation_still_has_a_check_outstanding` in
[`sipx_media::ice::checklist`](../../crates/sipx-media/src/ice/checklist.rs).)*

## 7. Roles and role conflict

### 7.1 Determining the role (§6.1.1)

| Local | Remote | Local role |
|---|---|---|
| full | full | **controlling** if it sent the initial offer, else controlled |
| full | lite | **controlling**, always |
| lite | full | controlled |
| lite | lite | controlling if it sent the initial offer |

sipx is always full (§12), so it is controlling whenever it offered *or* whenever the peer said
`a=ice-lite`. The role persists for the session and may only be redetermined at an ICE restart.

"The offerer controls" is the right answer for the first two rows and the wrong mechanism: two
agents can both believe they offered — third-party call control, glare, a re-INVITE crossing — and
two controlling agents never converge because neither will accept the other's nomination. That
failure appears only when both ends run the same stack, which is precisely the configuration sipx's
own tests run.

### 7.2 The tiebreaker

A 64-bit value chosen at random per ICE session (§7.1.3), carried in `ICE-CONTROLLING` on every
check the controlling agent sends and in `ICE-CONTROLLED` on every check the controlled agent sends.

It is regenerated on an ICE restart **and on receiving a 487**: §7.2.5.1 says the agent "MUST change
the tiebreaker value" when it switches role. Keeping the old value re-loses the same comparison
against a peer that has not switched, and the two agents ping-pong roles until the checklist fails.

### 7.3 Repairing a conflict (§7.3.1.1)

The table **is** the specification; the code is written from it and the tests walk it row by row.
`T` is our tiebreaker, `V` the value in the attribute.

| Our role | Attribute in the request | Condition | Action |
|---|---|---|---|
| Controlling | `ICE-CONTROLLING` | T ≥ V | 487 Role Conflict; **keep** controlling |
| Controlling | `ICE-CONTROLLING` | T < V | switch to **controlled**; answer normally |
| Controlled | `ICE-CONTROLLED` | T ≥ V | switch to **controlling**; answer normally |
| Controlled | `ICE-CONTROLLED` | T < V | 487 Role Conflict; **keep** controlled |
| Controlling | `ICE-CONTROLLED` | — | no conflict |
| Controlled | `ICE-CONTROLLING` | — | no conflict |
| either | neither attribute | — | no conflict; the peer is not doing role signalling |

Note `≥`, not `>`: with equal tiebreakers both agents must not switch, or they swap roles and the
conflict repeats forever. Receiving a 487 has the mirror effect (§7.2.5.1) — the sender switches to
the role opposite the attribute it sent, picks a **new** tiebreaker, recomputes every pair priority
(§6.2), and re-runs that check as a triggered one so the new role goes out immediately.

The remaining §7.3.1 processing runs even when the role changed, provided a success response was
generated.

## 8. Nomination — regular only (§8.1.1)

The controlling agent, once it has decided, **repeats the check that produced the pair** with
`USE-CANDIDATE` set, by enqueueing that pair on the triggered-check queue. When that check succeeds
the pair's nominated flag is set; when every component has a nominated pair the checklist is
`Completed` and those pairs are the selected pairs.

Having nominated a pair for a component, the agent **MUST NOT** nominate another for that component
in the same ICE session. Changing the selection requires an ICE restart.

**Aggressive nomination is not implemented, and must not be added behind an option.** §4 says it
"has been deprecated in this specification". §8.1.1 explains why it is no longer even useful: "In
this specification, data can always be sent on any valid pair, without nomination." An option to
turn it on is an option to make sipx re-nominate mid-session, which is the behaviour `ice2` exists
to stop; sipx therefore sends `a=ice-options:ice2` (RFC 8839 §5.6) and offers no such switch. The
controlled side must still tolerate a peer that nominates more than once — §8.1.1 requires
selecting the highest-priority nominated pair in that case — because tolerating a legacy peer is
not the same as being one.

**The stopping criterion.** §8.1.1 leaves this to local optimisation and requires only that the
agent eventually picks exactly one pair. sipx's rule, so that the choice is testable rather than
emergent: nominate when every component has at least one valid pair **and** either every pair of
higher priority than the best valid pair has reached `Failed`, or `Tn` has elapsed since the first
valid pair appeared.

## 9. Timers

| Symbol | Value | Meaning |
|---|---|---|
| Ta | 50 ms | Pacing: one check leaves per tick, across all checklists (§14.2) |
| — | 5 ms | Floor across all agents in the process, whatever Ta is (§14.2) |
| RTO | `MAX(500ms, Ta * N * (Num-Waiting + Num-In-Progress))` | Retransmit interval, recomputed per transaction (§14.3) |
| Rc | 7 | Request transmissions before a transaction fails (RFC 5389 §7.2.1) |
| Rm | 16 | Multiplier on the final wait (RFC 5389 §7.2.1) |
| Tr | 15 s | Keepalive interval on a pair carrying data (§11) |
| Tn | 1 s | sipx's own: how long the controlling agent keeps checking after the first valid pair before nominating (§8) |

`N` is the total number of checks to be performed. RTO is **not** constant: §14.3 says "the RTO will
be different for each transaction as the number of checks in the Waiting and In-Progress states
change", so it is computed when a check is sent, not once. `MUST NOT` be below 500 ms.

Everything here is configurable. Nothing may be a literal in the state machine — the same rule as
the transaction timers, for the same reason.

## 10. Keepalives (§11, RFC 8839 §6)

An agent MUST send a keepalive on each pair used for sending data if nothing has been sent on it in
the last Tr seconds. Once pairs are selected, keepalives go only on those.

The keepalive is a **STUN Binding Indication**, and its shape is unusually constrained: it "MUST NOT
utilize any authentication mechanism", it SHOULD carry `FINGERPRINT` to aid demultiplexing, and it
SHOULD NOT carry anything else. An indication draws no response, so it proves nothing about the path
— it only holds the NAT binding open. Sent from and to the selected pair's addresses.

An agent must still be ready to receive a full connectivity check at any time on a selected pair; it
answers per RFC 5389 and ICE processing is otherwise unaffected.

This is a different keepalive from RFC 5626 §4.4.2's, which holds the *signalling* flow open and
does expect a response. Both exist; neither substitutes for the other.

## 11. The STUN profile

Connectivity checks are STUN Binding transactions over the media port, so both roles are needed:
sipx sends checks and answers them. [`sipx_transport::stun`](../../crates/sipx-transport/src/stun.rs)
is a client with no attributes and no credentials, and says so in its own header comment — it is
reused for what it does (the header, `is_stun`, `XOR-MAPPED-ADDRESS` decoding, the RFC 5769 vectors)
and extended nowhere. ICE's additions are a separate module.

### 11.1 Attributes

| Attribute | Type | On | Reference |
|---|---|---|---|
| `PRIORITY` | 0x0024 | every check | §7.1.1 |
| `USE-CANDIDATE` | 0x0025 | flag; **controlling agent's nominating check only** | §7.1.2 |
| `ICE-CONTROLLED` | 0x8029 | every check from the controlled agent | §7.1.3 |
| `ICE-CONTROLLING` | 0x802a | every check from the controlling agent | §7.1.3 |
| `USERNAME` | 0x0006 | every check; **never a response** | RFC 5389 §15.3, §10.1.2 |
| `MESSAGE-INTEGRITY` | 0x0008 | every check and every response | RFC 5389 §15.4 |
| `FINGERPRINT` | 0x8028 | every check, response and keepalive indication | RFC 5389 §15.5 |
| `ERROR-CODE` | 0x0009 | error responses; 487 for role conflict, 401 for a bad credential | RFC 5389 §15.6 |
| `XOR-MAPPED-ADDRESS` | 0x0020 | success responses | RFC 5389 §15.2 |

`USE-CANDIDATE` is a flag: zero-length value. The controlled agent MUST NOT send it.

A response carries `MESSAGE-INTEGRITY` and no `USERNAME`. Both RFCs say so outright — RFC 5389
§10.1.2: "Any response generated by a server MUST include the MESSAGE-INTEGRITY attribute … The
response MUST NOT contain the USERNAME attribute"; RFC 8445 §7.2.2: "The responses utilize the same
usernames and passwords as the requests (note that the USERNAME attribute is not present in the
response)." The credential still applies — it is the key, not an attribute — which is what the
`USERNAME` row used to conflate.

*(Corrected by `M-20`, which implements the codec. The row previously read "every check and its
response", and an encoder written from it could not have reproduced RFC 5769 §2.2 — the IETF's own
response to §2.1's request, which carries no `USERNAME`. Asserted by
`a_success_response_encodes_to_the_rfc_5769_sample_response` in
[`sipx_media::ice::stun`](../../crates/sipx-media/src/ice/stun.rs).)*

### 11.2 Credentials

Short-term credentials from the SDP. A check sent to the peer uses username
`<peer-ufrag>:<our-ufrag>` and the **peer's** password as the HMAC key; a check received is
validated against `<our-ufrag>:<peer-ufrag>` and our password. Getting the order backwards produces
an agent that answers nothing and whose own checks are all rejected, and it looks exactly like a
network problem.

`MESSAGE-INTEGRITY` is HMAC-SHA1 over the message with the length field temporarily set as though
the message ended after the `MESSAGE-INTEGRITY` attribute; `FINGERPRINT`, if present, is computed
last, over the message including `MESSAGE-INTEGRITY`, and its value is `CRC-32 XOR 0x5354554e`.
Order is fixed: `MESSAGE-INTEGRITY` then `FINGERPRINT`, both last.

Comparison of the received tag is constant-time (`subtle`), as [`sipx_sdp::fingerprint`](../../crates/sipx-sdp/src/fingerprint.rs) already
does for certificate fingerprints and for the same reason.

### 11.3 What must not happen

Every parser here eats unauthenticated datagrams from whoever can reach the media port. No
`unwrap`, no raw indexing, no length arithmetic that can wrap; a malformed message is a typed error
and a dropped datagram, never a panic and never a state change. An unauthenticated message must not
move a pair's state — that is what stops an off-path attacker steering the media path by spraying
Binding requests.

## 12. ICE-lite — deferred, with the reason

**Deferred.** sipx does not implement the lite role (§2.2, §6.2, §8.2) and does not send
`a=ice-lite`. Recorded here rather than left to a reader of the code:

- A lite agent must be on a **public address with no NAT** and never gathers, never checks, never
  nominates. It is the role of a media server or an SBC that is already reachable. sipx is a user
  agent that places and answers calls from behind NATs, which is the case ICE exists for and the
  case the lite role explicitly does not serve.
- Lite is an **endpoint-wide** property, not a per-call one (`a=ice-lite` is session-level, §5.3),
  so supporting it means a second nomination path and a second conclusion path alive in the same
  binary, for a deployment shape sipx does not have.
- Nothing is lost by not sending it: a full agent is a strict superset of what a lite agent can do.

**Interoperating with a lite peer is not deferred and is in scope.** When the peer's description
carries `a=ice-lite`, sipx takes the controlling role unconditionally (§6.1.1), and expects the peer
to answer checks and never send any. An implementation that only handles a full peer will hang
waiting for checks that a lite peer is not required to send, so this has its own test.

Revisit if sipx ever grows a media-server deployment on a public address, where the lite role would
remove the checking machinery rather than duplicate it.

## 13. SDP (RFC 8839)

The grammar is pure parsing and lives in `sipx-sdp`, which reads no clock and owns no socket. The
attributes are media-level unless stated.

### 13.1 Attributes

```abnf
candidate-attribute = "candidate" ":" foundation SP component-id SP transport SP
                      priority SP connection-address SP port SP cand-type
                      [SP rel-addr] [SP rel-port] *(SP cand-extension)
foundation          = 1*32ice-char
component-id        = 1*3DIGIT
transport           = "UDP" / token
priority            = 1*10DIGIT
cand-type           = "typ" SP ("host" / "srflx" / "prflx" / "relay" / token)
rel-addr            = "raddr" SP connection-address
rel-port            = "rport" SP port
ice-char            = ALPHA / DIGIT / "+" / "/"

remote-candidate-att = "remote-candidates:" remote-candidate 0*(SP remote-candidate)
remote-candidate     = component-id SP connection-address SP port

ice-ufrag-att = "ice-ufrag:" 4*256ice-char
ice-pwd-att   = "ice-pwd:" 22*256ice-char
ice-pacing-att = "ice-pacing:" 1*10DIGIT
ice-options   = "ice-options:" ice-option-tag *(SP ice-option-tag)
ice-lite      = "ice-lite"          ; session-level only
ice-mismatch  = "ice-mismatch"      ; media-level, answer only
```

Rules the grammar does not state and a parser gets wrong:

- `ice-ufrag` and `ice-pwd` appear at session or media level; **media level wins**, and session
  level is a default for every stream. There MUST be both for every stream, one way or the other.
- An agent MUST NOT generate an FQDN in `connection-address`, and MUST **ignore** a remote
  `candidate` line carrying an FQDN or an unsupported address family — ignore the line, not the
  description.
- `raddr`/`rport` MUST be present for `srflx`, `prflx` and `relay`, and MUST be absent for `host`.
  A privacy-preserving agent sets them to `0.0.0.0`/`::` and port `9`, which must parse.
- Unknown `cand-extension` name/value pairs MUST be ignored, not rejected. sipx keeps unknown SDP
  lines already; this is the same discipline one level down.
- `ice-ufrag` MUST NOT be sent longer than 32 characters, but up to 256 MUST be accepted.
- `remote-candidates` is included by a controlling agent in an offer **only** for a stream that is
  Completed, and MUST NOT appear otherwise.

### 13.2 Offer, answer, restart

- **Initial offer** (§4.2.1): every gathered candidate as `a=candidate`, `a=ice-ufrag`,
  `a=ice-pwd`, `a=ice-options:ice2`. The `c=`/`m=` default destination is the candidate sipx would
  use if the peer turned out not to do ICE — which for a full agent is the highest-priority one.
- **Answer** (§4.2.2): symmetric, plus the role fixed by §6.1.1.
- **Restart** (§4.4.1.1.1): **both** `ice-ufrag` and `ice-pwd` change for the stream. Only both;
  changing one is not a restart, and the same value moving between session and media level is
  explicitly not a restart. On a restart, everything is rebuilt as for an initial offer: new
  candidates, new tiebreaker, new checklists, and the role may be redetermined. Media keeps flowing
  on the old selected pair until the new ICE session selects one.
  Setting `c=` to `0.0.0.0` implies a restart, so ICE implementations MUST NOT use it for hold —
  hold is `a=inactive`/`a=sendonly` (RFC 3264).
- **`ice-mismatch`** (§5.3): the answerer reports it when the offer's default destination for a
  component had no matching `candidate` attribute. It means ICE MUST NOT be used for that stream and
  RFC 3264 procedures apply instead — which is the fallback below, arrived at by a different route.

### 13.3 A peer with no ICE

RFC 8839 §6: "An agent can determine that its peer supports ICE by the presence of 'candidate'
attributes for each media session." No `a=candidate` means no ICE, and sipx then does exactly what
it does today: send to the `c=`/`m=` address, and replace it with the source of the first RTP packet
that parses (symmetric RTP, [`sipx_media::session`](../../crates/sipx-media/src/session.rs)). No checks are sent, no keepalive indications
are sent, and no ICE timer runs.

This is the common case and it must stay the common case. **A stack that requires ICE to place a
call has regressed**, and the regression test is the existing suite: the symmetric-RTP tests must
pass unchanged, with no ICE attributes offered unless ICE is switched on.

## 14. Test vectors

Tests are derived from these, not from the implementation.

1. **RFC 5769 §2.1** — the sample request, with username `evtj:h6vY`, password
   `VOkJxbRl1RmTxUk/WvJxBt`, `PRIORITY` = `6e 00 01 ff`, `ICE-CONTROLLED` with tiebreaker
   `93 2f f9 b1 51 26 3b 36`, `MESSAGE-INTEGRITY` and `FINGERPRINT`. It is an ICE connectivity
   check, published by the IETF, with the tag computed by somebody else — so it tests the encoder
   in the direction that matters: sipx must produce these exact bytes from these inputs.
   `sipx-transport`'s tests already carry this vector for its decode half.
2. **RFC 5769 §2.2** — the sample IPv4 response, decoding to `192.0.2.1:32853`, with an 11-byte
   `SOFTWARE` attribute whose padding a naive decoder walks into.
3. **RFC 8839 §5.1's example line** —
   `a=candidate:2 1 UDP 1694498815 192.0.2.3 45664 typ srflx raddr 203.0.113.141 rport 8998` —
   round-trips through parse and serialise unchanged, and its priority matches §4's table.
4. **RFC 8839 §5.2's example lines** — `a=remote-candidates:1 192.0.2.3 45664` and the RTCP twin.
5. **RFC 8839 §5.4's example lines** — `a=ice-pwd:asd88fgpdd777uzjYhagZg`, `a=ice-ufrag:8hhY`.
6. **§4's priority table** — three candidates, three stated integers.
7. **§7.3's role-conflict table** — seven rows, each its own assertion, including the `T = V` row
   that decides whether two identical stacks converge.
8. **§6.4's initial-state example** — RFC 8445 §6.1.2.6's three-checklist, five-foundation table,
   asserted pair by pair.

## 15. Where the code goes

| Piece | Crate | Why there |
|---|---|---|
| Attribute grammar (§13) | `sipx-sdp` | Pure parsing. No clock, no socket. |
| STUN-for-ICE codec (§11) | see below | Pure bytes. Needs HMAC-SHA1 and CRC-32. |
| The agent (§2–§9) | `sipx-media`, sans-IO module | A state machine over events, like the transaction machines |
| The driver (sockets, timers, gathering) | `sipx-media` | Where the media socket already is |
| Reflexive gathering | `sipx-media`, over `sipx_transport::stun` | The Binding client already exists; a second one would be a second thing to get wrong |
| Relayed gathering (TURN) | its own story | RFC 8656 is a protocol, not an attribute |

**Settled by `M-20`.** `MESSAGE-INTEGRITY` needs HMAC-SHA1, and the question was which crate should
carry it. The codec lives in `sipx-media`, which names `hmac`, `sha1` and `subtle` directly — all
three were already in its transitive graph through `sipx-rtp`, so the lockfile gained four
dependency lines and no new packages. `sipx-rtp` was rejected: an ICE connectivity check is not a
media packet, every downstream user parsing RTP would have inherited a STUN codec in that crate's
public API, and it would have put the codec a crate below its only caller. `sipx-media` also takes a
`default-features = false` edge on `sipx-transport` to reuse the STUN header constants without
inheriting a TLS stack, a WebSocket stack and a DNS client; `scripts/check-features.sh` asserts that
on the resolved graph.
