---
title: Logging
description: sipx is silent by default; hosts own tracing subscribers and choose how much lifecycle or protocol detail to collect.
---

# Logging

sipx libraries do not install a logger or tracing subscriber and do not write directly to stdout or
stderr. Without a subscriber they are silent. A binary or service that embeds sipx owns subscriber
installation, filtering, formatting and destination.

The level policy is:

| Level | Meaning |
|---|---|
| `error` | A subsystem cannot uphold an invariant or continue its promised operation; the typed error or counter remains the application's control surface. |
| `warn` | A request, connection or cleanup action was refused or degraded, while the owning task can continue safely. |
| `info` | A low-volume endpoint, registration, call or media lifecycle transition useful in ordinary operation. |
| `debug` | Diagnostic decisions such as retries, routing, negotiation, malformed input and packet disposition. |
| `trace` | Per-message signalling or media metadata when that detail is deliberately added; never credentials, keys or message bodies. |

Per-message signalling detail is never `info`. Turning on ordinary lifecycle reporting must not
turn one call into one record per SIP message. Use `debug` or `trace` only while investigating a
specific flow, and apply the host's own redaction and retention policy.

Applications should rely on returned errors, typed events and exported counters for behavior. A log
record is supporting evidence, not the only notification that an operation failed or data was
discarded.
