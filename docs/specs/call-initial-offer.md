# Spec: Initial call offer failure

**Status:** normative · **Story:** M-69 · **Design:**
[media interoperability](../designs/media-interoperability.md)

This specification covers failure while an answering call evaluates the session description in an
initial INVITE. It does not change a successfully established dialog or the separately specified
in-dialog offer rules in [`sip-update.md`](sip-update.md).

## 1. Normative references

- RFC 3261 §§8.2.6.2, 13.3.1.3 and 21.4.26 — final-response construction and 488 Not Acceptable
  Here when a syntactically valid request asks for an unsupported session.
- RFC 3261 §21.4.1 — 400 Bad Request when the request body cannot be understood.
- RFC 3261 §§17.1.1.2 and 17.2.1 — non-2xx INVITE transaction retransmission and ACK absorption.
- RFC 3264 §6.1 — an answer contains only media formats present in the offer, and rejects a media
  stream when none are acceptable.
- RFC 4566 — SDP syntax.
- [`sdp-format-identity.md`](sdp-format-identity.md) — the exact codec-format matching rule.
- [`sip-update.md`](sip-update.md) — established-dialog re-offer refusal and survival.

## 2. Failure classes

The answer path classifies failure before it sends a successful final response:

| Class | Example | Local error | Initial INVITE response |
|---|---|---|---|
| Malformed description | the body is not parseable RFC 4566 SDP | `Error::Sdp` retaining the parse reason | 400 `Bad Request` |
| Unsupported session | valid SDP has no active audio format accepted by the selected codec policy | `Error::NoCommonCodec` | 488 `Not Acceptable Here` |
| Internal media failure | local bind, resource, key creation or media-runtime setup fails | the original typed `Io`, `Media`, `Dtls` or profile/configuration error | no synthetic 488; the local failure remains distinguishable |

An all-rejected SDP answer is not sent as a successful INVITE response. Although RFC 3264 defines
port zero for each individually rejected media stream, an INVITE for which this endpoint can create
no usable session is refused 488 under RFC 3261 §13.3.1.3. This tells the caller that the request
arrived and its session was unacceptable; silence would instead look like loss.

Malformed syntax is not a capability mismatch and MUST NOT become 488. An internal failure is not
a statement about the peer's offer and MUST NOT become `NoCommonCodec` or increment a successful
response count. Mapping additional local failures to a separate SIP 5xx is outside M-69.

## 3. Final-response ordering and errors

For 400 and 488, the answer helper MUST build the complete failure response before it claims a
dispatcher-owned invitation. It then claims that invitation immediately before handing the
response to its existing server transaction. A crossing CANCEL therefore either wins and produces
the transaction's 487, or loses to the chosen failure response; both cannot be sent for one INVITE.

The response MUST:

- copy the top Via, From, Call-ID and INVITE CSeq transaction identifiers from the request;
- copy the To address and add the invitation's normal non-empty tag;
- carry an empty body and `Content-Length: 0`;
- use the original server transaction, which owns non-2xx retransmission and absorbs its ACK.

The helper waits until the endpoint has handed the response to the socket before it returns the
local SDP/codec error. Thus command teardown cannot overtake the refusal. If building, claiming or
sending the response fails, that failure is returned instead of the SDP/codec error. Transport
response counters increment only after a successful handoff, so a send failure is observable and
cannot be counted as a sent 400 or 488.

## 4. Command outcomes

For a no-common-codec INVITE, an answering diagnostic command reports `failed` with the explicit
local reason `no codec in common` and exit 1 after the 488 has left. The calling command receives
the final response promptly, reports `rejected` with status 488 and exits 3. It does not wait for
its configured invitation timeout.

## 5. Byte-level vectors

`IOF-1` uses a valid PCMU-only offer against an L16-only answering policy. `<CRLF>` denotes the two
wire bytes carriage-return and line-feed. Whitespace and generated branch/tag values are not fixed,
but the named fields and body bytes are:

```text
INVITE sip:answer@127.0.0.1:5070 SIP/2.0<CRLF>
Via: SIP/2.0/UDP 127.0.0.1:5090;branch=z9hG4bKiof1;rport<CRLF>
From: <sip:caller@example.test>;tag=from-iof1<CRLF>
To: <sip:answer@example.test><CRLF>
Call-ID: iof1@example.test<CRLF>
CSeq: 1 INVITE<CRLF>
Contact: <sip:caller@127.0.0.1:5090><CRLF>
Content-Type: application/sdp<CRLF>
Content-Length: 110<CRLF>
<CRLF>
v=0<CRLF>
o=- 1 1 IN IP4 127.0.0.1<CRLF>
s=-<CRLF>
c=IN IP4 127.0.0.1<CRLF>
t=0 0<CRLF>
m=audio 40000 RTP/AVP 0<CRLF>
a=rtpmap:0 PCMU/8000<CRLF>
```

The response vector is:

```text
SIP/2.0 488 Not Acceptable Here<CRLF>
Via: SIP/2.0/UDP 127.0.0.1:5090;branch=z9hG4bKiof1;rport<CRLF>
From: <sip:caller@example.test>;tag=from-iof1<CRLF>
To: <sip:answer@example.test>;tag=<non-empty-token><CRLF>
Call-ID: iof1@example.test<CRLF>
CSeq: 1 INVITE<CRLF>
Content-Length: 0<CRLF>
<CRLF>
```

`IOF-2` replaces the body with the bytes `v=not-a-valid-session<CRLF>`. It retains the same
transaction fields and tagged To, but the status line is `SIP/2.0 400 Bad Request`. No media port
or task is created in either vector.

`IOF-3` repeats IOF-1's INVITE before ACK. The server transaction retransmits the identical 488 and
does not surface a second invitation. Its matching non-2xx ACK is absorbed by that transaction.

`IOF-4` applies the PCMU-only body as a re-INVITE to an existing L16 dialog. The established-dialog
path retains its existing 488 response with Warning and leaves the working media session alive, as
specified by `sip-update.md`.
