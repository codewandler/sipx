# SIP authenticated identity and PASSporT

**Status:** implementing (`S-20`) · **Crates:** `sipx-sip`, `sipx-ua`

Normative references: RFC 8224 §§4, 6.1, 6.2, 7 and 8; RFC 8225 §§4–9 and Appendix A;
RFC 7515 §5.1; RFC 7518 §3.4; RFC 6979 §3.2.

## 1. Boundary and policy ownership

`sipx-sip::identity` is sans-I/O. It parses the revised RFC 8224 `Identity` value, derives
canonical identities from SIP URIs, serializes baseline PASSporT deterministically, and signs or
verifies ES256. It reads neither a clock nor a URI. Time is an integer supplied by the caller and
credentials are values supplied by `sipx-ua`.

`sipx-ua::identity` implements the authentication-service and verification-service order. The
application owns every deployment decision the RFC leaves local:

| Type | Owner | Contract |
|---|---|---|
| `Authority` | application | Says whether the authentication service may assert one canonical origin |
| `SigningCredential` | application | ES256 private key, `info` URI, and validity interval; `Debug` never reveals key bytes |
| `CredentialFetcher` | application | Dereferences an `info` URI and validates trust/authority; sipx performs no network fetch |
| `CachedCredentials<F>` | `sipx-ua` | Bounded successful-key cache in front of caller fetcher, keyed by the exact `info` URI |
| `Freshness` | application | Maximum absolute Date/`iat` skew; defaults to RFC 8224's recommended 60 seconds |

The fetch result distinguishes `Unavailable` (no credential could be acquired, response 436) from
`Unsupported` (bytes or a key were acquired but policy, trust, curve, or algorithm rejects them,
response 437). A successful credential includes its validity interval and the ES256 verifying key.
The caller supplies verification time to the fetcher/cache; an entry outside its validity interval
is evicted and reacquired from the same exact URI rather than becoming a permanent cached 437.
The fetcher is responsible for the RFC 8224 §7 credential-system authority check: a signature alone
never establishes that its key may speak for the origin.

## 2. Typed wire forms

The typed `Identity` value is:

| Field | Rule |
|---|---|
| `digest` | One full `header.payload.signature` PASSporT or compact `..signature`, using the grammar's base64 characters and dots |
| `info` | Required absolute URI enclosed in `<` and `>` |
| `alg` | Optional token; absent means `ES256`; any other value is unsupported |
| `ppt` | Optional token; baseline sipx supports no extension value |
| extensions | Parsed and retained as generic parameters, never treated as signed claims |

Malformed grammar is not a missing header. Multiple `Identity` rows remain individually typed and
are verified in wire order. An unsupported `ppt` makes that row unusable before credential lookup,
as RFC 8224 §6.2 Step 1 and RFC 8225 §8.1 require.

Baseline full PASSporT has exactly these deterministic JSON members:

```text
header  = {"alg":"ES256","typ":"passport","x5u":"<info>"}
payload = {"dest":{<kind>:["<destination>"]},"iat":<seconds>,"orig":{<kind>:"<origin>"}}
```

Keys at every object level are lexicographically ordered, JSON contains no insignificant
whitespace, `iat` is an integer, and base64url is unpadded. No `attest`, `origid`, or other profile
claim is emitted or accepted as baseline RFC 8225. ES256 is P-256 with SHA-256 and its JWS signature
is exactly 64 bytes, big-endian `R || S` (RFC 7518 §3.4). Signing is deterministic RFC 6979 ECDSA.

## 3. Canonical identity

For `tel:` and a SIP/SIPS URI explicitly carrying `user=phone`, remove every character except
digits, `*`, and `#`; a result with none is invalid. A deployment needing country-code or dial-plan
transformation performs it before constructing the request, because RFC 8224 §8.3 makes that local
policy.

Every other origin or destination is a canonical SIP/SIPS AoR: require a user and host; retain only
the scheme, decoded user, and host; discard password, port, URI parameters, and URI headers; lowercase
scheme, user, and host. An unsupported scheme or non-UTF-8 identity is invalid. Signing and verifying
run the same function over the From and To addr-specs; a full token's `orig` and `dest` must equal
those results, so token claims cannot replace the SIP identities (§6.2.4).

## 4. Authentication service

`sign(request, now)` runs in this order:

| Step | Failure | Action |
|---|---|---|
| derive From and To | malformed/missing identity | typed input error; request unchanged |
| ask `Authority` about From | false | `NotAuthoritative`; add no `Identity` |
| inspect Date | absent | format `now` as SIP GMT and add it |
| inspect Date | malformed or absolute skew exceeds `Freshness` | `StaleDate`; add no `Identity` |
| check credential validity | Date or `now` outside interval | `CredentialNotValid`; add no `Identity` |
| construct and sign | crypto/serialization failure | typed signing error |
| finish | — | append a full baseline `Identity` with `info`; retain any pre-existing rows |

`iat` equals the accepted Date timestamp. The full form is the baseline output because it is
self-describing, permits strict claim comparison, and is what the RFC-owned Appendix A vector tests.

## 5. Verification service

`verify(request, now, required, source)` evaluates rows in wire order. Any valid trusted row succeeds;
only when no row succeeds does the aggregate error become a SIP response.

| RFC 8224 order | Check | Per-row result |
|---|---|---|
| 1 | parse row; reject any `ppt`; require/default `alg=ES256` | unusable/invalid without fetching |
| 2 | derive canonical From and To from SIP | invalid PASSporT |
| 3 | acquire the exact `info` credential through `CredentialFetcher`/cache | unavailable or unsupported credential |
| 4 | require and parse Date; enforce absolute Date skew and credential validity | stale Date or unsupported credential |
| 5 | parse full PASSporT, require exact baseline header/claims, compare `x5u`, `orig`, `dest`, and `iat`, then verify ES256 | valid or invalid PASSporT |

Aggregate response mapping preserves the most informative stage reached across all rows:

| Outcome after all rows | Status | Reason |
|---|---:|---|
| no row, or every row has unsupported `ppt`, and identity is required | 428 | `Use Identity Header` |
| every usable row's credential acquisition failed | 436 | `Bad Identity Info` |
| a credential was acquired but every such credential is unsupported/untrusted/outside validity | 437 | `Unsupported Credential` |
| an otherwise usable row has a Date/`iat` outside freshness | 403 | `Stale Date` |
| no supported row has valid grammar, claims, and signature | 438 | `Invalid Identity Header` |
| identity is optional and no usable row exists | success with `Unverified` |

Step order is observable: unsupported `ppt` never invokes the fetcher; a 436 is not returned for a
row whose Date would already be stale only because credential acquisition precedes freshness; and a
bad signature that reached Step 5 is 438 rather than generic 400.

## 6. Vectors

| ID | Input | Required result |
|---|---|---|
| P1 | RFC 8225 Appendix A header/payload/private-key sample plus RFC 7515 Appendix A.3 public verification vector | deterministic signing reproduces the PASSporT signing input and self-verifies; the independent JWS signature verifies with the published P-256 point |
| P2 | valid token with one signature octet changed | `a_request_whose_identity_signature_does_not_verify_is_refused_with_438` |
| P3 | `Identity` with `ppt=unknown` and a fetcher that records calls | 428 when required; zero fetch calls |
| P4 | no `alg` parameter, otherwise valid ES256 token | verify successfully; missing `alg` defaults to ES256 |
| P5 | fetcher returns unavailable / unsupported | 436 / 437 respectively |
| P6 | Date 61 seconds behind `now` under the default policy | 403; signature verification is not attempted |
| P7 | authentication service lacks authority | request gains neither Date nor Identity |

The Appendix A key material is an RFC-owned interoperability oracle, not a sipx round trip. P2 uses
the same request/key but corrupts the signature, which proves the named 438 path reaches the actual
cryptographic verifier.
