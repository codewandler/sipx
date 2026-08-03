# GRUU ownership at a user agent

**Status:** implemented (`T-20`, `X-59`) · **Crates:** `sipx-sip`, `sipx-ua`

Normative references: RFC 5627 §4.1 (instance identity), §4.2 (learning and discarding
GRUUs), §4.5 (`gr` identifies a GRUU), §6.1 (request targeting and 404), §7 (wire
parameters); RFC 3261 §8.2.6.2 (the `To` tag on a final response).

## 1. Boundary

sipx implements the user-agent side of RFC 5627. A registrar mints public and temporary GRUUs,
and an authoritative proxy resolves one to a registered contact; neither server role is in sipx.
The UA offers `gruu` and its `+sip.instance`, learns only the GRUUs returned on the registration
row for that instance, and publishes the selected GRUU as its dialog contact.

The proxy normally replaces the GRUU Request-URI with the registered contact before the request
reaches the UA (RFC 5627 §6.1). A peer or test network can nevertheless deliver the original URI
to the wrong flow. The UA is the last component able to compare that URI with the exact values its
registration learned, so it makes the ownership decision below before call setup.

## 2. Types and ownership

| Value | Owner | Meaning |
|---|---|---|
| `InstanceId` | `UserAgent` registration | Stable identity shared with RFC 5626 Outbound |
| `Gruus { public, temporary }` | Successful registration binding | The exact URIs issued for this instance |
| `Request.uri` | Inbound request | Contains `gr` only when the sender addressed a GRUU (§4.5) |

The learned pair is replaced, never merged, on each successful REGISTER and cleared with a failed
or replaced binding. An empty pair provides no ownership evidence: a UA that has not learned a
GRUU must not infer that every GRUU it sees belongs to somebody else.

## 3. Initial-INVITE decision

`UserAgent::answer` applies this table before the application passes an invitation to `sipx-call`:

| Learned GRUU set | Request | Result |
|---|---|---|
| empty | any INVITE | not handled; the application decides |
| non-empty | INVITE without `gr` | not handled; an AOR or contact may legitimately reach it |
| non-empty | INVITE at this set's public or temporary GRUU | not handled; `sipx-call` may answer |
| non-empty | INVITE with `gr` matching neither learned URI | `404 Not Found`; handled |

The refusal is a sipx last-hop safety rule. RFC 5627 §6.1 assigns the authoritative proxy the
same `404` for a GRUU it cannot resolve. Repeating that shape at a UA which has positive knowledge
of its own GRUUs preserves the property §4.5 promises—a GRUU reaches one specific instance—when an
upstream component sends the request down the wrong flow. Answering instead would silently turn an
instance URI into an AOR fan-out and could establish media at the wrong device.

The rule is deliberately limited to an initial INVITE. In-dialog requests are owned and rejected
by dialog matching, while OPTIONS remains a liveness and capability probe answered by the UA. This
story does not turn `UserAgent::answer` into a general proxy or registrar.

Every final refusal adds a `To` tag when the request had none (RFC 3261 §8.2.6.2).

## 4. Byte-level vectors

Assume the current binding learned
`sip:alice@example.com;gr=urn:uuid:11111111-1111-4111-8111-111111111111`.

| Vector | Request line | Expected user-agent result |
|---|---|---|
| G1 | `INVITE sip:alice@example.com;gr=urn:uuid:11111111-1111-4111-8111-111111111111 SIP/2.0` | unhandled; eligible for call setup |
| G2 | `INVITE sip:alice@example.com;gr=urn:uuid:22222222-2222-4222-8222-222222222222 SIP/2.0` | `SIP/2.0 404 Not Found` with a `To` tag |
| G3 | `INVITE sip:alice@example.com SIP/2.0` | unhandled; no instance claim was made |

`each_of_two_registrations_of_an_address_of_record_is_called_individually` carries G1 and G2
through real call setup. Its mutation witness delivers G2 to the wrong flow, lets the old call path
answer it, and verifies audio crosses both ways before failing; the fixed path observes 404 instead.
