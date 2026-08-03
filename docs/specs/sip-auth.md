# SIP digest authentication

**Status:** implemented for registration and outbound calls (`S-16`, `S-28`) · **Crates:**
`sipx-sip`, `sipx-ua`, `sipx-call`, `sipx-cli`

Normative references: RFC 3261 §8.1.3.5 (retrying a challenged request), §22.2 (401 and
Authorization), §22.3 (407 and Proxy-Authorization), §17.1.1.2 (branch uniqueness); RFC 7616
§3.3 (challenge), §3.4 (digest response), §3.4.3 (nonce count), §3.5 (credentials); RFC 8760
§2.4 (multiple algorithms) and §3 (downgrade risk).

## 1. Boundary and ownership

Digest parsing and arithmetic are pure SIP protocol operations and live in `sipx-sip::auth`.
Neither registration nor call setup implements a second formula. `sipx-ua` re-exports those types
for compatibility and owns registrar retry state; `sipx-call` owns INVITE retry state. Random client
nonces are supplied by those I/O-facing crates, so the sans-I/O core reads no operating-system
entropy.

| Type | Owner | Purpose |
|---|---|---|
| `auth::Challenge` | `sipx-sip` | Parsed realm, nonce, algorithm, qop, stale and proxy/direct kind |
| `auth::Credentials` | application | Username and password; never formatted with `Debug` password output or logged |
| `DialOptions::credentials` | one outbound call attempt | Optional credentials selected by the application |
| nonce-use pair | registration or call retry driver | Last nonce and the next RFC 7616 nonce count |

The CLI takes the username from the `--from` URI and the password from `--password`, with
`SIPX_PASSWORD` as the preferred source. A password in argv is visible to other local processes;
the flag is a convenience, not the documented secure route. Capture redaction remains below this
layer and authentication code never logs a credential or rendered authorization field.

## 2. Challenge selection and header mapping

For a final 401, parse every `WWW-Authenticate` value as a direct challenge and answer the selected
one in `Authorization`. For a final 407, parse every `Proxy-Authenticate` value as a proxy
challenge and answer it in `Proxy-Authorization`. A non-Digest scheme, unsupported algorithm, or
`auth-int`-only quality-of-protection offer is not guessed; if no supported challenge remains, the
original 401/407 is surfaced as a rejection.

When several supported challenges are present, sipx selects the strongest algorithm. This is RFC
8760 §2.4's permitted local policy and removes the header-order downgrade described in §3. Ties
retain wire order.

## 3. Outbound INVITE retry state

`dial` and `dial_once` use the following bounded state. A retry never becomes an unbounded response
loop.

| State | Final response | Action |
|---|---|---|
| `Initial` | 401/407, no credentials | return `Rejected` with that status immediately |
| `Initial` | supported 401/407, credentials present | remember challenge; retry authenticated |
| `Authenticated` | 401/407 without `stale=true` | credentials failed; return `Rejected` immediately |
| `Authenticated` | 401/407 with fresh stale nonce | replace challenge and retry once |
| any | second stale challenge | return `Rejected`; never loop |
| any | 422 with usable `Min-SE` under `dial` | raise interval and retry once, retaining authentication |
| any | 422 under `dial_once`, or a second 422 | return `IntervalTooBrief` |
| any | 2xx | establish the dialog and media normally |
| any | other final | return `Rejected` unchanged |

The authentication and 422 budgets are independent because a proxy can challenge the first INVITE
and the UAS can then counter-offer a session interval. Every authenticated request increments the
nonce count for its nonce; a fresh nonce resets it to one.

## 4. Identity across a retry

RFC 3261 §8.1.3.5 requires a challenged request to be re-originated with these properties:

| Field | Retry rule |
|---|---|
| Request-URI, To, From tag, Call-ID | unchanged |
| CSeq number | increment by one; method remains INVITE |
| top Via branch | fresh transaction branch (§17.1.1.2) |
| Authorization | digest covers the retried request's method and Request-URI |
| Route, Contact, Supported, Allow, session and media headers | rebuilt from the same options; fresh per-attempt media resources may change the SDP port |

A non-2xx response is acknowledged by the transaction layer before the retry is sent. Reusing its
branch would merge two client transactions; changing its Call-ID or From tag would create a second
call rather than answer the challenge.

## 5. Byte-level vectors

The request body is abbreviated as the identical byte string `SDP`.

| Vector | Exchange | Required result |
|---|---|---|
| A1 | `INVITE`, CSeq `1 INVITE`, branch `z9hG4bK-one` → `407` with `Proxy-Authenticate: Digest realm="edge", nonce="n", algorithm=SHA-256, qop="auth"` | retry has CSeq `2 INVITE`, a branch other than `one`, `Proxy-Authorization`, the same Call-ID/From tag and an SDP offer, then connects on 200 |
| A2 | same with `401` and `WWW-Authenticate` | retry uses `Authorization`, never `Proxy-Authorization` |
| A3 | A1 with no credentials | return rejection 407; send no second INVITE |
| A4 | authenticated retry receives another non-stale 401/407 | return rejection after two INVITEs; send no third |
| A5 | authenticated retry receives `stale=true` with nonce `n2` | one final retry uses `nc=00000001` for `n2` |

`a_call_challenged_by_a_proxy_retries_with_credentials_and_connects` is A1 end to end and also
checks the stable identity, incremented CSeq, fresh branch and digest result. CLI coverage drives the
same path through `sipx dial --password` and verifies a missing/wrong credential maps directly to
exit 4 (`Unauthorized`), never to a timeout.
