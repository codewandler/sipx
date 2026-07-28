# Spec: Host configuration

**Status:** draft — `A-1` finishes it; vectors required before any code · **Epic:** `app-host` ·
**Design:** [app-host](../designs/app-host.md)

> One document describes a running host. Everything an operator can decide is in it; nothing
> in it is a program. This spec fixes the schema's *shape and semantics*; the concrete syntax
> examples below are illustrative until `A-1` pins them with vectors.

## 1. Shape

```toml
[listener.sip]            # kernel endpoint: bind, transport, advertised address

[app.reception]           # one section per app
binding = "webhook"       # webhook | session | embedded
url     = "https://apps.example.net/reception"   # per-binding fields, see the binding specs

[app.reception.on_failure]
timeout_ms     = 2000
on_timeout     = "continue"
on_5xx         = "continue"
on_unreachable = "continue"
on_4xx         = { reject = 500 }

[app.reception.grants]
play_roots   = ["/var/lib/sipx-app/audio/reception"]
dial_headers = ["x-campaign"]
```

## 2. Normative points

- **[sipx-app]** Failure semantics fields are exactly the contract's §9.2 knobs — same names,
  same values, same defaults. This file may not invent a semantic the contract lacks.
- **[sipx-app]** Grants are deny-by-default. An absent `grants` table means: `play` may use
  inline audio only, `dial` may set no extra headers.
- **[sipx-app]** Which app a call reaches is a total function of configuration (listener →
  app in v1). If no app matches, the declared listener default applies (refuse with a
  configured status); silence is not an option.
- **[sipx-app]** Reload is deliberate: a reload either applies wholly and atomically or is
  rejected wholly with the reason; live calls keep the policy they started with.
- **[sipx-app]** Secrets (webhook signing keys, session bearer material) are referenced by
  name, resolved outside this file; the document itself is committable.

## 3. Open until A-1

The concrete syntax (TOML above is a proposal), the listener schema's transport details, the
multi-app-vs-multi-process stance A4 needs left open, and the vector set (valid documents,
rejected documents, reload transitions).
