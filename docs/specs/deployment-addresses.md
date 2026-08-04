# Spec: Deployment addresses and non-ICE media latching

**Status:** normative for `M-42` · **Crates:** `sipx-transport`, `sipx-call`, `sipx-media`,
`sipx-cli` · **Story:** [M-42](../stories/M-42-advertise-a-chosen-address-and-latch-rtp-without-ice.md)

## 1. Normative references

- **RFC 3261 §8.1.1.3, §18.1.1 and §20.42** — Contact reachability, the Via `sent-by` value and
  response routing.
- **RFC 3261 §8.1.2 and §16.6** — a preloaded `Route` set determines the next hop independently of
  the Request-URI.
- **RFC 3581 §3–§4** — `rport` requests symmetric response routing; the server records `received`
  and the source port.
- **RFC 4566 §5.7** — the SDP `c=` line names the connection address offered to the peer.
- **RFC 4961 §3–§4** — symmetric RTP sends to the source address and port from which valid RTP was
  received.
- **RFC 8445 §8.1.1** — a nominated ICE pair selects the media path when ICE is in use.

## 2. Address roles

An application supplies two independent address roles. A **bind address** selects a local
interface on which a socket is opened. An **advertised address** is serialized for the peer and
need not be locally bindable. Treating the advertised address as the bind address makes a public
NAT mapping unusable: the host is expected to advertise that mapping precisely because it does not
own the address locally.

For signalling, `sipx_transport::Config::bind` remains the bind socket. `Config::sent_by` and
`sent_by_port` are the advertised Via values. The application-owned Contact value is built from
the same advertised host when it wants one deployment address across the message.

For media, the call configuration carries a media bind IP and a media advertised IP. The media
socket binds the former; the SDP session and media `c=` lines serialize the latter. Existing calls
that use the constructors and supply only one IP retain their current behaviour by using it for
both roles. Explicit bind selection must not reinterpret the existing media-address argument.

The concrete seam is `sipx_call::MediaAddress::new(advertised).with_bind(bind)`. Outbound calls
constructed with `DialOptions::new(..., advertised)` add `with_media_bind_address(bind)`. Inbound
entry points keep their existing `IpAddr` signatures; the new `answer_at`,
`answer_with_policy_at`, `answer_with_policy_and_headers_at`,
`answer_ringing_with_policy_at`, `ring_early_with_policy_at` and
`ring_offer_early_with_policy_at` variants accept the address pair.

This is a deliberate beta API break for callers that construct or exhaustively pattern-match the
public `DialOptions` struct: they must add `media_bind_address` (equal to `media_address` to retain
the old behavior), or migrate to `DialOptions::new` and its builders. Constructor-based outbound
callers and the existing inbound functions retain bind-equals-advertise behavior.

An unspecified bind address is valid. An unspecified advertised address is refused before a
request or response is sent because it cannot be a remote destination.

For CLI calls without `--advertise`, the route-selected reachable local address is both advertised
and bound; this preserves host and server-reflexive ICE gathering when the signalling listener is
wildcard-bound. Supplying `--advertise` selects the independent form: that explicit address is
advertised while the `--local` IP remains the media bind choice.

## 3. Initial request vector

Given:

```text
signalling bind       0.0.0.0:5060
media bind            0.0.0.0
advertised host       198.51.100.44
advertised SIP port   5080
allocated RTP port    40000
```

the initial INVITE has all of:

```text
Via: SIP/2.0/UDP 198.51.100.44:5080;branch=...;rport
Contact: <sip:alice@198.51.100.44:5080>
c=IN IP4 198.51.100.44
m=audio 40000 RTP/AVP ...
```

The test for this vector inspects one built request. Three isolated unit assertions are not a
substitute: consistency across Via, Contact and SDP is the property.

## 4. Response routing

For a request carrying bare `rport`, the server writes the observed source port into `rport` and
writes `received` when the observed source address differs from Via `sent-by`. That rule applies to
registration responses, initial call responses and in-dialog responses. The values come from the
transport envelope, never from Contact or SDP.

An initial client request may carry a preloaded Route set. `DialOptions::with_service_route`
serializes that set and preserves its order; it does not resolve a Route URI. The application
resolves the outermost loose-route hop and supplies it as the `Target` passed to `dial`, while the
Request-URI remains the called party. The test therefore delivers the INVITE to a chosen proxy
target whose address differs from the Request-URI and inspects both fields independently.

## 5. Non-ICE media destination

Before a valid RTP packet arrives, the session sends to the peer's SDP destination. After a valid
packet arrives, the session sends RTP and its paired RTCP flow to that packet's observed source as
specified by RFC 4961. A malformed, unauthenticated or wrong-SSRC packet cannot move the latch.

The black-hole vector offers a bound but unread SDP destination, then sends valid RTP from a
different reachable source. Audio returned after that packet must reach the observed source, while
the offered sink must receive none.

## 6. ICE precedence

When ICE is disabled, the symmetric-RTP latch in §5 owns destination changes. When ICE is enabled,
only the nominated candidate pair selects the destination. Ordinary RTP arriving from another
address cannot replace that pair. Before nomination, the media path follows the ICE state machine
and must not silently fall back to the non-ICE latch.

This is construction-level behavior in `MediaSession`: a session with an ICE driver sets its
source-learning switch to false. The test supplies a nominated destination, admits a valid RTP
packet from another source, and asserts that the destination cell remains the nominated address.

The call API documents this precedence beside the explicit media bind option. The CLI exposes an
advertised IP independently from its signalling bind socket and reports the chosen advertised and
bound addresses in JSON diagnostics.
