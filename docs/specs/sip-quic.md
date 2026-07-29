# Spec: SIP over QUIC

**Status:** normative, but describing an unratified mapping · **Crate:** `sipx-transport` ·
**Stories:** T-11 … T-12 · **Design:** [sip-transport](../designs/sip-transport.md)

> **Read this first.** There is no RFC for SIP over QUIC. RFC 9000 and RFC 9001 define QUIC;
> nothing defines how SIP sits on it. Every mapping decision below — the `Via` token, the port,
> the framing, the NAPTR service, the ALPN handling — is **sipx's choice**, marked `[sipx]`, and
> is not interoperable with any other implementation except by coincidence. Where a choice
> could later be contradicted by a standard, the section says what would have to change.
>
> This is why the transport ships behind a feature and is not offered as a `Target` by default:
> a transport that only talks to itself should not look like one that talks to the network.

## 1. Normative references

- RFC 9000 — QUIC as a transport. Streams, connection identifiers, migration, close.
- RFC 9001 — using TLS 1.3 to secure QUIC. There is no unencrypted QUIC.
- RFC 3261 §18 — the SIP transport layer's obligations, which do not change.
- RFC 3261 §7.1 — `Via` transport is a token, so a new one is grammatically legal.
- RFC 3263 — locating SIP servers; the NAPTR/SRV path a new transport has to join.
- RFC 5923 — connection reuse, and why a response goes back where the request came from.
- **[sip-tls.md §3](sip-tls.md) by reference** — the certificate policy is *identical* and is
  deliberately not restated. See §3 below for why that matters more here than elsewhere.

## 2. What QUIC changes, and what it does not

QUIC is a reliable, multiplexed, always-encrypted transport over UDP. For SIP that means:

| | TCP | TLS | WebSocket | QUIC |
|---|---|---|---|---|
| Reliable | yes | yes | yes | yes |
| Encrypted | no | yes | if WSS | **always** |
| Message boundaries | none | none | frame | **stream** |
| Head-of-line blocking across messages | yes | yes | yes | **no** |
| Survives an address change | no | no | no | **yes** (migration) |

**[sipx] QUIC is a *reliable* transport for RFC 3261's purposes.** Timer A and Timer E do not
apply; the transaction layer uses its reliable-transport branch, exactly as for TCP and TLS.
That it runs over UDP is invisible above the transport layer, and treating it as unreliable
would produce duplicate INVITEs on a transport that already guarantees delivery.

**[sipx] The 1300-byte datagram limit of RFC 3261 §18.1.1 does not apply.** That limit exists
because an oversized UDP datagram fragments; QUIC handles its own packetisation, so a message is
bounded only by [`Limits`](sip-parser.md), like TCP.

## 3. Security: nothing new, on purpose

**[RFC 9001 §3]** QUIC's handshake *is* TLS 1.3. There is no other option, no downgrade, and no
cipher negotiation outside TLS 1.3's own.

**[sipx] The certificate policy is [`sip-tls.md` §3](sip-tls.md), unchanged and unreimplemented.**
Same SAN rules (RFC 5922), same refusal of the CN when a SAN is present, same wildcard handling,
same inability to turn verification off. `sip-tls.md` §3.5's TLS 1.2 floor is moot rather than
relaxed: RFC 9001 forbids anything below 1.3.

This is a *requirement*, not an observation. A second implementation of a security check is how
one of the two ends up weaker, and a transport whose verification is "the same idea, written
again" has already lost the property `sips:` was asked for.

**[sipx] The ALPN token is `sip/2`, and a peer that negotiates anything else is refused.**

`sip/2` is what IANA has registered for SIP in the ALPN registry, under RFC 3261. It is *not*
`sip` — that is the WebSocket subprotocol from RFC 7118 §4, a different registry with a
different value, and the two are easy to confuse.

Refusal is absolute, including refusal of a peer that negotiates *no* ALPN at all. ALPN is the
only thing on a QUIC connection that says what the streams contain; without it, bytes that
happen to parse as SIP would be treated as SIP, which is how a service on a shared port gets
confused into speaking a protocol it was never offering.

**[sipx] 0-RTT is refused for requests, in both directions.**

RFC 9001 §9.2 is explicit that early data is replayable by an attacker who captures it, and
QUIC provides no way to bind a SIP transaction to a particular handshake. A replayed INVITE
matches an existing server transaction by branch and is absorbed — which is the *good* case; a
replayed request whose transaction has already terminated creates a second call. The saving is
one round trip on a transport whose whole point is that the handshake is already cheap.

sipx therefore configures `max_early_data_size = 0` on the server and does not attempt 0-RTT on
the client. This would have to change only if a future standard defines a replay-safe binding.

## 4. Framing: one message per stream

**[sipx] One SIP message per QUIC bidirectional stream. The stream's end is the message's end.**

This is the RFC 7118 §5 reasoning that [`sip-tls.md` §4](sip-tls.md) already applied to
WebSocket, and QUIC gives the same thing for the same reason: a delimiter the transport provides
is better than one the payload declares. It also buys what WebSocket cannot — because streams
are independent, a large message in flight does not block a small one behind it, which is the
head-of-line blocking that makes a REGISTER wait behind someone else's SDP on TCP.

Consequences, each of which is a rule:

- **`Content-Length` is optional.** RFC 3261 §20.14 makes it mandatory on a stream because
  nothing else says where a message ends. Here the stream says. A message that carries one is
  still checked against the bytes actually received, and a mismatch closes the connection: a
  peer that disagrees with itself about its own length has revealed a framing bug, and the next
  message from it cannot be trusted to start where we think it does.
- **Two messages on one stream is malformed**, and closes the connection rather than being
  split. Same reasoning as `sip-tls.md` §4: the peer disagrees about where messages end.
- **A stream that ends mid-message is malformed**, not a truncation to be salvaged. RFC 4475
  §3.1.1 exists because half a SIP message can be a *different* SIP message.
- **Unidirectional streams are refused.** A request needs somewhere for its response to go.

**[sipx] A response goes back on the stream its request arrived on.**

Not merely on the same connection — on the same *stream*, which is stronger than RFC 5923 needs
and is free here. It removes transaction-to-stream bookkeeping entirely: the stream *is* the
transaction's transport identity for as long as it is open.

Provisional responses are the wrinkle: several may precede the final one, so the stream is not
closed until the transaction is done with it. **[sipx]** sipx keeps the stream open until the
final response has been written, then closes its send side.

**[sipx] `Contact` is ignored for in-dialog requests over QUIC**, as for WebSocket, and for the
same reason — a client may have no connectable address, and connection migration means the
address it had is not necessarily the address it has. Everything goes back over the connection
the dialog was established on. This is `sip-tls.md` §4's absolute rule, extended.

## 5. Naming and resolution

**[sipx] The `Via` transport token is `QUIC`: `SIP/2.0/QUIC`.**

RFC 3261 §7.1 makes `transport` a token, so this is grammatically legal without registration. A
peer that does not know it will fail to route the response — which is correct and is why §7
says a QUIC target is never chosen implicitly.

**[sipx] The default port is 5061**, matching TLS, because both are authenticated transports and
a deployment that has already opened 5061 for `sips:` has made the same trust decision. Note it
is 5061/**udp**, which is not the same listener as 5061/tcp and does not conflict with it.

**[sipx] A non-QUIC datagram arriving on the QUIC port is dropped without a response.** QUIC's
long-header form is identifiable from the first byte (RFC 9000 §17.2), and a peer that sends
plain SIP to 5061/udp has guessed wrong about what is there. Answering it — even with an error —
would turn the port into a reflector.

**[sipx] The RFC 3263 NAPTR service is `SIPS+D2Q`.**

Nothing is registered for QUIC. The existing tags are `SIP+D2U`, `SIP+D2T`, `SIP+D2S`,
`SIP+D2W`, `SIPS+D2T`, `SIPS+D2W`, `SIPS+D2S`; `D2Q` follows the pattern. **Only the `SIPS`
form exists**, because RFC 9001 leaves no unencrypted variant — a `SIP+D2Q` tag would advertise
an authenticated transport under the scheme that promises nothing, and the distinction the two
tags exist to draw would be lost.

**[sipx] `SIPS+D2Q` is never *preferred* over `SIPS+D2T` by sipx's own ordering.** A server that
publishes both is asked for whichever its NAPTR order prefers, and where the order is equal sipx
takes TLS. An unratified mapping should not silently win a choice the operator did not make.

## 6. Connections, keepalive, and failure

**[sipx] The pool key is the one [`sip-transport.md` §8](sip-transport.md) defines**, with the
transport naming QUIC and no `path`: a QUIC connection is never a WebSocket, so the resource
never applies. The `identity` is in the key for the reason it is there for TLS
([`sip-tls.md` §5](sip-tls.md)): two hostnames on one address are two connections, because a
connection verified for one name is not usable for the other.

**[sipx] Connection migration is the QUIC stack's business, not the transport layer's.** A
connection survives an address change by design (RFC 9000 §9), so the pool key must **not**
include the peer's current address as observed from the socket. Keying on it would mean a phone
moving from Wi-Fi to cellular loses a connection that has not actually broken.

**[sipx] QUIC PING frames are the keepalive; there is no SIP-level one.**

The WebSocket transport pings every 25 seconds (`sip-tls.md` §4) because intermediaries drop
idle sockets. The same is true of UDP NAT bindings, which are usually *more* aggressive — but
QUIC already has a keepalive that operates below SIP and does not consume a transaction, so
adding a SIP-level OPTIONS ping on top would be a second mechanism for one job. sipx configures
the QUIC layer's keepalive interval and leaves it there.

**[sipx] A connection close fails every transaction on it, immediately and namedly.**

RFC 3261 §18 has no notion of a transport that goes away, so this is our rule: outstanding
client transactions get a transport error rather than being left to time out at 32 seconds. A
connection that has closed will not deliver a response, and pretending otherwise makes a phone
appear to ring for half a minute after the server went away. The error names the close reason
QUIC reported, because "the far end restarted" and "the certificate was rejected" are different
problems for whoever is reading the log.

## 7. What is deliberately not done

- **QUIC is never chosen implicitly.** A `sip:` or `sips:` URI with no transport parameter is
  resolved over UDP, TCP or TLS as RFC 3263 directs. QUIC is used when a `Target` names it, or
  when NAPTR explicitly returns `SIPS+D2Q`. An unratified mapping must be opted into.
- **No datagram mode.** RFC 9221 unreliable datagrams over QUIC would be a second, differently
  framed SIP transport inside the first. One mapping at a time.
- **No 0-RTT**, per §3.
- **No multipath.** Not stable, and nothing in SIP asks for it.

## 8. Test vectors

Derived by `T-12`. `Q*` numbering continues the `L*`/`W*` tables in `sip-tls.md` §6.

| # | Scenario | Expected |
|---|---|---|
| Q1 | Certificate signed by an untrusted CA | Refused, error names the issuer |
| Q2 | Certificate for the wrong host | Refused, error names the host mismatch |
| Q3 | Peer negotiates ALPN `h3` | Refused before any SIP is read |
| Q4 | Peer negotiates no ALPN at all | Refused |
| Q5 | Peer negotiates `sip` (the WebSocket subprotocol) | Refused — the token is `sip/2` |
| Q6 | One message per stream | Delivered |
| Q7 | Two messages on one stream | Rejected as malformed; connection closed |
| Q8 | Stream ends mid-message | Rejected as malformed; connection closed |
| Q9 | Message with no `Content-Length` | Accepted; body runs to the end of the stream |
| Q10 | `Content-Length` disagreeing with the bytes received | Rejected; connection closed |
| Q11 | Response to a QUIC request | Returns on the same stream |
| Q12 | 100 Trying then 200 OK | Both on one stream; stream closes after the 200 |
| Q13 | Client offers 0-RTT early data | Server accepts no early data; request goes at 1-RTT |
| Q14 | Connection closes with a transaction outstanding | Transaction fails at once, naming the close reason |
| Q15 | Two hostnames on one address | Two connections, not one reused |
| Q16 | QUIC and TLS to one address | Two connections, not one reused |
| Q17 | Peer migrates to a new address mid-dialog | Same connection; the pool entry is unchanged |
| Q18 | In-dialog BYE over QUIC | Goes over the connection, not to the `Contact` |
| Q19 | Plain SIP datagram sent to the QUIC port | Dropped; nothing is sent back |
| Q20 | Idle connection | Kept alive by QUIC PING, no SIP-level ping sent |
| Q21 | NAPTR returning both `SIPS+D2T` and `SIPS+D2Q` at equal order | TLS chosen |
| Q22 | `sips:` URI with no transport parameter | QUIC not attempted |
| Q23 | Message far above the 1300-byte datagram limit | Delivered; the limit does not apply |
| Q24 | Transaction timers on QUIC | Reliable-transport branch; no Timer A or E |
