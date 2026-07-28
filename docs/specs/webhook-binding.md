# Spec: The webhook binding (document mode)

**Status:** draft — `A-2` finishes it; vectors required before any code · **Epic:** `app-host` ·
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
  budget with the **same `seq` and identical body**. `on_5xx` / `on_unreachable` apply when
  the budget is spent, not per attempt. Retry pacing is host policy; the observable contract
  is: same bytes, same `seq`, bounded by the declared budget, then the declared outcome.
- **[sipx-app]** A 4xx is not retried — the app said the request is wrong; `on_4xx` applies
  immediately.

## 3. Trust

- **[sipx-app]** Every request carries the contract's `Sipx-Signature`; the key is a named
  secret from [host-config.md](host-config.md). Key rotation means two named keys valid
  simultaneously; the spec's vectors must include a rotation window.
- **[sipx-app]** The response is trusted because the channel is: the host validates the
  document (contract §6.4), never its origin beyond TLS. Apps that need mutual
  authentication front themselves with it; the host does not grow client-cert config in v1.

## 4. Open until A-2

Retry pacing and attempt caps inside the budget; connection reuse; whether a 3xx is followed
(inclination: no — a moved app is configuration, not a redirect); the vector set (delivery,
timeout-then-late-response, retry-then-declared-outcome, signature vectors including
rotation).
