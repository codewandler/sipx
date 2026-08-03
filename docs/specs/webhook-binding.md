# Spec: The webhook binding (document mode)

**Status:** implemented · **Epic:** `app-host` ·
**Design:** [app-host](../designs/app-host.md)

> The wire is the contract:
> [`app-contract.md`](app-contract.md)
> §5–§7 and §9 define the envelope, the documents, the alternation rule and the signature.
> This spec adds only what the *host* must do around it — delivery, redelivery, and the
> mapping of transport failures onto the app's declared failure semantics. It may not extend
> the vocabulary.

## 1. Delivery

- **[sipx-app]** One `POST` per event, serialized per call: the contract's
  at-most-one-outstanding rule is enforced by the host, and events queue in order behind the
  outstanding request.
- **[sipx-app]** `timeout_ms` from the app's declaration bounds the whole exchange (connect,
  send, response). On expiry the declared `on_timeout` applies **and the exchange is
  abandoned**: a response that arrives later is discarded (its `seq` is already resolved —
  contract vector AC-4 territory).

## 2. Redelivery

- **[sipx-app]** A failed delivery (connect failure, 5xx) is retried within `timeout_ms`'s
  budget with the **same `seq`, identical body, timestamp and signature**. At most three
  attempts are made. The second starts 100 ms after the first retriable result and the third
  200 ms after the second; an attempt or delay that would start after the budget is exhausted
  is omitted. The timeout applied to each attempt is the whole budget still remaining, so DNS,
  connect, TLS, send and response time cannot escape the call's declaration.
- **[sipx-app]** When no attempt succeeds, the last retriable result selects the declaration:
  no HTTP response is `on_unreachable`; a `5xx` response is `on_5xx`. A response with another
  non-success status that the contract does not classify (including `3xx`) is a binding failure
  and applies `on_5xx` without retry. The budget itself expiring while any attempt is in flight
  is `on_timeout`, irrespective of an earlier result.
- **[sipx-app]** A 4xx is not retried — the app said the request is wrong; `on_4xx` applies
  immediately.

## 3. Trust

- **[sipx-app]** Every request carries the contract's `Sipx-Signature`; the key is a named
  secret from [host-config.md](host-config.md). Each configured key contributes one `v1=` value
  to the single header, in configuration order; the head remains the active key and the second
  is the retiring key during a rotation window. All named keys are resolved before the host
  starts accepting calls. The timestamp and every `v1` value are computed once per logical
  delivery and retained across its attempts.
- **[sipx-app]** The `sipx-host` process resolves a name from the byte value of
  `SIPX_SECRET_<name>` at startup (the name is used exactly, including lowercase letters, dots or
  dashes). Library embedders supply an explicit resolver to `Host::start_with_secrets`. A missing
  or empty value is a startup error before a listener is bound.
- **[sipx-app]** The response is trusted because the channel is: the host validates the
  document (contract §6.4), never its origin beyond TLS. Apps that need mutual
  authentication front themselves with it; the host does not grow client-cert config in v1.

## 4. HTTP behaviour

- **[sipx-app]** Redirects are never followed. An app moving is a configuration change, and
  replaying a signed call envelope to an address the operator did not name crosses a trust
  boundary.
- **[sipx-app]** One HTTP client is owned by the host and shared by call actors. Its connection
  pool may reuse a connection across deliveries and calls. Serialization remains per call; reuse
  does not permit a second outstanding callback for that call.
- **[sipx-app]** A successful response is any `2xx`. Its body is passed unchanged to
  `sipx-app-protocol::Interpreter` as `Response::Body`; an empty body therefore means “keep
  going”, and malformed JSON or an invalid document is classified by the interpreter as
  `on_5xx`. The binding never parses or interprets a response document.

## 5. Vectors

The test names are the vector identifiers. Times are offsets from the delivery's start; the
driver supplies the Unix timestamp, so none of these depends on the test machine's clock.

| ID | Script | Required result |
|---|---|---|
| WB-1 | `2xx` with a document | one POST; exact envelope bytes become the body; the body is returned unchanged |
| WB-2 | no response through `timeout_ms`, then a late `2xx` | `Timeout`; the late body is never presented to the interpreter |
| WB-3 | connect failure, wait 100 ms, connect failure, wait 200 ms, connect failure | three byte-identical attempts; `Unreachable` |
| WB-4 | `500`, wait 100 ms, `503`, wait 200 ms, `502` | three byte-identical attempts; `ServerError` carrying the last status |
| WB-5 | `400` | one attempt; `ClientError`; no retry |
| WB-6 | `302` with `Location` | one attempt; redirect target sees nothing; `ServerError` |
| WB-7 | budget expires during a retry sequence | no attempt starts after the budget; `Timeout` wins while an attempt is in flight |
| WB-8 | key `new` at Unix second 1,772,270,104 | one `Sipx-Signature`: `t=1772270104, v1=<HMAC-SHA-256(new, "1772270104." + body)>` |
| WB-9 | keys `new`, `old` at the same instant | the header has `t`, then the `new` and `old` `v1` values in that order; all attempts repeat it exactly |

WB-1 through WB-7 run against a real loopback HTTP peer. Their four resulting contract failures
are then applied by A-7's shared, fake-time failure-knob scenarios; retry and redirect behaviour
cannot be expressed in A-7's deliberately socket-impossible binding type. WB-8 and WB-9 use fixed
bytes and keys, so their expected hexadecimal values are test literals rather than values computed
by the assertion.
