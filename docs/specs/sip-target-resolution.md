# Spec: bounded SIP target resolution

**Status:** normative · **Crate:** `sipx-transport` · **Stories:** T-38, T-39 · **Design:**
[bounded endpoint resolution](../designs/endpoint-resolution.md)

## 1. Normative references

- RFC 3263 §4.1–§4.3 and §8 — selecting a SIP transport and turning a SIP or SIPS URI into an
  ordered set of transport, port, and address tuples.
- RFC 2782 — SRV priority and weighted ordering, including the special meaning of target `.`.
- RFC 7984 §3.1 and §4 — looking up every supported address family and preserving SRV-major,
  address-selection-minor ordering.
- RFC 5922 §4 and §7.3 — authenticating the selected TLS peer against the original domain supplied
  to RFC 3263, not an address or SRV target produced by DNS.
- [`sip-tls.md`](sip-tls.md) — certificate validation, verification-name syntax and no-downgrade
  policy.
- [`sip-transport.md`](sip-transport.md) — transaction ownership, connection pooling and transport
  failure delivery.

The RFCs define selection and ordering. The finite resource limits, typed failure vocabulary,
command-line precedence and cancellation contract below are sipx policy.

## 2. Boundary

Resolution determines where one outbound SIP transaction may be sent. It never rewrites the
Request-URI. The operation has two deliberately separate halves:

1. **Selection is pure.** A URI, explicit command policy, finite resolver answers and an injected
   random stream produce an ordered finite `Candidate` list or a typed refusal. It reads no clock,
   opens no socket and performs no DNS query.
2. **The adapter performs I/O.** `sipx-transport` obtains resolver answers, applies deadlines and
   cache policy, then attempts candidates serially. It owns every DNS and connection future it
   starts and cancels it when the parent operation is cancelled.

No part of this subsystem adds I/O, a clock read or an async runtime to `sipx-sip` or `sipx-sdp`.
Those crates continue to receive only values and fired-timer inputs.

Which nameservers the adapter asks is configuration, not policy: the host's own unless a command
adapter is told otherwise, in which case that source MUST be a value the operator can state — an
address, optionally with a port — and MUST be refused rather than ignored when it cannot be read.
An override that silently fell back would answer from a zone nobody asked about, which is
indistinguishable from the failure it was set to diagnose. A literal target consults no resolver
and therefore no configuration.

## 3. Values

The public boundary is equivalent to these types. Rust names may differ only when the same
information and invariants remain explicit.

```text
ResolutionInput {
    uri: Uri,
    next_hop: Optional<HostAndOptionalPort>,
    command_transport: Optional<Transport>,
    verification_override: Optional<VerifiedDnsName>,
}

ResolverAnswers {
    naptr: Answer<NaptrRecord>,
    srv: Map<QueryName, Answer<SrvRecord>>,
    addresses: Map<HostName, Answer<OrderedIpAddress>>,
}

Answer<T> = Records(List<T>) | Negative | Unavailable

Candidate {
    address: SocketAddress,
    transport: Transport,
    service_identity: Optional<VerificationIdentity>,
    source: Literal | ExplicitPort | Srv | AddressFallback,
}
```

`VerificationIdentity` is either a validated DNS name or an IP literal accepted by the TLS
boundary. `next_hop` is the explicit deployment override exposed by commands such as `--target`.
It changes the host and optional port resolved, but not the Request-URI or secure service identity.
An IP literal in `next_hop` is therefore a no-DNS route to the original URI authority. A named next hop
is resolved by the same rules as the URI host, with its explicit port taking precedence over SRV.

`Negative` means an authoritative empty answer, including an SRV target of `.`. `Unavailable`
means no trustworthy answer was obtained. They are never collapsed before failure classification:
a negative answer may be cached; an unavailable lookup may not.

An address list is already ordered by the adapter's RFC 6724 destination-selection policy. The
pure selector preserves that order inside one host. It never interleaves addresses belonging to
different SRV targets.

## 4. Input precedence and transport policy

Inputs are resolved in this order:

1. Parse the URI and optional next-hop override. A missing host, zero explicit port, malformed DNS
   name or unsupported transport is `InvalidInput` before I/O.
2. The URI scheme sets the security floor. A SIPS URI permits secure transports only. No later
   record, fallback or connection error can lower that floor.
3. A URI `transport` parameter and an explicit command transport are two spellings of one choice.
   If both are present, their effective transports must agree; otherwise resolution returns
   `ConflictingTransport`. For SIPS, `transport=tcp` means TLS over TCP. A clear command transport
   for SIPS is `SecureTransportRequired`.
4. The next-hop host wins over the URI host for address discovery. An explicit next-hop port wins
   over a URI port; otherwise the URI port is used. Any explicit port skips NAPTR and SRV.
5. `verification_override`, when present, is validated by the TLS verification-name parser before
   lookup and becomes the secure `service_identity`. Without it, the original URI hostname is the
   identity. The next-hop name, NAPTR replacement, SRV target and selected IP never replace it.

An IP-literal URI or next hop emits one candidate immediately, using the explicit port or the
selected transport's default. A named host with an explicit port performs only address lookup.
Both paths preserve the selected transport and secure identity.

## 5. Named-host selection

For a named host with no explicit port:

1. If transport was selected explicitly, skip NAPTR and query only that transport's SRV owner.
2. Otherwise query NAPTR. Discard malformed records, unsupported services, non-`S` records and
   every clear service for SIPS. Order usable records by ascending order and preference. Each
   usable replacement names an SRV query.
3. If the NAPTR answer is negative or leaves no usable record, query the conventional SRV owners
   for supported transports. SIPS queries secure owners only. A SIP URI may retain secure owners;
   failure of a secure candidate never authorizes a clear candidate for SIPS.
4. For each positive SRV answer, discard target `.`, zero ports, malformed targets and records
   outside the configured record bound. Order remaining records by ascending priority. Within one
   priority, apply RFC 2782's weighted removal using the injected random stream: zero-weight rows
   precede non-zero rows when the running sums are built, and each draw is inclusive of zero and
   the total.
5. Resolve every ordered SRV target into all supported address families. Preserve the complete
   address order for that target before moving to the next SRV target.
6. If no usable SRV candidate exists, resolve the original discovery host and use the selected
   transport's default port. This is the RFC 3263 address fallback; it does not run after an SRV
   target of `.`, which is an explicit statement that the service is unavailable.
7. Remove exact duplicate `(address, transport, service_identity)` candidates without changing the
   first occurrence. Stop at `max_candidates`; reaching a configured bound is observable as
   `LimitExceeded`, never silent truncation presented as a complete answer.

The pure function returns the whole finite list. Connection attempts consume it serially. A later
candidate cannot start until the earlier attempt has either produced a usable transport, failed,
or been terminated. Parallel duplicate SIP requests are outside this contract.

## 6. Bounds and state table

Every resolver instance has a validated `ResolutionLimits`. Zero is invalid for every count and
duration except a test-supplied cache capacity of zero, which disables caching. The shipped phone
uses these defaults:

| Limit | Default | Scope |
|---|---:|---|
| DNS lookups | 32 | one resolution |
| NAPTR records | 16 | one answer |
| SRV records | 32 | one answer |
| addresses | 16 | one target name |
| candidates | 64 | one resolution |
| connection attempts | 16 | one outbound operation |
| lookup deadline | 2 s | one DNS question |
| resolution deadline | 8 s | all questions and selection |
| connection-attempt deadline | 5 s | one connection-oriented candidate |
| operation deadline | 20 s | resolution plus connection attempts; SIP response time is separate |
| cache entries | 1,024 | all positive and negative DNS entries in one resolver |
| maximum retained TTL | 1 h | one cache entry |

The overall deadline always wins over a larger remaining per-step duration. Record and candidate
bounds are checked before allocation or append. An answer containing more than its bound is a
failure; it is not partly accepted. `max_connection_attempts` may be smaller than
`max_candidates`, in which case exhausting the attempt budget is `LimitExceeded` and the untried
tail remains diagnostic evidence.

| State | Input or event | Action | Next state / bound |
|---|---|---|---|
| `Validate` | resolution input | validate URI, names, ports, transport agreement and limits | `Literal`, `Naptr`, `Srv` or `Address`; no I/O |
| `Literal` | IP literal | emit one candidate | `Connect`; zero lookups |
| `Naptr` | named, implicit transport | ask once, classify answer, validate at most 16 records | `Srv`; lookup and overall deadlines |
| `Srv` | selected SRV owner | ask each distinct owner once, at most 32 total lookups and 32 rows per answer | `Address` or address fallback |
| `Address` | one ordered host | ask for all supported families and validate at most 16 addresses | `Connect` or typed failure |
| `Connect` | next candidate | start one owned attempt, never a parallel SIP request | `Done`, next `Connect`, or failure; at most 16 attempts |
| `Done` | transport usable | return selected target and retain URI service identity | terminal |
| any active state | overall deadline | cancel active child and report the stage | `DeadlineExceeded` |
| any active state | caller cancellation | cancel active child and await its termination | `Cancelled` |
| any active state | count bound | report limit name, configured maximum and observed count | `LimitExceeded` |

UDP has no connection handshake. Selecting a UDP candidate completes the connection-attempt phase
immediately; its transaction timeout and RFC 3263 retry behavior remain transport-transaction
policy. Connection-attempt deadlines apply to TCP, TLS, WS and WSS establishment. The operation
deadline does not replace or extend a call command's separate answer deadline.

## 7. Cache and cancellation

The DNS adapter may cache positive and authoritative negative answers no longer than the lesser of
their DNS TTL and `maximum_retained_ttl`. It never caches `Unavailable`, `DeadlineExceeded` or
cancelled work. The cache's entry count includes every record type and address family. Before
insertion it removes expired entries, then evicts the least-recently-used entry with a stable
query-name/record-type tie-break. Cache lookup and eviction are bounded by the configured capacity;
the adapter does not create an unbounded maintenance task.

Resolution and candidate connection are child futures of the one calling operation. The adapter
does not detach or spawn per-record work. Dropping or cancelling the parent cancels the active DNS
query or connection attempt, waits for owned cleanup through the transport's ordinary task join,
and returns `Cancelled`. No cache entry is published from a cancelled lookup.

Tests inject `ResolverAnswers`, an ordered address-selection result, a deterministic RFC 2782
random stream, and fired deadlines. They do not query public DNS or sleep until a wall clock happens
to pass. Adapter tests use a finite local DNS fixture only to prove the I/O boundary.

## 8. Typed failures

```text
InvalidInput { field, reason }
ConflictingTransport { uri, command }
SecureTransportRequired { requested }
NegativeAnswer { query, record_type }
LookupUnavailable { query, record_type }
NoUsableCandidate { authority }
LimitExceeded { limit, maximum, observed }
DeadlineExceeded { stage, elapsed }
ConnectionFailed { attempted, last_error }
Cancelled { stage }
```

`NoUsableCandidate` is used only after authoritative answers were obtained and none could produce
a candidate. `LookupUnavailable` means the resolver could not establish the answer. A deadline is
not rewritten as either. `ConnectionFailed` retains how many ordered candidates were attempted and
the final concrete transport error. This vocabulary lets command adapters map invalid policy to a
usage exit, deadline to timeout, and DNS/connection failure to a non-zero operational exit without
parsing an error string.

## 9. Deterministic vectors

In these vectors `id` is the secure service identity, `q` is the ordered query list and `=>` is the
ordered candidate result. Unmentioned answer maps are authoritative negative answers.

| # | Input and injected answers | Expected |
|---|---|---|
| R1 | `sip:alice@192.0.2.10`, no explicit policy | `q=[]`; `=> udp://192.0.2.10:5060`, no identity |
| R2 | `sips:alice@[2001:db8::10]`, no explicit policy | `q=[]`; `=> tls://[2001:db8::10]:5061`, identity is the literal |
| R3 | `sip:alice@voice.example:5088`; ordered addresses `[2001:db8::20, 192.0.2.20]` | `q=[ADDR voice.example]`; two UDP candidates at port 5088 in the injected family order; no NAPTR/SRV query |
| R4 | `sip:alice@voice.example;transport=tcp` plus command transport UDP | `ConflictingTransport`; no query |
| R5 | `sip:alice@voice.example`; NAPTR contains unsupported and malformed rows, then `SIP+D2T -> _sip._tcp.voice.example`; SRV rows `(10,0,5070,a.example)`, `(10,10,5070,b.example)`; seed draws `7`; `b.example -> [192.0.2.31, 2001:db8::31]`, `a.example -> [192.0.2.30]` | unsupported rows discarded; TCP candidates for `b` remain adjacent and precede `a`; SRV target groups are never interleaved |
| R6 | `sips:alice@secure.example`; NAPTR offers `SIP+D2U` and `SIPS+D2T`; secure SRV target is `edge.example -> 192.0.2.40` | UDP row discarded; `=> tls://192.0.2.40:5061`; `id=secure.example`, not `edge.example` |
| R7 | R6 plus validated verification override `tenant.example` | selected address unchanged; `id=tenant.example` |
| R8 | named host; NAPTR and SRV are negative; address answer is empty | `NoUsableCandidate`, distinct from an unavailable lookup |
| R9 | named host; active SRV lookup receives its fired lookup deadline | `DeadlineExceeded { stage: Srv }`; no address lookup, cache insertion or connection attempt |
| R10 | named host; cancellation while address lookup is active | active lookup is cancelled and joined; `Cancelled { stage: Address }`; zero detached tasks |
| R11 | one SRV answer carries 33 rows with `max_srv_records=32` | `LimitExceeded { limit: SrvRecords, maximum: 32, observed: 33 }`; no partial candidate list |
| R12 | `sips:alice@secure.example` resolves only clear services and addresses | `NoUsableCandidate`; no clear connection attempt |

R3 and R5 pin mixed-family behavior at the value boundary. The adapter proof separately requires
both A and AAAA questions on a dual-stack resolver and passes the resulting per-host order into the
same vectors.
