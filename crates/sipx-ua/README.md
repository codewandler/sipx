# sipx-ua

SIP user agent: registration, digest authentication, and experimental subscriptions and presence.

## What this is

Stateful user-agent services over the transport layer: registration leases, digest challenges,
reachability identifiers, and experimental subscriptions, event packages, and published presence.

## Stability

The supported and experimental surfaces are maintained in the
[crate-level Stability section](https://codewandler.github.io/sipx/api/sipx_ua/#stability).
That is the contract; it is linked rather than copied here so the two cannot drift.

## Deliberately absent

Dialogs, calls, and media are higher-layer concerns. Runtime features can also be disabled so pure
digest and header logic do not force a socket runtime into a sans-I/O consumer.

## See also

- [`sipx-transport`](../sipx-transport/README.md) — endpoint I/O beneath the user agent.
- [`sipx-call`](../sipx-call/README.md) — dialogs and calls above it.
