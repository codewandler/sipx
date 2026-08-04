# Spec: RTCP multiplexing and DTLS setup-role negotiation

**Status:** normative · **Story:** `M-46` · **Crates:** `sipx-sdp`, `sipx-media`, `sipx-call`

This document owns the two generic offer/answer choices needed before the stricter
[browser-audio profile](webrtc-audio.md) can compose them. The browser profile requires both;
ordinary SIP calls negotiate each independently and retain their established fallback behavior.

## 1. Normative references

- **RFC 5761 §4** — RTP and RTCP packet distinction on one port, including the payload-type range
  that cannot be used when multiplexing is active.
- **RFC 5761 §5.1.1 and §5.1.3** — `a=rtcp-mux` offer/answer and fallback to separate RTP/RTCP
  ports when the answer omits the attribute.
- **RFC 4145 §4 and §4.1** — `a=setup` roles and their legal offer/answer combinations.
- **RFC 5763 §5** — the RFC 4145 roles as used by DTLS-SRTP; an offerer uses `actpass`, an
  answerer selects `active` or `passive`, and `active` is preferred for NAT traversal.
- **RFC 8122 §6.2** — certificate verification after the role-selected handshake and before media
  is accepted.

The existing [SRTP specification](srtp.md) remains normative for fingerprints, DTLS profile and
key-export ordering. [ICE](ice.md) remains normative for components and selected pairs.

## 2. Types

| Type | Values | Meaning |
|---|---|---|
| `RtcpMode` | `Separate`, `Mux` | the result of offer/answer for one media section |
| `SetupCapabilities` | can act as DTLS client/server, preferred answer role | roles the local handshake can actually hold |
| `SetupRoleError` | §4.3 | a role exchange that cannot start a conforming handshake |
| `MuxedPacket` | `Rtp`, `Rtcp` | the second-stage class inside RFC 5764's RTP-or-RTCP range |

`RtcpMode` is per media section and is fixed before its workers start. It is not guessed from the
first packet and does not change merely because an RTCP packet arrives on one of the ports.
In-dialog descriptions preserve the running mode. An offer or answer that would change it receives
the typed `RtcpModeChange` refusal before ICE or media state changes; changing socket ownership
mid-dialog is outside this story and requires an explicit media-session replacement.

## 3. RTCP mux offer/answer

### 3.1 State table

| Local action | Peer description | Result |
|---|---|---|
| offer mux | answer has `a=rtcp-mux` | `Mux`; RTP and RTCP use the offered RTP port |
| offer mux | answer omits `a=rtcp-mux` | `Separate`; RTP uses the media port and RTCP uses the control port without a second exchange |
| answer with mux capability | offer has `a=rtcp-mux` | answer includes `a=rtcp-mux`; `Mux` |
| answer with mux capability | offer omits it | answer omits it; `Separate` |
| answer without mux capability | any offer | answer omits it; `Separate` |

An endpoint never inserts `a=rtcp-mux` into an answer when it was not offered. A rejected media
section does not negotiate a mode.

### 3.2 One-port behavior

In `Mux`, the media socket is the only RTP/RTCP socket used by the running session:

1. outbound RTP and RTCP both originate there;
2. outbound RTCP is addressed to the peer's RTP destination, not `port + 1`;
3. inbound RTP and RTCP are read by one owner and classified before either parser runs; and
4. after mux is agreed, ICE checks and selects only component 1 for the stream.

Inside RFC 5764's first-byte range `128..=191`, the second byte decides:

| Second byte | Class |
|---|---|
| `192..=223` | RTCP/SRTCP |
| anything else | RTP/SRTP |

Therefore payload types 64 through 95 MUST NOT be offered or accepted for a muxed stream: with the
RTP marker bit set, their second byte occupies the RTCP range. An empty datagram or one-byte prefix
is malformed input and reaches neither parser.

In `Separate`, the RTP socket does not apply the RFC 5761 second-byte classifier. RTCP continues to
use and be received on the control socket, and its destination remains the peer media port plus one
unless ICE selected component 2. This is a real fallback, not mux inferred from packet contents.

### 3.3 Resource ownership

| Mode | receive owners | RTCP send socket | ICE components |
|---|---:|---|---:|
| `Mux` | one owner for the media socket | media socket | 1 |
| `Separate` | media owner plus control owner when bound | control socket when bound, otherwise media socket | 1 or 2 according to [ice.md](ice.md) §6.1 |

No mode starts two tasks reading the same socket. Queue and datagram limits are those already owned
by `sipx-media`; negotiating mux does not create an unbounded handoff.

An initial ICE offer that permits the separate-port fallback still advertises both components and
an `a=rtcp` fallback destination, as RFC 5761 §5.1.3 requires. An answer agreeing to mux emits only
component 1, and the offerer then checks component 1 only. The stricter browser-audio profile has no
fallback and therefore gathers component 1 alone from the start.

## 4. DTLS setup-role negotiation

### 4.1 Offer and answer table

`Setup::Active` means the endpoint sends `ClientHello` and uses the DTLS client key direction.
`Setup::Passive` means it accepts `ClientHello` and uses the server direction.

| Offer | Legal answer | Offerer's local role | Answerer's local role |
|---|---|---|---|
| `actpass` | `active` | `passive` / server | `active` / client |
| `actpass` | `passive` | `active` / client | `passive` / server |

A DTLS-SRTP offer generated by sipx always carries `a=setup:actpass`. An answer generated by sipx
selects `active` when its capabilities permit it, otherwise `passive` when permitted. It never
copies `actpass` into an answer.

`holdconn` is not a DTLS-SRTP role: it requests that no connection be formed. A DTLS-SRTP offer
carrying it is refused before the answerer binds or gathers, rather than producing a successful SIP
answer which can fail only when the handshake should start.

### 4.2 Capability and ordering rules

Role selection is checked before a handshake worker starts. A local endpoint may advertise only a
role its handshake implementation can hold. The selected role is then passed unchanged to the DTLS
adapter.

The existing security order is unchanged:

1. require a supported signalled fingerprint;
2. run the handshake in the negotiated complementary role;
3. obtain the peer certificate and compare its fingerprint in constant time;
4. require an agreed SRTP protection profile;
5. export and split keys; and
6. install both directions before any media worker starts.

A mismatch returns no keys. Neither role negotiation nor mux permits a fallback to SDES or clear
RTP.

### 4.3 Typed refusals

| Error | Input |
|---|---|
| `UnresolvedOffer` | a DTLS-SRTP offer says `holdconn` |
| `MissingAnswer` | a DTLS answer has no `a=setup` |
| `UnresolvedAnswer` | an answer says `actpass` or `holdconn` instead of selecting a role |
| `UnsupportedLocalRole` | the complementary local role is outside `SetupCapabilities` |
| `NoAnswerRole` | an answerer supports neither legal role for the offer |

These errors occur before the handshake. They name roles, never certificate or key material.

## 5. Test vectors

| ID | Input | Required result |
|---|---|---|
| `MUX-SDP-1` | offer and answer both carry `a=rtcp-mux` | `Mux` and the answer carries the flag |
| `MUX-SDP-2` | offer carries mux; answer omits it | `Separate` without renegotiation |
| `MUX-SDP-N1` | rejected offer and answer retain mux flags | `Separate`; rejected media never owns a mode |
| `MUX-PKT-1` | `80 c8` on the RTP port in `Mux` | RTCP parser and feedback state, not RTP parser |
| `MUX-PKT-2` | ordinary RTP on a separate RTP port | RTP parser; separate RTCP path remains live |
| `MUX-ICE-1` | initial mux-capable ICE offer | components 1 and 2 plus explicit `a=rtcp` fallback |
| `MUX-ICE-2` | answer agrees to mux | exactly component 1; no component-2 candidate or owner |
| `MUX-RENEG-N1` | running `Mux`; later offer or answer omits mux | typed refusal; old ICE/media state unchanged |
| `SETUP-1` | offered `actpass`, answer `active` | offerer is DTLS server |
| `SETUP-2` | offered `actpass`, answer `passive` | offerer is DTLS client |
| `SETUP-N0` | DTLS-SRTP offer says `holdconn` | `UnresolvedOffer`; no bind, gather or answer |
| `SETUP-N1` | answer `actpass` | `UnresolvedAnswer`; no handshake |
| `SETUP-N2` | answer selects `active`, local cannot be server | `UnsupportedLocalRole`; no handshake |
| `KEY-N1` | matching role and wrong certificate | fingerprint mismatch and zero returned keys |

Tests may add complete packets around the stated classifier bytes, but the IDs and outcomes remain
the contract.
