# Application-owned dialog extensions

This specification defines the narrow escape hatch through which an application handles SIP
methods whose semantics are not owned by the call state machine. It does not weaken the specialized
paths for session negotiation, transfer, capability discovery, or teardown.

## Normative references

- RFC 3261 §§8.1.1, 8.2.6.2, 12.2 and 22: dialog request construction, response construction,
  sequence ordering, remote-target refresh, and digest challenges.
- RFC 3428 §§4 and 7: in-dialog MESSAGE construction and response handling.
- RFC 6086 §§4.2, 4.3 and 4.5: INFO package negotiation, bodies, and responses.

## Types and limits

`ApplicationRequest` is an owned snapshot of one accepted request:

| Field | Contract |
|---|---|
| `method` | `INFO`, `MESSAGE`, or an explicitly admitted `Method::Other` token |
| `headers` | the parser-validated, ordered header collection |
| `body` | owned bytes, at most 65,536 octets |
| response capability | owns the server transaction and can send one final response |

An application body is meaningful only when a `Content-Type` header is present. A non-empty body
without one is rejected with `415 Unsupported Media Type`; it is never surfaced or partially sent.
The same rule applies outbound as a typed error. The content type's package-specific meaning remains
application policy.

The application may add end-to-end headers, but it cannot supply `Via`, `Route`, `From`, `To`,
`Call-ID`, `CSeq`, `Max-Forwards`, `Content-Length`, `Authorization`, or `Proxy-Authorization`.
Those fields are derived or maintained by the dialog and transaction layers.

`INVITE`, `ACK`, `BYE`, `CANCEL`, `OPTIONS`, `PRACK`, `UPDATE`, `SUBSCRIBE`, `NOTIFY`, `REFER`,
`PUBLISH`, and the known non-dialog methods are never application-owned through this API. INFO and
MESSAGE are admitted intrinsically. An unknown token is admitted only after the application names
that exact, case-sensitive `Method::Other` value on the call.

## Inbound state table

| Input | State change | Output |
|---|---|---|
| method is stack-owned | specialized handler only | the specialized handler's response/event |
| unknown method is not admitted | none | caller returns `false`; dispatcher policy applies |
| accepted CSeq is not newer | none | `500 Server Internal Error` |
| body exceeds 65,536 octets | none | `413 Content Too Large` and typed error |
| non-empty body has no content type | none | `415 Unsupported Media Type` and typed error |
| valid application-owned request | record remote CSeq | one `ApplicationRequest` event |
| capability sends a final response | claim capability | exactly that response |
| capability is dropped unclaimed | claim capability | `500 Server Internal Error` |
| capability remains unclaimed for 32 seconds | claim capability | `504 Server Time-out` |
| any second claimant | none | typed `ResponseAlreadySent` error or no-op fallback |

The 32-second interval is a bound on failure, aligned with SIP's 64*T1 transaction horizon. It is
not a delay used to establish ordering. Drop responds immediately when a runtime is available; the
timer remains the bounded fallback for cancellation and abandoned tasks.

## Outbound state table

| Input | State change | Output |
|---|---|---|
| call already ended | none | typed `DialogEnded` error |
| method is not application-owned/admitted | none | typed `StackOwnedDialogMethod` error |
| protected header or invalid body | none | typed error; no transaction is created |
| valid request | increment local CSeq | request built from remote target, route set and dialog IDs |
| final 2xx | refresh remote target when supplied | response returned |
| first supported 401/407 and credentials exist | increment local CSeq again | one authenticated retry |
| second challenge or no credentials | none | typed authentication/rejection error |
| other final response | none | typed rejection error |

## Byte-level vectors

For a dialog whose local party is `<sip:a@example.test>;tag=local`, remote party is
`<sip:b@example.test>;tag=remote`, Call-ID is `call@example.test`, next local CSeq is 8, remote
target is `sip:b@192.0.2.20:5070`, and route set is `<sip:proxy.example.test;lr>`, sending MESSAGE
with `Content-Type: text/plain` and body `hi` produces the invariant fields below (the transaction
layer adds Via and its branch):

```text
MESSAGE sip:b@192.0.2.20:5070 SIP/2.0
To: <sip:b@example.test>;tag=remote
From: <sip:a@example.test>;tag=local
Call-ID: call@example.test
CSeq: 8 MESSAGE
Max-Forwards: 70
Route: <sip:proxy.example.test;lr>
Content-Type: text/plain
Content-Length: 2

hi
```

An inbound `INFO` with `CSeq: 9 INFO`, `Content-Type: application/example`, and four body octets
becomes one event preserving that method, content type and body. Replacing the body with 65,537
octets produces `413` and no event. Replacing INFO with BYE reaches teardown and can never become an
application event. Replacing it with `PRIVATE` produces no event until the exact `PRIVATE` token has
been admitted.
