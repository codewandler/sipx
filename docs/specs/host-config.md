# Spec: Host configuration

**Status:** normative · **Epics:** `app-host`, `openai` · **Stories:** `A-1`, `A-22` ·
**Design:** [app-host](../designs/app-host.md) ·
**Vectors:** [`crates/sipx-app/src/config/vectors.rs`](../../crates/sipx-app/src/config/vectors.rs),
run by [`crates/sipx-app/tests/config_vectors.rs`](../../crates/sipx-app/tests/config_vectors.rs)

> One document describes a running host. Everything an operator can decide is in it; nothing in
> it is a program. Behaviour comes from handlers — this file declares **capabilities, failure
> semantics, routing and listeners**, and the moment it can express a decision a handler should
> have made, it has become the configuration-driven PBX the design lists as a non-goal.

Everything below is `[sipx-app]`: there is no RFC for configuring a call host. What the file
*names* — transports, status codes, the failure knobs — is specified elsewhere and only
referenced here.

## 1. Normative references

- [`app-contract.md`](app-contract.md) §9.2 — the failure knobs, their values and their
  defaults. §4 for the wire's unknown-field rule, which §3 of this document deliberately
  inverts.
- [`webhook-binding.md`](webhook-binding.md) §3, [`session-binding.md`](session-binding.md) §1,
  [`engine-binding.md`](engine-binding.md) §2 — what each binding needs named here.
- [`openai-realtime.md`](openai-realtime.md) §2–§3 — what a realtime bridge binding needs named
  here, and why its credential is a name rather than a value.
- [`sip-transport.md`](sip-transport.md) §4 — the transports a `sip` listener may name.
- RFC 3986 §3 — the `url` of a webhook app is an absolute URI.
- RFC 3261 §21 — the status codes a refusal may carry.

## 2. Concrete syntax

The document is UTF-8 with no BOM, in the following **subset of TOML**. The subset is the
syntax: a construct outside it is refused with the line that caused it, rather than accepted by
a parser more permissive than this page.

```text
document   = *( comment / table-header / entry / blank )
comment    = "#" *CHAR                       ; to end of line, outside strings
table-header = "[" name *( "." name ) "]"
entry      = key "=" value                   ; inside the table most recently opened
name / key = %x61-7A *( %x61-7A / DIGIT / "-" / "_" )   ; lowercase, starts with a letter
value      = string / integer / boolean / array / inline-table
string     = %x22 *( CHAR / escape ) %x22    ; escapes: \" \\ \n \r \t
integer    = [ "-" ] 1*DIGIT
boolean    = "true" / "false"
array      = "[" [ value *( "," value ) [ "," ] ] "]"
inline-table = "{" [ key "=" value *( "," key "=" value ) ] "}"
```

An array or an inline table may span lines; nothing else may. Not in the subset, and therefore
refused: multi-line and literal strings, dotted keys outside a table header, arrays of tables,
floats, dates, times, hexadecimal and underscore-separated integers, and any key or table name
outside the grammar above. `\u` escapes are refused too — a configuration value that has to be
written as a code point is a value that wants a different key.

The subset is deliberately narrow rather than deliberately convenient. Every construct it omits
is one an operator would have to be able to read in someone else's document during an incident.

## 3. Normative points

- **N1 — The syntax is §2, and a refusal names its line.** A document containing a construct
  outside the subset is refused whole, with the physical line that caused it. *(HC-2, HC-3)*
- **N2 — Unknown keys and tables are refused, not ignored.** This is the opposite of the
  contract's wire rule (§4 of [`app-contract.md`](app-contract.md): unknown *fields* must be
  ignored), and the difference is deliberate. On the wire, ignoring an unknown field lets two
  versions of a peer interoperate. In this file, ignoring `on_5xxx` means an operator who
  declared `hangup` silently gets `continue`, and finds out during an incident. A key that is
  known but not permitted where it appears — `app` on a `session` listener — is refused the same
  way. *(HC-4, HC-5, HC-30, HC-32)*
- **N3 — A key or table declared twice is refused.** Last-wins is a merge, and a merge is the
  other way a document can mean something other than what it says. *(HC-6, HC-7)*
- **N4 — The failure knobs are the contract's §9.2 knobs.** Same names, same values, same
  defaults, and no others: `timeout_ms`, `on_timeout`, `on_5xx`, `on_unreachable`, `on_4xx`.
  This file may not invent a semantic the contract lacks, nor restate one it has. *(HC-1, HC-8,
  HC-9, HC-10, HC-11)*
- **N5 — Grants are deny-by-default.** An absent `grants` table denies everything grantable:
  `play` may use inline audio only, `dial` may set no extra header, and the app may not
  `originate`. *(HC-1, HC-12, HC-13, HC-31)*
- **N6 — Routing is total.** Before the first packet, every listener resolves either to a
  declared app or to a refusal status. Silence is not an option, and neither is a route to an
  app that does not exist. *(HC-1, HC-14, HC-15, HC-16, HC-17)*
- **N7 — Secrets are named, never carried.** No key in this schema takes secret material; the
  keys that concern secrets take a **name**, resolved outside this file. The name grammar (§4.4)
  is narrow enough that a pasted key does not fit through it. The document is committable.
  *(HC-1, HC-18, HC-19, HC-31, HC-37)*
- **N8 — A binding's fields are required at load, not at first call.** An app whose binding
  cannot be used is a document error, discovered before a call arrives rather than by one.
  *(HC-20, HC-21, HC-22, HC-23, HC-31, HC-33, HC-34, HC-35, HC-36)*
- **N9 — A reload applies wholly or is refused wholly, with the reason.** A refused reload
  leaves the running configuration byte-for-byte as it was; there is no partial application and
  no state in between. *(HC-24, HC-25)*
- **N10 — Listener topology is frozen for the life of the process; policy and routing are not.**
  A reload may change apps, failure semantics, grants, secret names and which app a listener
  routes to. It may not add, remove or rebind a listener. This is what makes N9 achievable:
  everything reloadable is a value the host already holds, so nothing that can fail at the
  operating system can fail halfway through a reload. *(HC-26, HC-27)*
- **N11 — A live call keeps the policy it was admitted with, and a reload never ends one.** The
  app's failure semantics, grants and binding are captured at admission. A call that started
  under `on_5xx = "continue"` continues after a reload declares `hangup`; the next call does
  not. *(HC-28)*
- **N12 — The document means the same thing with one app and with many.** See §7: the
  multi-app-versus-multi-process question is open, and the schema does not answer it. *(HC-29)*

## 4. The document

### 4.1 Shape

```toml
[listener.pbx]                     # a SIP endpoint calls arrive on
protocol  = "sip"
transport = "udp"
bind      = "0.0.0.0:5060"
advertise = "sip.example.net:5060"
app       = "reception"

[listener.trunk]                   # a SIP endpoint with no app: refuse, and say so
protocol  = "sip"
transport = "tcp"
bind      = "0.0.0.0:5061"
no_app    = 503

[listener.apps]                    # where session-mode apps connect
protocol = "session"
bind     = "127.0.0.1:8088"

[app.reception]
binding         = "webhook"
url             = "https://apps.example.net/reception"
signing_secrets = ["reception-hook-2026-07", "reception-hook-2026-04"]

[app.reception.on_failure]
timeout_ms     = 2000
on_timeout     = "continue"
on_5xx         = "continue"
on_unreachable = "continue"
on_4xx         = { reject = 500 }

[app.reception.grants]
play_roots   = ["/var/lib/sipx-app/audio/reception"]
dial_headers = ["x-campaign"]
originate    = false

[app.notifier]
binding       = "session"
bearer_secret = "notifier-bearer"

[app.greeter]
binding = "embedded"
handler = "greeter.ts"
```

`[app.<name>.on_failure]` and `[app.<name>.grants]` are optional; every other table's required
keys are below. A document with no listeners, or no apps, is valid — a host that is configured
to do nothing does nothing, visibly.

### 4.2 `[listener.<name>]`

| Key | Type | Required | Meaning |
|---|---|---|---|
| `protocol` | `"sip"` · `"session"` | yes | what connects here: calls, or session-mode apps |
| `bind` | `"<ip>:<port>"` | yes | the local address, an IPv4 or bracketed IPv6 socket address |
| `transport` | `"udp"` · `"tcp"` · `"tls"` · `"ws"` · `"wss"` | `sip` only, yes | per [`sip-transport.md`](sip-transport.md) §4 |
| `advertise` | `"<host>:<port>"` | `sip` only, no | the address to put in `Via`/`Contact` when it differs from `bind` |
| `app` | app name | `sip` only, no | the app every call on this listener reaches |
| `no_app` | status `400`–`699` | `sip` only, see below | the refusal when the listener routes to no app |

**Routing (N6).** A `sip` listener declares exactly one of `app` and `no_app`. Declaring
neither is refused (`unrouted-listener`) — that is the silence N6 forbids. Declaring both is
also refused (`unreachable-no-app`): with `app` present, `no_app` can never apply, and a
declaration that cannot take effect is a lie an operator will one day rely on. An `app` naming
an app the document does not declare is refused (`unknown-app`).

A `session` listener routes nothing: an app arriving there identifies itself with its
`bearer_secret` ([`session-binding.md`](session-binding.md) §1). `transport`, `advertise`,
`app` and `no_app` on one are refused (`key-not-allowed`).

**Open until `A-4`:** whether a `session` listener can be TLS-terminated by the host, and how
its certificate would be named. Until then it is plain WebSocket and belongs behind a reverse
proxy or on a loopback address, which is what the example shows.

### 4.3 `[app.<name>]`

| Key | Type | Required | Meaning |
|---|---|---|---|
| `binding` | `"webhook"` · `"session"` · `"embedded"` · `"realtime"` | yes | which application seam owns the call |
| `url` | absolute `http`/`https` URI | `webhook` only, yes | where events are `POST`ed |
| `signing_secrets` | 1–2 secret names | `webhook` only, yes | the `Sipx-Signature` key, head first |
| `bearer_secret` | secret name | `session` only, yes | what the app presents at upgrade |
| `handler` | non-empty path | `embedded` only, yes | the handler file, relative to the host's handler root |
| `endpoint` | absolute `ws`/`wss` URI without query or fragment | `realtime` only, yes | the realtime WebSocket endpoint before model selection |
| `model` | non-empty string | `realtime` only, yes | the model selected in the endpoint query |
| `instructions` | non-empty string | `realtime` only, yes | the instructions sent in the session update |
| `api_key_secret` | secret name | `realtime` only, yes | the bearer resolved at startup per [`openai-realtime.md`](openai-realtime.md) §2 |

`signing_secrets` is a list because [`webhook-binding.md`](webhook-binding.md) §3 requires key
rotation to be expressible: the head is the key the host signs with, and a second entry is the
key still honoured during a rotation window. **What the host does with the second entry is
`A-2`'s** — this document's job is that two keys can be named at once, so a rotation is a
reload rather than a restart.

**Open until `A-6`:** what an `embedded` app's `handler` path is resolved against, and whether
a handler may name more than one file. The key exists so the binding is expressible in v1's
schema; its resolution is the engine binding's.

A `realtime` binding is a terminal call application rather than a transport for the
`sipx.app.v1` document contract: the host answers the routed SIP call, enables encoded relay on
its negotiated PCMU or PCMA media session, and gives that call leg to the bridge specified by
[`openai-realtime.md`](openai-realtime.md). The endpoint carries no query because this schema gives
model selection one owner, `model`; the bridge appends it exactly once. `ws` remains useful only
for a loopback stand-in because the WSS client refuses cleartext elsewhere.

```toml
[app.agent]
binding        = "realtime"
endpoint       = "wss://api.openai.com/v1/realtime"
model          = "gpt-realtime-2.1"
instructions   = "answer briefly"
api_key_secret = "openai-api-key"
```

### 4.4 Secrets (N7)

A secret **name** matches `[a-z][a-z0-9._-]{0,63}`: lowercase, starts with a letter, at most 64
characters. Nothing else in this schema concerns secrets — there is no key whose value is key
material, so there is nothing to redact before committing the file.

The grammar is narrow on purpose. Base64 and PEM both use characters it excludes (`+`, `/`,
`=`, uppercase, leading `-`), so the common accident — pasting the key where the name goes —
is refused (`not-a-secret-name`) rather than committed. It is a grammar, not a secret detector:
the guarantee is that no field *means* material, and the grammar is what stops the guarantee
from resting on the author's care.

**Resolution is outside this file and outside this spec.** A name that does not resolve is a
*startup* failure with its own message, not a document error: whether a document is valid must
not depend on the environment it is read in, or the same file would be committable on one host
and not on another.

### 4.5 `[app.<name>.on_failure]` (N4)

Exactly [`app-contract.md`](app-contract.md) §9.2, restated only as syntax:

| Key | Type | Default |
|---|---|---|
| `timeout_ms` | integer, `1`–`600000` | `2000` |
| `on_timeout` | action | `"continue"` |
| `on_5xx` | action | `"continue"` |
| `on_unreachable` | action | `"continue"` |
| `on_4xx` | action | `{ reject = 500 }` |

An action is `"continue"`, `"hangup"`, or `{ reject = <status> }` with a status in `400`–`699`.
An absent table means all five defaults; an absent key means that key's default. The asymmetry
of the defaults is the contract's and is not re-argued here.

The table's keys are the four knob names of §9.2 plus `timeout_ms`, and the host takes them
from the contract's own enumeration rather than from a second list — a fifth knob in §9.2 is a
failing test here, not a silently unsupported key.

### 4.6 `[app.<name>.grants]` (N5)

| Key | Type | Default | Grants |
|---|---|---|---|
| `play_roots` | list of paths | `[]` | `play` may name a file under one of these roots; with none, inline audio only ([`app-contract.md`](app-contract.md) §6.5) |
| `dial_headers` | list of field names | `[]` | `dial` may set these header fields, and no others (§6.5) |
| `originate` | boolean | `false` | the app may place outbound calls (§8) |

A grant is a capability of the *host*, exercised on the app's behalf; the app never receives
one directly. There is no grant here that is not the enforcement point of a contract verb —
when a capability has no verb, the contract grows first ([design](../designs/app-host.md),
ground rule 1).

**Deliberately absent:** any grant for `record`. The design lists media file lifecycle as an
open capability question, and a key that promised somewhere to write would be answering it
here by accident.

## 5. Refusals

A document is accepted whole or refused whole. Every refusal carries a machine-readable code
and a message; the codes are a closed set, so a vector can name one and an operator can search
for one.

| Code | Refused because |
|---|---|
| `syntax` | a construct outside §2, at a named line |
| `unknown-table` | a table this schema does not define |
| `unknown-key` | a key this schema does not define |
| `key-not-allowed` | a known key where it may not appear |
| `duplicate-table` | the same table opened twice |
| `duplicate-key` | the same key set twice in one table |
| `missing-key` | a required key absent |
| `bad-value` | the right key, a value outside its type or range |
| `not-a-secret-name` | a secret reference that is not a name (§4.4) |
| `unknown-app` | a listener routing to an app that is not declared |
| `unrouted-listener` | a `sip` listener declaring neither `app` nor `no_app` |
| `unreachable-no-app` | a `sip` listener declaring both |
| `topology-changed` | a *reload* changing the listener set or a listener's address (§6) |

## 6. Reload

- **[sipx-app]** A reload is a whole document, read from the same source as the initial load.
  It is parsed and validated in full before anything is applied. On any refusal the running
  configuration is untouched and the reason is reported; on acceptance the new configuration
  replaces the old one in one step (N9).
- **[sipx-app]** A reload may change apps (added, removed, altered), failure semantics, grants,
  secret names, and a `sip` listener's `app`/`no_app`. It may **not** change the set of listener
  names, or any listener's `protocol`, `bind`, `transport` or `advertise` — those are topology,
  frozen for the life of the process, and a reload that changes one is refused with
  `topology-changed` (N10).

  This is not timidity about sockets; it is what makes atomicity true rather than aspirational.
  Rebinding can fail at the operating system, and a reload that has already given up a port it
  cannot get back is exactly the partial application N9 forbids. Changing a listener is a
  restart, and restarts are visible.
- **[sipx-app]** A call is **admitted** under the configuration current at its first event, and
  keeps that app's failure semantics, grants and binding for its whole life (N11). A reload
  never ends a live call, and an app removed by a reload stays alive for the calls already
  admitted to it — until the last one ends, at which point it is gone.
- The management surface a reload is requested *through* is phase 4's and is not specified here;
  a signal and a re-read of the same path satisfies everything above.

## 7. Open: one host, many apps — or one app, many hosts

**Deliberately open.** Whether a v1 host process serves several apps, or serves exactly one with
"many" as an operational layer above it, is decided in **phase 4** ([design](../designs/app-host.md)),
when phases 1–3 have shown what operating this actually needs. The argument is real on both
sides: one process is one thing to run, one configuration to read and one place to look; one
process per app is a blast radius that needs no code to be correct, and makes an app's failure
semantics the operating system's problem rather than the host's.

This schema **must not decide it**, and does not. What it preserves, whichever way phase 4
lands:

1. **The document is valid with one app and with many** — and a one-app document means exactly
   what the same app means inside a many-app document. That is vector HC-29, and it is the
   concrete form of "the schema does not preclude either".
2. **Nothing is per-app that would have to become per-process, and nothing is global that would
   have to become per-app.** Failure semantics, grants and secret names are per-app today, so
   splitting one document into N single-app documents is a partition rather than a rewrite.
   Listeners are the only global, and they are what a split would have to divide — which is why
   a listener names its app rather than an app naming its listeners.
3. **An app's identity is its name**, stable across both shapes, and is what a live call's
   captured policy refers to. A per-process future addresses an app the same way a multi-app
   present does.
4. **No cross-app construct exists.** There is no shared pool, no app inheritance, no "default
   app" a second app could differ from. Each `[app.<name>]` is complete on its own, so a process
   holding one is not holding a fragment.

What phase 4 may still need and does not exist yet: an identity for the *host* itself (a name
in logs, metrics and the management surface), and whatever a supervisor needs to be told about
a process it owns. Both are additive — a top-level table this document does not define — and
neither is invented here on the strength of a guess about which shape wins.

## 8. Vectors

Every row runs in
[`crates/sipx-app/tests/config_vectors.rs`](../../crates/sipx-app/tests/config_vectors.rs); the
documents are the vectors' own text, in
[`crates/sipx-app/src/config/vectors.rs`](../../crates/sipx-app/src/config/vectors.rs). HC-28
and HC-9 run under the `A-7` harness, because what they assert is what happens to a *call*.

| # | Document | Expected | Pins |
|---|---|---|---|
| HC-1 | the reference document of §4.1 | accepted; three listeners, three apps, routes, policies, grants and secret names exactly as declared | N4 N5 N6 N7 |
| HC-2 | a multi-line basic string | `syntax`, at its line | N1 |
| HC-3 | a key before any table header | `syntax`, at its line | N1 |
| HC-4 | `on_5xxx = "hangup"` | `unknown-key` | N2 |
| HC-5 | `[app.reception.retry]` | `unknown-table` | N2 |
| HC-6 | `bind` set twice in one listener | `duplicate-key` | N3 |
| HC-7 | `[app.reception]` opened twice | `duplicate-table` | N3 |
| HC-8 | an app with no `on_failure` table | accepted; §9.2's five defaults exactly | N4 |
| HC-9 | `on_5xx = "hangup"` | accepted; **and a 5xx ends the call** under the harness | N4 |
| HC-10 | `on_4xx = "reject"` (no status) | `bad-value` | N4 |
| HC-11 | `on_5xx = { reject = 1000 }` | `bad-value` | N4 |
| HC-12 | an app with no `grants` table | accepted; nothing granted | N5 |
| HC-13 | all three grants declared | accepted; exactly those | N5 |
| HC-14 | a `sip` listener with neither `app` nor `no_app` | `unrouted-listener` | N6 |
| HC-15 | a `sip` listener with both | `unreachable-no-app` | N6 |
| HC-16 | `app = "typo"` | `unknown-app` | N6 |
| HC-17 | one routed listener, one `no_app` listener | accepted; the first admits its app, the second refuses `503` | N6 |
| HC-18 | the reference document | accepted; every secret is a name, and the text carries no material | N7 |
| HC-19 | a base64 blob where a secret name goes | `not-a-secret-name` | N7 |
| HC-20 | a `webhook` app with no `signing_secrets` | `missing-key` | N8 |
| HC-21 | a `session` app with no `bearer_secret` | `missing-key` | N8 |
| HC-22 | an `embedded` app with no `handler` | `missing-key` | N8 |
| HC-23 | `binding = "carrier-pigeon"` | `bad-value` | N8 |
| HC-24 | reload changing `on_5xx` | accepted; a fresh call gets the new policy | N9 |
| HC-25 | reload of a document that does not parse | `syntax`; the running configuration is unchanged | N9 |
| HC-26 | reload changing a listener's `bind` | `topology-changed`; unchanged | N10 |
| HC-27 | reload changing a listener's `app` | accepted; routing is not topology | N10 |
| HC-28 | a live call, then a reload to `on_5xx = "hangup"` | the live call keeps `continue` (its call survives a 5xx under the harness); the next call hangs up | N11 |
| HC-29 | a one-app document and a many-app document | both accepted; the shared app and listener are identical in each | N12 |
| HC-30 | `app` on a `session` listener | `key-not-allowed` | N2 |
| HC-31 | a complete `realtime` app with no grants table | accepted; endpoint, model and instructions retained, nothing granted, and only `agent-api-key` inventoried | N5 N7 N8 |
| HC-32 | `temperature` under a `realtime` app | `unknown-key` | N2 |
| HC-33 | a `realtime` app with no `endpoint` | `missing-key` | N8 |
| HC-34 | a `realtime` app with no `model` | `missing-key` | N8 |
| HC-35 | a `realtime` app with no `instructions` | `missing-key` | N8 |
| HC-36 | a `realtime` app with no `api_key_secret` | `missing-key` | N8 |
| HC-37 | credential material in `api_key_secret` | `not-a-secret-name` | N7 |

Two properties are checked over the *set* rather than by a row, because they are what stops the
set from silently shrinking: every normative point of §3 is named by at least one vector, and
every refusal code of §5 is produced by at least one vector.
