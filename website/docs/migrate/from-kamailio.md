---
title: Migrating from Kamailio
description: An honest concept map — which Kamailio roles land in the sipx ecosystem today, which land in the clustered platform, and which have no equivalent yet.
---

# Migrating from Kamailio

Kamailio is a proxy: it routes, forks, load-balances and registers **other people's** traffic.
sipx — the project this site documents — is deliberately not that. It is the *endpoint* side of
SIP: a phone as a library and a CLI, and the kernel underneath a wider ecosystem. So the honest
first answer is:

**You do not migrate a Kamailio deployment to sipx. You migrate it to the sipx ecosystem — and
part of that ecosystem is younger than your deployment.**

## Maps today / not yet

| In your Kamailio deployment | Goes to | Status |
|---|---|---|
| UAC/UAS endpoints, test calls, monitoring probes | sipx CLI + library | **today** |
| Registering agents behind your proxy | sipx as the *client* — `Path`, `Service-Route`, Outbound honoured | **today** |
| TLS / WebSocket / secure WebSocket edges (client side) | sipx transports | **today** |
| Proxy: forwarding, forking, `Record-Route` | [sipx-clstr](https://github.com/codewandler/sipx-clstr), the clustered proxy/registrar built on the sipx kernel | early — in development |
| Registrar and location service | sipx-clstr | early — in development |
| Load balancing / clustering | sipx-clstr — clustering is its founding concern, proved in deterministic simulation | early — in development |
| The routing script | sipx-clstr composes routing from **typed modules**, by design; there is no script language | different by design |
| Presence, dialog state (busy-lamp) | the event framework and the `dialog`, `reg` and `presence` packages are in sipx; the join to a live dialog store or registrar is not, and is where sipx-clstr's location service comes in | **partly today** |

## The parts you can move today

**Probes and test traffic.** If Kamailio sits in your stack with scripted test callers around
it, those callers can be `sipx dial` / `sipx answer` / `sipx register` today: JSON output, a
distinct exit code per outcome, WAV in and out, DTMF, and call-quality statistics on exit. See
[the CLI reference](../reference/cli.md).

**Endpoint-side services.** Anything that *is* a SIP endpoint — a dialler, an announcement
server, an alerting bridge — can be built on `sipx-call` now, in Rust; the
[SDK](../sdk/overview.md) is the path to building the same without Rust, and it is preview
today.

**Nothing has to move at once.** sipx is continuously verified against Kamailio — registration
over UDP, TCP, TLS and WebSocket, including the refusal cases. Your Kamailio keeps running;
sipx endpoints register against it from day one, which is exactly how a gradual migration
starts.

## The part that replaces Kamailio itself

The proxy, registrar and cluster roles live in
[sipx-clstr](https://github.com/codewandler/sipx-clstr) — a separate product on the same
kernel, aimed at operators. Be aware of two things before planning a cutover:

- **It is early.** It is developed specification-first and proved in deterministic simulation
  before it is pointed at a network; read its own status page rather than assuming parity.
- **There is no routing script, on purpose.** Routing policy composes from typed modules with
  declared inputs, validated at startup as a set. If your Kamailio value is a large routing
  script, expect to restate *what* it decides (peers, prefixes, failover, admission) as
  configuration and modules, not to port the script line by line. An async external routing
  hook — consult your own service for egress selection per call — is part of its design.

## What does not carry over

- A drop-in `kamailio.cfg` translator. There is none and there will not be one.
- In-proxy scripting of any kind.
- Modules whose job was to work around endpoint defects you do not have anymore — a stack
  whose parser survives the RFC 4475 torture corpus needs fewer strap-on defences.

If you are unsure which side of the line your deployment sits on, start with
[Does sipx fit?](../guides/does-this-fit.md) — it is written to be disagreed with.
