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

Of 61 tracked RFCs, 22 are implemented, 7 partial, 10 parse-only and 21 not started. The
parse-only ten are the interesting number: `RAck`, `RSeq`, `Session-Expires`, `Min-SE`,
`Accept-Contact`, `Identity` and the rest all survive the wire intact today. Nothing is lost by
a message carrying them, and nothing acts on them. That is a deliberate position — losslessness
first — and it means each of these becomes a behaviour module rather than a change to the
parser.

## Order

### 1. Media security

| RFC | What it unlocks |
|---|---|
| 3711 SRTP | Encrypted media at all |
| 4568 SDES | Keying over an already-secure signalling path — the simpler half |
| 5764 DTLS-SRTP | Keying that does not trust the signalling path; also the WebRTC path |

sipx does `sips:` and WSS today and then sends the audio in the clear. For a stack whose TLS
policy has no "skip verification" option anywhere, that is the most conspicuous inconsistency in
it. SDES first because it is a smaller change and already useful; DTLS-SRTP after, because it is
what a browser will insist on.

### 2. Session integrity

| RFC | What it unlocks |
|---|---|
| 4028 Session timers | Detecting a call whose far end vanished |
| 3262 100rel / PRACK | Reliable provisionals, which some carriers require |
| 3311 UPDATE | Renegotiating before the call is answered |

Today a call whose far end disappears without a BYE stays up in sipx forever. That is a leak of
exactly the kind the soak test exists to find, except it is a protocol-level one and no amount
of local tidying fixes it. All three headers already parse, so these are behaviour-only.

### 3. Reachability

| RFC | What it unlocks |
|---|---|
| 3327 Path | Routing back toward a UA through the proxies it registered through |
| 5626 Outbound | Flow tokens, `reg-id`, redundant registrations, NAT survival |
| 5627 GRUU | A URI that reaches one specific instance |
| 8599 Push | Waking a mobile client that is not holding a connection |

This is the group that turns a UA into something a real deployment can register. Strictly
ordered: `Path` is not even known to the parser yet, Outbound builds on it, GRUU on both, push on
Outbound. It is also the group that most changes what sipx *is*, since a registrar is a
different kind of program from a phone.

### 4. Event framework

| RFC | What it unlocks |
|---|---|
| 6665 SUBSCRIBE/NOTIFY | The framework, properly, rather than REFER's implicit subscription |
| 4488 `Refer-Sub` | Suppressing that subscription when it is not wanted |
| 3680, 4235, 3856 | Registration, dialog and presence event packages |
| 3903 PUBLISH | Publishing state into it |

sipx already implements the *implicit* subscription a REFER creates, including terminating it.
Generalising that into a subscription store with packages is a considerable piece of work, and
everything in the presence and busy-lamp family waits behind it.

### 5. Identity and interconnect

| RFC | What it unlocks |
|---|---|
| 8760 Digest | SHA-512-256, and offering several algorithms at once |
| 8224 / 8225 STIR/PASSporT | Signed caller identity |
| 7044 History-Info | Saying who forwarded a call and why |
| 7339 / 7415 Overload control | Something better than answering 503 |

8760 is small and self-contained. STIR is not, and it is the one that matters for anything
touching the public telephone network.

### 6. Recording and the rest

| RFC | What it unlocks |
|---|---|
| 7865 / 7866 SIPREC | Recording as a protocol rather than a local file |
| 5118 | The IPv6 torture corpus, as a check rather than a feature |
| 8445 ICE | The hard NAT cases symmetric RTP does not solve |
| 3428 MESSAGE, 6086 INFO | Methods that currently parse and do nothing |

## What is deliberately not on this list

**Proxy and registrar roles.** Several tracked RFCs define proxy behaviour that sipx does not
implement — forking, Record-Route insertion, loop detection. That is not an oversight; sipx is a
user agent, and becoming a proxy is a decision about what the project is rather than a gap to
close. The compliance table's `Roles` column says so per RFC rather than leaving a reader to
infer it.

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
