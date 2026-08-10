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

## Why diagnostic values are redacted

The `trace` row's “never credentials, keys or message bodies” is backed by the type surface rather
than by review alone. [scripts/check-audio-claims.py](https://github.com/codewandler/sipx/blob/main/scripts/check-audio-claims.py)
walks every reachable public `Debug`, including private carriers that rendering can reach. It
requires raw sample buffers to render a class and count rather than audio, protects opaque byte
buffers on the `sipx-media`/`sipx-rtp` relay path and in the `sipx-app-protocol`/`sipx-testkit`
crates where a call comes to rest, and treats every fixed-size `[u8; N]` array as a key unless the
type states why it is not a secret.

Redaction is not anonymisation. Protocol headers are deliberately left visible because routing,
negotiation and correlation are what a protocol diagnostic exists to explain; those headers can
include SIP identities. Counts and lengths are also kept on purpose, so an operator can distinguish
an empty message from a full packet or a short frame from an unbounded one without recording its
contents. See [Privacy and diagnostic redaction](privacy.md) for the scopes and their boundary.

The checked guarantee covers ordinary rendering of sipx values. A host that explicitly logs a
field, installs a formatter that captures inputs, or retains a record has made a separate policy
decision; the library cannot redact data the application deliberately extracts.

Per-message signalling detail is never `info`. Turning on ordinary lifecycle reporting must not
turn one call into one record per SIP message. Use `debug` or `trace` only while investigating a
specific flow, and apply the host's own redaction and retention policy.

Applications should rely on returned errors, typed events and exported counters for behavior. A log
record is supporting evidence, not the only notification that an operation failed or data was
discarded.
