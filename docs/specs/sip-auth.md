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

## 6. Server verifiers and bounded replay state

`auth::DigestVerifier` holds private username, realm, algorithm and base-HA1 fields. The
base-HA1 is `H(username:realm:password)` (RFC 7616 §3.4.2), including for session algorithms;
the nonce-dependent session hash is computed for each request. This value is credential-equivalent,
not a password-hardening hash: anyone holding it can impersonate the user in its protection space.
No Debug, Display or error output may disclose it. `derive` accepts credentials; `from_ha1`
imports exactly 32 hexadecimal digits for MD5 or 64 for SHA-256/SHA-512-256, normalizing case.
Both reject empty usernames, controls and a colon in the username. An empty realm remains an
exact binding for compatibility with the existing password API; callers are responsible for
selecting a compliant globally unique realm (RFC 3261 §22.1), and an empty realm is not a
production recommendation. Import binds the
caller's metadata; no hash can prove which metadata originally produced imported bytes.

`expose_ha1` explicitly borrows the canonical lowercase hexadecimal base-HA1 for an application
that must persist a verifier. Store it with the username, realm and exact algorithm getters,
protecting the whole bound record as credential-equivalent secret material. Export does not imply
safe logging or ordinary password-hash security. No implicit serialization, Debug, Display or
error representation exposes it. A derived verifier can be exported, the plaintext credentials
and original verifier discarded, then the bound record imported without duplicating Digest
arithmetic in the application. Storage encryption, access control and replication belong to the
caller; exporting the verifier does not export this authenticator's replay window.

`Authenticator::verify_with_verifier[_at]` and existing password methods share this order:
validate algorithm, declared realm and verifier binding; require qop=auth, a nonzero count and
nonempty client nonce; authenticate the server nonce; compare the response in constant time;
check expiry; then record replay state. Unsupported named algorithms fail parsing; only an absent
algorithm defaults to MD5. The complete authorization parameter list must parse before field
lookup: malformed quoting/escaping, invalid tokens, missing values/separators, duplicate names,
trailing junk and invalid UTF-8 are rejected rather than interpreted as absent metadata. Parameter
names and the Digest scheme are case-insensitive; valid quoted escapes and extension parameters
remain supported. Wrong credentials on an expired nonce are rejected, never `Stale`.
The caller must compare the presented URI with the actual request URI before trusting either API.
This API does not provide userhash, distributed replay storage or request-level authorization.

Replay state keys the tuple (nonce, username, client nonce), using a length-framed SHA-256 key
so retained input lengths cannot grow storage. Each key stores its issue time, highest count and
response. A greater count advances it; an identical highest count/response is a retransmission;
a lower count or changed response at the highest count is `Reason::Replay`. Identical Digest
retransmissions cannot be distinguished from captured identical requests; the SIP transaction
layer must avoid executing a retransmitted request twice.

At most 4096 identities are retained. Only expired entries may be removed. New identities at
capacity return `Reason::ReplayCapacity`; retained identities continue to work. Capacity refusal
is not `Stale`, and must not prompt an unbounded immediate retry loop. After expiry, capacity is
available again. The supplied clock must not move backwards, and lifetime is configured before
verification begins; replay state is local to one authenticator and does not survive its restart.

| Vector | Required result |
|---|---|
| RFC 2617 §3.5 base-HA1 `939e7578ed9e3c518a452acee763bce9`, user `Mufasa`, realm `testrealm@host.com`, GET `/dir/index.html`, nonce `dcd98b7102dd2f0e8b11d0f600bfb0c093`, cnonce `0a4f113b`, nc=1 | response `6629fae49393a05397450978507c4ef1` |
| All six supported algorithms, derived and imported verifiers | same response as the password API |
| Changed declared realm, username, algorithm, method or credential | rejected before expiry/replay success |
| Wrong hex length/encoding, empty user/control metadata, colon in user | typed import/derive error without input echo |
| 4096 fresh identities then another, before nonce expiry | first 4096 authenticate; new identity returns ReplayCapacity; old lower-count requests still Replay |
| Same server nonce with distinct users or cnonces | independent replay counters |
| Correct/incorrect responses after expiry | Stale/Mismatch respectively; newly issued nonce authenticates |
