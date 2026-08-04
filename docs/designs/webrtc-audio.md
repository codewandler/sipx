# Design: browser-compatible WebRTC audio

**Status:** proposed; the component protocols exist, their browser-audio composition does not ·
**Pillar:** Media · **Epic:** `webrtc-audio` · **Tracker:** `M-38`

## Scope

This epic delivers one browser-compatible **audio** path between a sipx endpoint and an independently
implemented browser SIP endpoint. It composes SIP over secure WebSocket (RFC 7118), ICE (RFC 8445 and
RFC 8839), DTLS-SRTP (RFC 5763, RFC 5764 and RFC 8827), RTP/RTCP multiplexing (RFC 5761), JSEP's SDP
profile (RFC 8829), the WebRTC media profile (RFC 8834), and the mandatory audio formats in RFC 7874.

It does not turn sipx into a full WebRTC stack. Video, browser APIs, capture/render UI, data channels,
SCTP, simulcast, multiple bundled media sections, and a general-purpose browser media engine remain
out of scope. `M-24`'s TURN client widens which NAT topologies are reachable, but running a relay is
not part of this epic and TURN is not required to demonstrate the first host/server-reflexive path.
The public boundary must continue to say that relay-required networks are not served until `M-24`.

## What already exists

| Layer | Existing evidence | Current boundary |
|---|---|---|
| SIP signalling | `T-8`, `T-9`, `T-23` | WS/WSS and non-root request paths are implemented and independently exercised |
| Audio formats | `M-3`, `M-7`, `M-13`, `M-30`, `P-9`, `P-13` | G.711, telephone events and Opus are call- and CLI-reachable |
| ICE | `M-19` … `M-23`, `M-27`, `P-9` | host and server-reflexive paths work; TURN remains `M-24` |
| DTLS-SRTP | `M-15`, `M-28`, `P-9` | a call can use it, but the call layer refuses to combine it with ICE |
| Browser media profile | none | no `RTP/SAVPF`, `a=rtcp-mux`, combined ICE/DTLS path, or browser-peer proof |

The last row is the epic. The rows above are prerequisites to reuse, not work to reimplement.

## Design direction

- Add a named browser-audio media profile above the existing independent codec, ICE and keying
  choices. Selecting it is fail-closed: it either negotiates the complete profile or returns a typed
  error before sending a weaker offer.
- Use one ICE component for RTP and RTCP. The selected candidate pair becomes the DTLS peer; the
  handshake must not connect to the provisional SDP address and then move encrypted media elsewhere.
- Demultiplex STUN, DTLS, SRTP and SRTCP on that selected component. RTCP multiplexing changes socket
  ownership and SDP together, so it must not be implemented as an attribute the runtime ignores.
- Offer the audio vocabulary RFC 7874 requires: Opus, PCMU, PCMA, comfort noise for G.711, and
  `telephone-event`. Opus remains an explicit optional build feature until its native dependency
  policy changes; a build lacking it cannot claim the browser-audio profile.
- Keep the proof at the product boundary: a bounded shell run drives sipx and an independently
  implemented browser SIP endpoint, exchanges audible, non-silent Opus in both directions over WSS +
  ICE + DTLS-SRTP, and reports the negotiated path. A wrong fingerprint, absent nominated pair, or
  weaker-media answer must fail explicitly.

## Exit

`M-38` is the single tracker. The epic is done only when its normative profile exists, the combined
port and SDP behavior are implemented, an independently implemented browser SIP endpoint passes the
audio exchange in both call roles, and the RFC registry and public fit boundary describe both the
working path and the remaining TURN/full-WebRTC exclusions.
