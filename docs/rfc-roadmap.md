# RFC roadmap

Which gaps in [the compliance table](compliance.md) close next, in what order, and why that
order. The table says where sipx *is*; this says where it is going.

Two things shape it.

**Dependencies are real and mostly one-way.** GRUU needs Path and Outbound. Presence needs a
real event framework. SRTP keying needs SRTP. Doing them out of order means building something
twice.

**A gap that changes what sipx can be deployed as beats a gap that adds a feature.** SRTP is
first below not because it is interesting but because "signalling can be encrypted and media
cannot" is the sentence that disqualifies the stack from most of the places it would otherwise
fit.

## Where the gaps actually are

68 tracked RFCs; [the table](compliance.md) has the per-status counts, and it is generated, so it
is the one place worth reading them from.

The interesting status is **parse-only**: `Accept-Contact`, `Identity`, `Reason` and the rest all
survive the wire intact today, and nothing acts on them. That is a deliberate position —
losslessness first — and it means each of these becomes a behaviour module rather than a change to
the parser. `RAck`/`RSeq` and `Session-Expires`/`Min-SE` were in that group until `S-12` and
`S-11`; both turned out to be exactly that, a module beside the parser rather than a change to
it.

## Done since this list was written

The first two groups are closed, and the third is half closed. Kept here rather than deleted,
because the *reasons* were the argument for the order and they are worth being able to check
against what happened.

| RFC | Story | What it turned out to be |
|---|---|---|
| 3711 SRTP + 4568 SDES | `M-14` | Encrypted media, keyed over the signalling path. The remaining half is DTLS-SRTP, now in **M6**. |
| 4028 Session timers | `S-11` | A call whose far end vanished is now torn down locally rather than kept forever. |
| 3262 100rel / PRACK | `S-12` | Behaviour-only, as predicted: a module beside the parser. The retransmission schedule was the part with a surprise in it — no T2 cap. |
| 3327 Path | `T-14` | Inbound routing back through the registrar's proxies. Its own surprise: RFC 3327 §5.1 has the *UA ignore* what comes back, which is why `T-16` exists. |
| 8760 Digest | `S-14` | SHA-512-256 and several algorithms offered at once. |

## Order

### 1. Reachability — **M6**

| RFC | What it unlocks | Status |
|---|---|---|
| 3327 Path | Routing back toward a UA through the proxies it registered through | done (`T-14`) |
| 3608 Service-Route | The same route set in the direction requests actually leave | `T-16` |
| 5626 Outbound | Flow tokens, `reg-id`, redundant registrations, NAT survival | `T-15` |
| 5764 DTLS-SRTP | Keying that does not trust the signalling path; also the WebRTC path | `M-15` |
| 5627 GRUU | A URI that reaches one specific instance | **M10** (`T-20`) |
| 8599 Push | Waking a mobile client that is not holding a connection | **M10** (`T-21`) |

This is the group that turns a UA into something a real deployment can register. Strictly
ordered: Outbound builds on `Path`, GRUU on both, push on Outbound. It is also the group that
most changes what sipx *is*, since being reachable through infrastructure is a different problem
from placing a call.

DTLS-SRTP rides along here rather than in a media group of its own: it is the last piece of "can
a real deployment, including a browser, talk to this", which is what M6 is about.

### 2. Forwarding — **M7**

Not RFC gaps but API ones, and they gate everything a proxy or B2BUA would need: a request that
can be dropped without a counter (`T-19`), a response that matched nothing and is discarded
rather than forwarded (`T-18`), a resolver shaped for one UA (`T-17`), a header collection with
no editing operations (`S-15`), and digest that can only be answered and never issued (`S-16`).
RFC 7616's server half is the only RFC-shaped item in the group.

### 3. Event framework — **M8**

| RFC | What it unlocks |
|---|---|
| 6665 SUBSCRIBE/NOTIFY + 4488 `Refer-Sub` | The framework, rather than REFER's implicit subscription — and suppressing that one when it is not wanted (`S-13`) |
| 4235, 3680 | Dialog and registration event packages: busy-lamp fields, and watching a registration go stale (`S-17`) |
| 3856, 3863, 3903 | Presence with PIDF, and PUBLISH to put state into the framework (`S-18`) |

sipx already implements the *implicit* subscription a REFER creates, including terminating it.
Generalising that into a subscription store with packages is a considerable piece of work, and
everything in the presence and busy-lamp family waits behind it.

The packages are ordered by what they report. Dialog and registration state is state sipx
*already keeps*, so those two exercise the framework without also needing a state model of their
own; presence needs somewhere for presence to come from, which is a separate question and why
PUBLISH travels with it.

### 4. Session integrity, the remaining piece — **M9**

| RFC | What it unlocks |
|---|---|
| 3311 UPDATE | Renegotiating before the call is answered (`S-19`) |

The other two in this group are done (`S-11`, `S-12`). UPDATE is what makes an early
renegotiation possible at all, and 100rel is its prerequisite — which is now in place, so this is
the smallest unstarted item on the list.

It is one RFC and does not fill a milestone, so **M9** wraps it with the two non-RFC gaps of the
same shape: early media, which `S-12` built the offer/answer for and never used (`C-2`), and two
dialogs driven as one call (`C-1`). All three are about a session before it is confirmed, which is
where a forwarding element's hard cases live.

### 5. Identity and interconnect — **M11**

| RFC | What it unlocks |
|---|---|
| 8224 / 8225 STIR/PASSporT | Signed caller identity (`S-20`) |
| 7044 History-Info + 3326 Reason | Saying who forwarded a call and why (`S-21`) |
| 7339 / 7415 Overload control | Something better than answering 503 (`T-22`) |

8760 was the small self-contained one and is done (`S-14`). STIR is not small, and it is the one
that matters for anything touching the public telephone network.

RFC 3326 joins History-Info rather than sitting in the parse-only list on its own: RFC 7044 §10.2
requires the `Reason` inside the `hi-targeted-to-uri`, so the two are one piece of work.

### 6. Recording and the rest

| RFC | What it unlocks |
|---|---|
| 7865 / 7866 SIPREC | Recording as a protocol rather than a local file |
| 3428 MESSAGE, 6086 INFO | Methods that currently parse and do nothing |

Two things have moved out of this group. **5118**, the IPv6 torture corpus, is in **M12** (`X-16`)
with the rest of the measurement work — it is a check rather than a feature, so it belongs beside
the interop matrix and the fuzzers rather than beside a recording protocol. **8445 ICE** is in
**M10** (`M-16`), because listing it here was a mis-grouping: reaching the far end at all is not a
feature, it is the same class of gap as GRUU and push, which is the group ICE now sits in.

## What is deliberately not on this list

**Proxy and registrar roles.** Several tracked RFCs define proxy behaviour that sipx does not
implement — forking, Record-Route insertion, loop detection. That is not an oversight; sipx is a
user agent, and becoming a proxy is a decision about what the project is rather than a gap to
close. The compliance table's `Roles` column says so per RFC rather than leaving a reader to
infer it.

M9's `C-1` is not an exception to that. A B2BUA is *two user agents*, which is precisely why the
primitive belongs in a user-agent stack while forking and Record-Route insertion do not — and why
M9 adds the coupling and nothing that would make sipx a proxy.

**IMS.** A different trust model and a different header set. Tracking it would mean tracking
documents nothing in sipx will read.

**Anything obsoleted.** RFC 4566 is listed as superseded and RFC 4474 as syntax-only precisely so
a reader looking for them finds out where they went, rather than finding nothing and assuming an
oversight.

## Keeping this honest

[`docs/rfc/registry.toml`](rfc/registry.toml) is the source; the compliance table is generated
from it and CI fails if the two disagree, if an entry names a header the parser does not know, or
if it cites a file that does not exist. That check cannot verify behaviour — the tests do — but it
does stop the table drifting away from the code, which is how a compliance document usually stops
being true.
