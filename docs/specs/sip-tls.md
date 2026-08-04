# Spec: TLS, WebSocket, and the certificate policy

**Status:** normative · **Crate:** `sipx-transport` · **Stories:** T-6 … T-9 · **Design:**
[sip-transport](../designs/sip-transport.md)

## 1. Normative references

- RFC 3261 §26 (security), §26.3.2.2 (`sips`), §19.1.1 (the `sips` scheme).
- RFC 5922 — domain certificates in SIP. This is the one that says *how* to check a name.
- RFC 5923 — connection reuse, which TLS complicates.
- RFC 7118 — SIP over WebSocket.
- RFC 6125 — service identity in TLS generally.
- RFC 8446 (TLS 1.3), RFC 5246 (TLS 1.2).

## 2. What `sips:` actually promises

**[sipx] `sips:` is hop-by-hop, and sipx says so.**

RFC 3261 §26.2.2 requires TLS on each hop up to the last proxy; the final hop to the callee is
whatever that proxy chooses. So a `sips:` URI guarantees that *sipx's own* connection is
encrypted and authenticated. It does not guarantee the call is encrypted end to end, and it
cannot: a proxy in the middle terminates TLS by design.

This matters because it is routinely misread. A user who believes `sips:` means end-to-end
encryption will make decisions on that basis. Any sipx documentation, log line or CLI output
that mentions `sips:` therefore describes it as *transport* security, never as end-to-end.

**[sipx]** sipx never downgrades a `sips:` request. If no TLS candidate can be reached the
request fails, with an error naming that reason. `T-4` already implements the resolution half
of this — a `sips:` URI yields no cleartext candidate — and this spec extends it to the
handshake: a certificate that fails verification is a failed candidate, not a reason to try TCP.

## 3. Certificate verification

### 3.1 Mandatory

These are not configurable, because a stack in which they can be turned off is a stack whose
security depends on every deployment getting a flag right.

- **[sipx] The chain must validate** to a trust anchor, with expiry and signature checked.
- **[sipx] The peer identity must match** what we set out to reach, per §3.3.
- **[sipx] A failure closes the connection.** No retry in cleartext, no "continue anyway".
- **[sipx] The error names which check failed** — expired, wrong host, unknown issuer are three
  different operational problems with three different fixes, and collapsing them into
  "handshake failed" costs an engineer an afternoon.

### 3.2 Configurable

Each entry names the API that provides it, and `crates/sipx-transport/tests/tls_spec.rs` holds it
to that: an entry naming nothing, or naming something that has since been renamed, fails the build.
A list of knobs is the one part of a spec that can be checked against the code mechanically, and
until `X-46` this one was not — it listed a minimum protocol version no type has ever taken (the
paragraph after the list), and nothing about the entry looked different from the two real ones.

- **The trust anchors** — `TrustAnchors`, given to `ClientTls::new`. `TrustAnchors::system` is the
  platform's own store; `TrustAnchors::only` with `TrustAnchors::add_pem` is a set named
  certificate by certificate. There is no implicit default, because the anchors are an argument
  rather than a field: trusting nothing is a configuration error refused at construction, not a
  silent refusal of every peer at handshake time.
- **A client certificate to present** (§3.4) — `ClientTls::with_identity`, from `Identity::from_pem`.

**[sipx] The protocol version is not one of them, and the floor is not sipx's to set.** Neither
`ClientTls` nor `ServerTls` takes a version and nothing above them names one: what is offered is
the TLS library's default version set, `{1.3, 1.2}`. So 1.0 and 1.1 are excluded because there is
nothing older to select, not because sipx refuses them — the floor is a **dependency** property,
which §3.5 states, `tls.rs`'s module documentation states, `tests/tls_versions.rs` pins with
`the_library_offers_nothing_below_the_floor`, and RFC 8996's registry row says in the same words.
Restated in each of them deliberately (`X-43`), so that a change of TLS backend cannot move the
floor in one place only.

The knob was not built instead, for two reasons. Its only representable value above the default is
"1.3 only" — the library offers no third version, and RFC 8996 makes anything below 1.2
unrepresentable — so it would be a knob whose one setting is a deployment policy nobody has asked
for. And an API that selects versions is the thing whose *absence* is currently the evidence for
RFC 8996 and RFC 8446: adding one would replace a claim that cannot be got wrong with a claim that
has to be tested at every call site.

**[sipx] There is no "skip verification" option.** Test code that needs to trust a fixture CA
adds that CA as a trust anchor, which is a different operation with a different shape: it says
*what* to trust rather than *that anything goes*. This is deliberate. Every stack that ships an
`insecure` flag eventually finds it in production, because it is exactly the thing a frustrated
engineer reaches for at midnight, and nothing about it is loud the next morning.

### 3.3 Which name must match (RFC 5922)

The identity being checked is **the host in the URI sipx set out to reach** — the SIP domain
before resolution, not the address a SRV record led to. Checking the resolved name instead
means anyone who can influence DNS chooses which certificate is acceptable, and the whole
verification becomes decorative.

In order:

1. **`subjectAltName` of type `URI`** matching the SIP URI, if present.
2. **`subjectAltName` of type `dNSName`** matching the host.
3. **The `CN`**, *only* when the certificate carries no `subjectAltName` at all. A certificate
   with a SAN that does not match is a failure, not a reason to consult the CN — RFC 6125 §6.4.4.

**[sipx]** A certificate may carry several names and any one matching is enough; that is normal
for a proxy serving many domains. Wildcards match a single leftmost label only, so
`*.example.com` matches `sip.example.com` and not `a.b.example.com` or bare `example.com`.

### 3.4 Mutual TLS

**[sipx]** sipx presents a client certificate only when configured with one. When a server
requests one and none is configured, the handshake proceeds without it and the *server* decides
— sipx does not pre-emptively fail, because plenty of servers ask optionally.

**[sipx]** As a server, sipx requests a client certificate only when configured to, and when it
does, an unverifiable one is refused. "Request but do not check" is worse than not asking: it
produces logs that look like authentication and are not.

### 3.5 Versions and ciphers

**[sipx] TLS 1.2 is the floor; 1.3 is preferred.** 1.0 and 1.1 are deprecated (RFC 8996) and
excluded. This will lock out some old SBCs, which is the point of a floor — the alternative is
that every sipx deployment inherits their weaknesses. It is not configurable in either direction,
and §3.2 says what governs it instead.

**[sipx]** sipx offers 1.3 and 1.2 and lets the server choose. A server that rejects the offer
outright rather than selecting 1.2 is misconfigured, not old — Kamailio's default `tls_method`
does exactly this, and `openssl s_client` fails against it too. sipx does not stop offering 1.3
to accommodate it.

**[sipx]** Cipher selection is left to the TLS library's defaults rather than pinned here. A
hand-written cipher list is a snapshot of one afternoon's opinion, and it ages badly: the lists
people pin are the reason deployments are still negotiating things nobody meant to allow.

## 4. WebSocket (RFC 7118)

**[RFC 7118 §4]** The handshake negotiates the `sip` subprotocol. **[sipx]** A peer that does not
offer it is refused: without the subprotocol there is no agreement about what the frames mean.

**[RFC 7118 §4] The resource name is not fixed, so a target names it.** RFC 7118 registers a
subprotocol and a URI scheme; it says nothing about where on a server SIP lives, and both `/` and
`/ws` are therefore conformant. **[sipx]** A `Target` carries the resource its handshake asks for,
defaulting to `/`. A client that can only ask for `/` reaches servers that serve SIP at their root
and no others — which is not a stricter reading of the RFC but a narrower one, and the difference
is invisible until it meets the second kind. The resource is part of the connection's identity for
the same reason the verified name is (§5): a socket upgraded at `/ws` was accepted by whatever
serves `/ws`, and lending it to traffic that asked for somewhere else discards the only thing the
target said about where it was going.

The port is not fixed either, and needs nothing new: a server is entitled to serve SIP over
WebSocket from an HTTP server on its own port, and `Target` already takes the address to send to.

**[RFC 7118 §5] One SIP message per WebSocket message.** Not `Content-Length` framing — the
frame boundary *is* the message boundary. A message split across frames is malformed, and two
messages in one frame likewise. **[sipx]** Both close the connection rather than being patched
up: a peer that frames wrongly has revealed it disagrees about where messages end, so nothing
further from it can be trusted to be what it claims.

**[sipx] `Content-Length` is optional here**, unlike on TCP and TLS. RFC 3261 §20.14 makes it
mandatory on a stream because nothing else says where a message ends; a WebSocket message says,
so a body simply runs to the end of it. Requiring it anyway would reject messages this transport
can frame perfectly well.

**[sipx] The configured SIP message limit is also the WebSocket decoder's frame and
assembled-message limit.** RFC 6455 §5.2 puts an attacker-controlled length before the payload.
Checking only after the WebSocket implementation assembled its default-sized message would preserve
parser correctness while leaving a much larger allocation path in front of it. WS and WSS pass
`Limits::max_message_bytes` into the decoder before the handshake completes; TLS changes the bytes
underneath, not this bound.

**[sipx] Text frames where the bytes allow it, binary otherwise.** RFC 7118 §5 permits either.
Text is what a browser's network panel and every capture tool show as readable SIP; a body that
is not valid UTF-8 cannot go in a text frame at all (RFC 6455 §5.6).

**[RFC 7118 §5.2]** A WebSocket client has no listening port and can never be connected back to.
Its `Via` sent-by is therefore an arbitrary unique hostname it invents, which is not resolvable
and must not be resolved. **[sipx]** sipx invents one per endpoint under `.invalid`, which
RFC 2606 reserves precisely so that nothing will ever resolve it. An endpoint that *does* listen
for WebSocket connections is not that client and keeps its own name.

**[sipx] The same applies to `Contact`, and it has to.** A dialog's remote target is taken from
`Contact` (RFC 3261 §12.2.1.1), so a WebSocket client that advertised a real address there would
have every in-dialog ACK and BYE aimed at a port that is not listening. sipx therefore writes
the invented name, marked with the transport, and — on the receiving side — **ignores `Contact`
entirely for in-dialog requests over WebSocket**: everything goes back over the connection the
dialog was established on, unconditionally. This is the RFC 5923 rule from `T-3` made absolute:
for WebSocket there is no fallback, because there is nowhere to fall back to.

**[sipx]** Ping (RFC 6455 §5.5.2) keeps the connection alive, every 25 seconds by default.
Intermediaries close idle sockets well inside a registration's lifetime, and a registration
whose connection has silently died is a phone that rings nowhere. A Ping is also the only
keep-alive that tells us something: the peer must answer it.

**[sipx] WSS is §3 with §4 on top, and is not permitted to be anything else.** The certificate
policy is not restated here because it is not reimplemented — the same `ClientTls`/`ServerTls`
perform the same handshake before the upgrade begins. A second implementation of a security
check is how one of the two ends up weaker.

## 5. Connection reuse under TLS

**[sipx]** Which fields the pool keys connections by is defined once, in
[`sip-transport.md` §8](sip-transport.md), and generated there from the type — the list was
restated in three specs and had gone stale in one of them twice. This section says *why* two of
those fields are in the key, which is the half no generator can write.

The *transport* is in the key because TCP, TLS and WebSocket to one address are not
interchangeable — they can share a port, and a `sips:` request riding a cleartext socket has
silently become the thing it asked not to be.

The *verified identity* is in the key because two names that resolve to one address are two
connections. Reusing one for the other would mean traffic for `a.example.com` travelling over a
connection authenticated as `b.example.com`, which defeats the check that was just performed. A
connection a peer opened has no identity — sipx verified nothing about it — so it can never
stand in for a name it never checked.

The *WebSocket resource* is in the key for the same reason one step down (§4): a socket upgraded
at `/ws` was accepted by whatever serves `/ws`, and lending it to traffic that asked for
somewhere else discards the only thing the target said about where it was going. `None`
everywhere the question does not arise — every other transport, and every connection a peer
opened.

**[sipx] The identity is the URI host, and it survives resolution.** RFC 3263 turns one name
into a list of addresses by way of NAPTR and SRV records that may name something else entirely;
the certificate is still checked against what the URI said. Attaching the resolved name instead
would leave the handshake succeeding, the check apparently running, and whoever can influence
DNS deciding which certificate is acceptable.

## 6. Test vectors

| # | Scenario | Expected |
|---|---|---|
| L1 | Certificate signed by an untrusted CA | Refused, error names the issuer |
| L2 | Certificate for the wrong host | Refused, error names the host mismatch |
| L3 | Expired certificate | Refused, error names expiry |
| L4 | SAN present but not matching, CN matching | **Refused** — the CN is not consulted |
| L5 | No SAN, CN matching | Accepted |
| L6 | Wildcard `*.example.com` vs `sip.example.com` | Accepted |
| L7 | Wildcard `*.example.com` vs `a.b.example.com` | Refused |
| L8 | `sips:` where only TCP is reachable | No candidate; the request fails, never downgrades |
| L9 | TLS 1.1 offered | Refused |
| L10 | Two hosts on one address | Two connections, not one reused |
| W1 | WebSocket peer offering no `sip` subprotocol | Refused |
| W2 | One message per frame | Delivered |
| W3 | Message split across two frames | Rejected as malformed |
| W4 | Response to a WebSocket request | Returns on the same connection |
| W5 | Two messages in one frame | Rejected as malformed |
| W6 | Message with no `Content-Length` | Accepted; body runs to the end of the frame |
| W7 | Server upgrading without echoing the subprotocol | Refused |
| W8 | Outbound WebSocket request | `Via` sent-by and `Contact` are `.invalid` |
| W9 | In-dialog BYE over a WebSocket | Goes over the connection, not to the `Contact` |
| W10 | Idle connection | Pinged before an intermediary would time it out |
| W11 | WS and TCP to one address | Two connections, not one reused |
| W12 | WSS with a certificate for another host | Refused before the upgrade; nothing crosses |
| W13 | Server serving SIP only at `/ws` | Reached when the target names it; `404` when it does not |
| W14 | Frame or fragmented message above the configured SIP message limit | Refused by the WebSocket decoder before assembly |
| I1 | Register over TLS against a third-party server | Accepted |
| I2 | …presenting a certificate for another name | Refused, immediately |
| I3 | …signed by an issuer we do not know | Refused, immediately |
| I4 | Register over WebSocket against a third-party server | Accepted |
