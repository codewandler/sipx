# Comparative signalling-load profile

## 1. Scope and requirements language

This specification defines `sipx.comparative-load.v1`: one finite, signalling-only endpoint
workload, the process-supervision contract around it, and the evidence required to report a result.
It deliberately does not define a product ranking. A result is evidence about one immutable build,
direction, machine and profile execution.

The key words **MUST**, **MUST NOT**, **SHOULD** and **MAY** are used as requirements. SIP behavior is
defined by RFC 3261, particularly §§8, 12, 13, 17 and 20. The profile adds measurement rules; it
does not replace SIP transaction or dialog rules.

The v1 workload excludes SDP, RTP, authentication, DNS, TLS, connection reuse and registration. UDP
loopback is the only transport. Those features need separate profiles because their setup and
resource costs are not interchangeable with an INVITE transaction.

## 2. Roles and immutable inputs

Each execution has exactly two roles:

- the **driver** offers calls, validates responses, sends ACK and BYE, and records dialog latency;
- the **responder** validates requests, answers INVITE, validates ACK and answers or originates BYE.

Both directions MUST be measured eventually: each selected endpoint build acts once as driver and
once as responder. A single direction is a valid raw run and is not a complete comparison result.

Before a process starts, the run manifest MUST fix:

| Input | Requirement |
|---|---|
| `run_id` | 32 lowercase hexadecimal digits, unique to the execution |
| `seed` | unsigned 64-bit integer; all probabilistic choices derive only from it |
| `direction` | index `0` or `1`, two distinct endpoint IDs and the driver/responder assignment |
| builds | immutable revision and SHA-256 of each executable or source archive |
| command | argument vector, working directory and non-secret environment key names |
| machine | OS, architecture, logical CPUs, memory bytes and clock source |
| ceiling | positive integer calls/second used to derive the six fixed rates |
| limits | positive maximum active dialogs and descriptor/task/log/event bounds |

Credentials are not part of this profile. Commands, identifiers, pins and evidence for a particular
endpoint live under `docs/comparison/`; generic runner code and this specification name no subject.

## 3. One measured dialog

### 3.1 Identifiers

For zero-based dialog index `n`, the driver derives these printable ASCII values:

```text
Call-ID: cl-<run_id>-<n>@driver.invalid
From tag: f-<first-16-hex(H(seed || run_id || n || "from"))>
INVITE branch: z9hG4bK-i-<first-20-hex(H(seed || run_id || n || "invite"))>
BYE branch: z9hG4bK-b-<first-20-hex(H(seed || run_id || n || "bye"))>
```

`H` is SHA-256 over each UTF-8 field separated by one zero byte; integers are lowercase decimal.
The responder creates a non-empty To tag from the same seed/run/index plus `"to"`. A branch, tag or
Call-ID MUST NOT repeat within a run. The Request-URI and sent-by host/port come from the readiness
record, never from a response body or display name.

### 3.2 Byte-level flow

Whitespace shown below is exact: one SP between tokens, CRLF line endings, no folded fields, and an
empty body. `<driver-via>`, `<driver-uri>`, `<responder-uri>` and `<target>` are bounded values fixed
before admission. Header order is stable for reproducibility but receivers MUST apply RFC 3261
semantics rather than depend on it.

```text
INVITE sip:load@<target> SIP/2.0\r\n
Via: SIP/2.0/UDP <driver-via>;rport;branch=<invite-branch>\r\n
Max-Forwards: 70\r\n
From: <sip:driver@<driver-uri>>;tag=<from-tag>\r\n
To: <sip:load@<target>>\r\n
Call-ID: <call-id>\r\n
CSeq: 1 INVITE\r\n
Contact: <sip:driver@<driver-uri>>\r\n
Content-Length: 0\r\n
\r\n
```

The responder MAY send exactly one `100 Trying`. The run manifest fixes this to on or off before
startup. The successful final response is:

```text
SIP/2.0 200 OK\r\n
Via: <copied top Via with required received/rport processing>\r\n
From: <copied From>\r\n
To: <copied To>;tag=<to-tag>\r\n
Call-ID: <copied Call-ID>\r\n
CSeq: 1 INVITE\r\n
Contact: <sip:load@<responder-uri>>\r\n
Content-Length: 0\r\n
\r\n
```

The driver validates status, Via branch, Call-ID, both tags, CSeq and Contact before counting the
dialog established. It then sends ACK with CSeq `1 ACK`, the dialog target and route rules learned
from the response, and a fresh top Via branch. The responder must match that ACK to the accepted
dialog. The driver immediately sends an in-dialog BYE with CSeq `2 BYE` and the deterministic BYE
branch. The responder validates dialog identifiers and monotonically increasing remote CSeq, then
answers `200 OK` with `CSeq: 2 BYE`. No body or media session is created at any point.

Setup latency begins when the first INVITE byte is handed to the transport and ends after the final
2xx has been fully validated. Teardown latency begins when the BYE is handed to the transport and
ends after its final 2xx has been validated. A completed dialog has all five semantic steps in this
order: `INVITE, 2xx, ACK, BYE, 2xx`.

### 3.3 Retransmission and timeout accounting

UDP transactions use RFC 3261's state machines and configured `T1 = 500 ms`, `T2 = 4 s`, `T4 = 5 s`:
INVITE uses Timers A/B and the final-response side uses G/H/I; BYE uses E/F/K. A retransmission is
part of its original dialog and increments a retransmission counter; it never increments offered,
established or completed counts. Duplicate final responses cause the required ACK retransmission but
not another BYE. A request or response with the run's identifiers but invalid transaction/dialog
fields is `invalid_message`, not an unrelated packet.

One transaction reaching its RFC deadline is `transaction_timeout`. The phase does not extend to
wait for it: admission stops at the phase boundary and the at-most-40-second drain owns all remaining
transactions. Anything still live when drain expires is `cleanup_timeout` and makes the repetition
fail.

### 3.4 Failure classes

Each offered dialog reaches exactly one terminal class:

| Class | Meaning |
|---|---|
| `completed` | the five-step flow validated and both sides released the dialog |
| `rejected` | a valid final non-2xx response, recorded by exact status code |
| `transaction_timeout` | a required final response did not arrive before its RFC deadline |
| `invalid_message` | method, identifiers, CSeq, route, status or ordering contradicted the flow |
| `transport_error` | bounded send/receive failed before a terminal SIP outcome |
| `admission_refused` | responder's configured active-dialog limit refused the INVITE |
| `internal_error` | either endpoint reported an owned task/state failure |
| `cleanup_timeout` | work remained after the finite drain deadline |

`CANCEL` is not emitted by the v1 driver. A responder MUST nevertheless classify a valid CANCEL
race in its own conformance tests; that capability is not inferred from this workload.

## 4. Fixed execution protocol

Every duration below is wall time measured by a monotonic clock. No fixed duration stands in for
readiness or cleanup: readiness is a record, and cleanup is a zero-state/process-exit observation;
durations only bound their failure.

1. **Correctness preflight:** 20 dialogs at 1 call/s, maximum active 4. Every dialog must complete,
   all five semantic steps must be observed, and post-drain state must be zero.
2. **Driver headroom:** before testing endpoint builds, the driver runs against the packaged
   minimal fixture at `2 * ceiling` for 10 seconds warm-up plus 60 seconds measurement. It must meet
   every capacity predicate with driver CPU below 80% of one logical CPU. Failure invalidates the
   whole execution; it does not lower the ceiling.
3. **Six-rate ladder:** rates are `ceiling/32`, `ceiling/16`, `ceiling/8`, `ceiling/4`, `ceiling/2`
   and `ceiling`, rounded up to at least 1 call/s. Duplicate rounded rates are invalid; choose a
   ceiling large enough to produce six distinct rates.
4. **Repetitions:** each rate runs exactly five repetitions. A repetition has 10 seconds warm-up,
   an observed zero-active barrier, 60 seconds measurement, stopped admission, and an observed
   drain lasting no more than 40 seconds. Warm-up dialogs and counters are excluded from measurement
   and must drain before the measurement seed/index range begins.
5. **Early stop:** after every repetition at two consecutive rates has failed the capacity
   predicate, higher rates are recorded `not_run_after_two_failed_rates`; they are not zero-valued
   measurements. An isolated failed rate does not skip the next rate.

Seeds are derived as `seed XOR (direction_index << 56) XOR (rate_index << 32) XOR repetition_index`.
Runs do not retry a failed repetition under a new seed. A crash or environmental failure is evidence
from that attempt and remains visible.

## 5. Readiness, bounds and process supervision

Each process writes exactly one UTF-8 JSON line to stdout before accepting workload:

```json
{"schema":"sipx.comparative-load.ready.v1","role":"driver|responder","pid":1234,"address":"127.0.0.1:5060","transport":"udp","limits":{"active":1024,"events":65536,"stdout_bytes":16777216,"stderr_bytes":16777216}}
```

The driver address MAY be omitted; the responder address is required and must be an IP socket
address. The readiness line is at most 4096 bytes and must arrive within 10 seconds. EOF, malformed
JSON, duplicate readiness, a changed address, or traffic accepted before readiness fails the run.

All queues and counters have positive finite manifest limits. Per-process stdout and stderr are each
limited to 16 MiB; the structured event stream is at most 65,536 records and 64 MiB. Crossing a
limit terminates the repetition as `evidence_overflow` instead of truncating a successful result.
Secrets and raw packet bodies are forbidden in evidence.

The supervisor starts every external command directly, never through a shell, in a new process
group. It owns that group until every descendant exits. Its outer runner installs `EXIT`, `INT` and
`TERM` cleanup before the first child starts. Cleanup performs these observable steps:

1. stop admission and request orderly endpoint shutdown;
2. wait for endpoint zero-state and process exit for at most 5 seconds;
3. send `TERM` to the whole process group, never only the leader;
4. wait at most 5 seconds for group exit and inherited output pipes to close;
5. send `KILL` to the group if any member remains, then wait/reap the leader and require pipe EOF;
6. report every exit status, escalation and remaining endpoint counter.

A process must not daemonize, create a new session or move descendants out of its inherited group.
The supervisor treats an inherited pipe that remains open after leader exit as a descendant leak and
fails the run. Cleanup runs after success, failure, exception or signal; no result is complete until
it has run.

## 6. Stable result record

Each attempted repetition emits exactly one JSON object after cleanup. The closed top-level key set is:

```text
schema, status, run, build, machine, profile, counts, responses, errors,
latency_ms, resources, post_drain, cleanup
```

`schema` is `sipx.comparative-load.result.v1`. `status` is `passed`, `failed` or
`environment_failed`. An unattempted rate is a separate omission fact carrying its rate and the
reason `two_consecutive_failed_rates`; it is never a repetition populated with zero measurements.
Required result members are:

- `run`: `run_id`, `seed`, `direction`, `rate_index`, `rate_per_second`, `repetition`, UTC start,
  monotonic elapsed milliseconds, and phase durations;
- `build`: endpoint ID, role, immutable revision, artifact SHA-256 and command-argument SHA-256;
- `machine`: OS, architecture, logical CPUs, memory bytes and monotonic clock name;
- `profile`: transport, T1/T2/T4 milliseconds, maximum active, log/event limits and contract hash;
- `counts`: offered, established, completed, active high-water and retransmitted requests/responses;
- `responses`: maps decimal provisional and final status codes to measured counts;
- `errors`: every failure class from §3.4 plus `evidence_overflow` and `process_crash`, including
  explicit zeroes so an absent class cannot be mistaken for an omitted measurement;
- `latency_ms`: setup and teardown each carry count, p50, p95, p99 and maximum, or are absent when
  count is zero;
- `resources`: sampling interval and available measured fields among process CPU milliseconds,
  peak RSS bytes, descriptor high-water, task/thread high-water and endpoint-active high-water;
- `post_drain`: active dialogs, transactions, timers, endpoint tasks and retained event records;
- `cleanup`: admission stopped, zero-state observed, process-group exit observed, leader status,
  descendant pipe EOF, escalation (`none`, `term` or `kill`) and elapsed milliseconds.

Resource fields unsupported by the OS or endpoint are absent and accompanied by an
`unsupported_resources` string array. They MUST NOT be emitted as zero. A supported measurement may
legitimately be zero. Missing required metadata, unknown keys, non-finite numbers, negative counts,
inconsistent totals, a percentile without samples, or a success with non-zero post-drain state makes
the record invalid rather than partial.

Raw records, stdout/stderr (within their bounds), manifest, hashes and environment inventory are
written before any aggregate. A generated summary may only point at those immutable inputs.

## 7. Capacity and interpretation

A repetition passes capacity only when all of these are true:

- `completed / offered >= 0.999` with at least 1,000 offered dialogs;
- every `invalid_message`, crash, internal error, evidence overflow and cleanup timeout count is zero;
- loopback setup p99 is at most 250 ms;
- every post-drain count is zero and the process group plus inherited pipes are closed.

A rate is supported only when all five repetitions pass. The capacity point is the highest supported
rate below the first pair of consecutive failed rates. Report the five achieved-throughput values as
an observed uncertainty interval `[minimum, maximum]`; intervals that overlap are inconclusive.
Non-overlap permits stating that one measured interval is higher on that machine/profile, not that an
implementation is generally faster. Correctness failures, unsupported directions and environmental
failures are never converted into a ranking or a zero capacity.

## 8. Conformance vectors

The checker/supervisor fixture suite must contain at least:

| Vector | Expected result |
|---|---|
| CL1 exact profile and complete result | accepted |
| CL2 zero/missing phase bound | rejected before process start |
| CL3 missing cleanup or post-drain object | rejected |
| CL4 unsupported CPU/RSS/descriptor/task represented as zero | rejected; field must be absent |
| CL5 incomplete build/machine/hash metadata | rejected |
| CL6 malformed/oversized/duplicate readiness | process group terminated; no workload accepted |
| CL7 leader spawns a blocking descendant | group cleanup terminates both and observes pipe EOF |
| CL8 child escapes or retains an output pipe | failed as descendant leak |
| CL9 success record with live dialog/task/timer | rejected |
| CL10 two consecutive failed rates | higher rates recorded not-run, never measured as zero |

The actual cross-endpoint run belongs to M14. X-98 freezes and tests this contract; P-15 implements
the responder side without changing it.

## 9. Bounded responder command

### 9.1 Command and admission

`sipx load-responder` is the public P-15 answering surface. It binds exactly one UDP endpoint and
MUST reject any explicit non-UDP transport before opening a socket. It requires positive
`--max-active <N>` and `--cleanup <S>` values plus at least one positive admission bound,
`--calls <N>` or `--duration <S>`. If both admission bounds are present, the first reached closes
admission. `--dialog-duration <S>` is a positive bound on every accepted dialog and defaults to
40 seconds. Zero is not an unbounded spelling for any of these fields.

The command binds, obtains the OS-selected address, writes and flushes one
`sipx.comparative-load.ready.v1` record, and only then begins polling the endpoint receiver. It
never infers readiness from a delay. After admission closes it refuses fresh INVITEs with
`503 Service Unavailable` while continuing to route ACK, CANCEL and BYE for owned dialogs through
the earlier of zero state or the cleanup deadline.

No more than `max_active` dialog workers exist. An INVITE beyond that bound receives `503`; it does
not wait on a semaphore or enter an unbounded queue. The dispatcher inbox, per-dialog inbox,
terminal-event storage and joined-worker set all have finite capacities derived from
`max_active`. Arithmetic that would overflow or exceed the runtime's task bound is a usage error.

### 9.2 Seeded policy

`--seed <U64>` defaults to zero. For zero-based surfaced INVITE index `n`, two SplitMix64 outputs
derived from `seed XOR n` decide provisional and final policy. `--provisional-percent <0..100>`
controls whether exactly one `100 Trying` precedes the final response. `--answer-percent <0..100>`
controls acceptance; the remainder receives the final status selected by `--reject-status`, which
MUST be in 400 through 699 and defaults to 486. Given the same seed, indices and flags, decisions
MUST be identical across executions.

Successful signalling-only answers have no body and use the response shape in §3.2. Their To tag is
`t-<first-16-hex(H(seed || run_id || n || "to"))>` when the Call-ID has the profile form. A
non-profile Call-ID receives a deterministic tag derived from the same seed, its complete bounded
Call-ID and `n`; the command never reflects unbounded or invalid bytes into a header.

`--mode signalling` is the default and creates no SDP, RTP socket or media task. The distinct
`--mode generated-media` accepts only an INVITE carrying a negotiable SDP offer, uses the ordinary
call/media stack, and emits a finite deterministic audio fixture. A missing or unusable offer in
that mode receives a typed final refusal. Media behavior and measurements never enter the v1
signalling-load result.

Before either mode applies policy or consumes admission, an initial INVITE must carry exactly one
Call-ID, From, To, CSeq and Contact, a parseable CSeq whose method is INVITE, and enough dialog
identity to construct the response. A malformed request receives `400 Bad Request`, is counted as
invalid, and cannot consume an admission slot, the call bound or active high-water.

### 9.3 Per-dialog state machine

The signalling-only state machine is:

| State | Input | Action | Next |
|---|---|---|---|
| pending | selected provisional | send one `100`; do not create a dialog | pending |
| pending | policy rejects | claim INVITE; send configured final response | terminal |
| pending | matching CANCEL wins | dispatcher sends `200` to CANCEL and `487` to INVITE | terminal cancelled |
| pending | policy accepts | claim INVITE; send bodyless `200`; arm RFC 3261 G/H retransmission | awaiting ACK |
| awaiting ACK | matching ACK with INVITE CSeq | stop 2xx retransmission | established |
| awaiting ACK | final-response timer H | classify failure; release dialog | terminal failed |
| established | valid increasing BYE | send `200`; release dialog | terminal completed |
| established | dialog-duration deadline | originate BYE; require final 2xx within cleanup bound | terminal completed/failed |
| any live | malformed/wrong-dialog/out-of-order request | send the RFC response where one exists; classify invalid | unchanged or failed by policy |

The 2xx retransmission schedule is driven inside the dialog worker rather than by a nested task, so
worker completion is the complete ownership barrier. An ACK is validated
against the dialog identifiers and the INVITE sequence; an arbitrary packet cannot establish a
dialog. A BYE is validated against both tags, Call-ID, method-consistent CSeq and monotonically
increasing remote sequence before its `200` is counted. A final response to a locally originated
BYE must likewise match both dialog tags, Call-ID and that BYE's exact CSeq before it can complete
the dialog. CANCEL remains transaction-matched by the dispatcher and never becomes an in-dialog
BYE substitute.

### 9.4 Shutdown and result

Ctrl-C, duration/count completion and the first internal error atomically close admission. Shutdown
then requests BYE for each established owned dialog, stops pending invitations with a final response,
joins every worker, drops every dialog inbox, observes the dispatcher route set empty, and waits for
the endpoint's transaction/timer count to become zero. A monotonic cleanup deadline bounds failure;
no fixed sleep stands in for any of those barriers. Expiry reports failure and the exact non-zero
leftovers.

After cleanup the command writes exactly one `sipx.load-responder.v1` object. Its closed top-level
keys are:

```text
schema, status, seed, mode, limits, counts, responses, latency_ms, post_drain, reason
```

`status` is `completed`, `interrupted` or `failed`. `limits` records calls/duration, maximum active,
dialog duration and cleanup seconds. `counts` records surfaced INVITEs, admitted, established,
completed, cancelled, rejected, failed, active high-water and invalid messages. `responses` maps
one semantic observation per SIP transaction: provisional and final responses this command
successfully sends, plus a valid final response received for a BYE this command originated. A
response build/send failure and an invalid final response are not wire evidence; protocol-level
retransmissions do not add another observation. `latency_ms.setup` and `.teardown` each carry
the exact count and maximum plus p50, p95 and p99 from a seeded bounded reservoir, or are `null` with
no samples. Its capacity is eight observations per active-dialog slot, capped at 65,536, so a
duration-bounded run cannot turn latency evidence into unbounded memory use. `post_drain` records active dialogs,
dispatcher routes, endpoint transactions and owned tasks. Every terminal INVITE belongs to exactly
one completed/cancelled/rejected/failed class, and a completed/interrupted result has zero post-drain
state. `reason` is null except for interruption/failure and never contains packet bodies or secrets.
