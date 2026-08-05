# Publication endpoint contract

**Status:** normative · **Owner:** `sipx-ua` protocol core and `sipx-call` endpoint driver ·
**RFC:** RFC 3903

## 1. Boundary

The publication core is sans-I/O. Requests, final responses and fired timer generations enter as
values; complete PUBLISH requests, timer changes and typed state changes leave as values. The
endpoint driver alone owns transactions, clocks and cancellation.

The inbound service uses the existing `presence::Compositor` allocation. It does not introduce a
second publication database. The application injects that compositor and an authorization policy;
durability, distributed coordination, PIDF composition and identity policy remain outside this
driver.

## 2. Protocol fields

RFC 3903 §§4.1 and 11.3 define `SIP-ETag` and `SIP-If-Match` as single opaque token fields. Both
names are known to the parser and duplicates or non-token values fail closed. The four requests are:

| Operation | Body | SIP-If-Match | Expires |
|---|---:|---:|---:|
| initial | required | absent | positive |
| refresh | absent | current tag | positive |
| modify | required | current tag | positive |
| remove | absent | current tag | zero |

Every successful response, including removal, contains exactly one fresh `SIP-ETag` and exactly one
`Expires`; the removal interval is zero (RFC 3903 §6 step 6). A client replaces its retained tag
only after both fields validate. It discards the tag on 412 and never retries that conditional
request; the application must begin a new initial publication with the complete body (RFC 3903 §5).

PUBLISH does not create a dialog. Record-Route and Contact do not alter subsequent requests
(RFC 3903 §4). One logical publication keeps its Call-ID and From tag and increments CSeq for every
new request or authentication/interval retry. It never has two new PUBLISH requests in flight for
the same resource (RFC 3903 §4).

## 3. Inbound decision table

The dispatcher selects PUBLISH before call-dialog routing and gives it to the configured publication
service. Requests for the same resource are processed synchronously in receive order and each store
mutation is atomic (RFC 3903 §6).

| Check, in order | Response and mutation |
|---|---|
| required SIP request fields and PUBLISH CSeq parse | 400; none |
| injected authorization accepts the request/source | 403; none |
| exactly one supported Event (`presence`) | 489 with Allow-Events; none |
| zero or one valid SIP-If-Match token | 400; none |
| zero or one parseable Expires, otherwise configured default | 400; none |
| positive expiry below configured minimum | 423 with Min-Expires; none |
| body exceeds configured byte bound | 413; none |
| body operation is not UTF-8 PIDF or has another media type | 400/415; none |
| a conditional tag is absent, expired or belongs to another resource | 412; none |
| a new resource would exceed active-publication capacity | 503 with Retry-After; none |
| accepted | 200 with fresh SIP-ETag and granted Expires; arm/replace one expiry task |

Expiry is checked at request time before matching a tag, not only when a sweep task happens. A stale
conditional request therefore receives 412 independently of scheduler timing. Removal cancels the
resource's task and stores no state. Dispatcher/service cancellation aborts and joins every owned
expiry task before its observable active-task count reaches zero.

## 4. Outbound state table

The public publisher begins with a bounded body and positive requested expiry. A successful initial
response enters `Published { tag, expires }`, schedules Refresh at four fifths of the granted
interval and retains a finite local Expiry. Modify and automatic refresh use the current tag;
remove uses that tag with `Expires: 0`.

| Input/result | Transition |
|---|---|
| 401/407 with usable challenge | bounded digest retry, increment CSeq, fresh driver cnonce |
| 423 with increasing in-policy Min-Expires | bounded interval retry with that Expires |
| valid 2xx initial/refresh/modify | replace tag and granted expiry; replace Refresh and Expiry |
| valid 2xx remove | discard tag, cancel timers, terminate `Removed` |
| 412 conditional operation | discard tag, cancel timers, terminate `StaleTag`; no retry |
| malformed/duplicate/missing SIP-ETag or Expires in 2xx | terminate `MalformedResponse` |
| non-success final or transaction failure | terminate with the typed status/failure |
| Refresh timer | send bodyless conditional PUBLISH; keep old Expiry until success |
| Expiry timer | terminate `LocalExpiry`; do not resurrect |
| CSeq at `u32::MAX` | terminate `LocalCSeqExhausted`; emit no request |
| application remove while another operation is live | retain at most one pending remove; send it after that operation ends |
| driver shutdown deadline | cancel/join response and timer work; terminate `Shutdown` |

Application bodies, logical publishers, command queues and deliveries are all bounded. The driver
owns at most one transaction task, one Refresh task and one Expiry task per publication.

## 5. Conformance vectors

- **S39-V1 — inbound lifecycle.** Initial PIDF PUBLISH receives 200 with tag A and granted expiry;
  refresh with A receives tag B; modify with B replaces the document and receives C; remove with C
  receives fresh tag D, `Expires: 0`, and leaves the compositor and task count at zero.
- **S39-V2 — conditional failure.** A refresh with an unknown or expired tag receives 412, no tag,
  no timer and no store mutation.
- **S39-V3 — interval negotiation.** A positive interval below policy receives 423 and exactly one
  Min-Expires. Retrying at that value receives 200, a fresh tag and the granted interval.
- **S39-V4 — fail-closed input.** Duplicate/malformed conditional or expiry fields, an oversized
  body, wrong media type and an unauthorized source each produce their table status without state.
- **S39-V5 — authenticated outbound lifecycle.** Initial PUBLISH is challenged, its digest retry
  increments CSeq, and 200 stores tag A/expiry. Refresh sends A and stores B; modify sends B and
  stores C; remove sends C and terminates only after a 200 containing D and zero expiry.
- **S39-V6 — stale outbound tag.** A 412 to refresh/modify/remove discards the tag, sends no retry
  and emits `StaleTag`, after which a new public `publish` call starts without SIP-If-Match.
- **S39-V7 — response validation and CSeq bound.** Missing, malformed or duplicate tag/expiry in
  2xx is typed and changes no retained authority; CSeq exhaustion emits no request.
- **S39-V8 — owned cleanup.** With paused time and real endpoints, removal and dispatcher shutdown
  leave publisher tasks, timers, transactions, compositor publications and endpoint transactions
  at zero after the protocol retention horizon.
