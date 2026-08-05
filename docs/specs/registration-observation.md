# Registration path observation

## Scope

This specification defines one fact a registering user agent may learn from the final successful
REGISTER response: the IP address and port the registrar reports seeing as the request source. It
does not change registration success, the granted lease, or any routing/media configuration.

Normative references:

- RFC 3261 §18.2.1: a server adds `received` to the topmost `Via` when the source address differs
  from the sent-by address.
- RFC 3581 §§3–4: a client requests symmetric response routing with a valueless `rport`; the server
  returns the observed source port in `rport` and also supplies `received`.

Only the topmost `Via` of the final 2xx response is read. No observation is taken from a challenge,
provisional response, earlier retry, lower `Via`, `Contact`, socket peer address, Path,
Service-Route, GRUU, Outbound flow state, push state, SDP or RTP.

## Types

`RegistrationObservation` is stored in `registrar::Registered` and on `UserAgent` after success:

| Variant | Meaning |
|---|---|
| `Absent` | A valid top `Via` carries neither `received` nor `rport` |
| `Observed(SocketAddr)` | Exactly one IP-valued `received` and one decimal, non-zero `rport` form an address |
| `Invalid(RegistrationObservationError)` | Observation parameters were present but cannot state one unambiguous socket address, or the top `Via` itself is missing/malformed |

`RegistrationObservation::address()` returns `Some(SocketAddr)` only for `Observed`. The enum is the
primary API because `Option<SocketAddr>` alone would collapse absent, malformed and contradictory
registrar behavior into the same value.

`RegistrationObservationError` has these closed meanings:

| Error | Condition |
|---|---|
| `MissingVia` | The successful response has no `Via` header |
| `MalformedVia` | The first `Via` header cannot produce a top hop |
| `ContradictoryReceived` | The top hop carries `received` more than once |
| `ContradictoryRport` | The top hop carries `rport` more than once |
| `MissingReceived` | `rport` is present but `received` is absent |
| `MissingRport` | `received` is present but `rport` is absent, or `rport` has no value |
| `NonIpReceived` | `received` is valueless or not an IPv4 or IPv6 address; bracketed IPv6 is accepted |
| `InvalidRport` | `rport` is not a decimal integer in `1..=65535` |

Duplicate parameters are contradictory even when their bytes happen to agree: a single-valued
observation with two assertions has no defined authoritative copy, and accepting the first would
make wire order a policy decision.

## Interpretation state table

The table is evaluated in order against the final successful response:

| Input | Outcome | Other registration state |
|---|---|---|
| no top `Via` | `Invalid(MissingVia)` | success and every other field are unchanged |
| malformed top `Via` | `Invalid(MalformedVia)` | unchanged |
| duplicate `received` | `Invalid(ContradictoryReceived)` | unchanged |
| duplicate `rport` | `Invalid(ContradictoryRport)` | unchanged |
| neither parameter | `Absent` | unchanged |
| only `rport` | `Invalid(MissingReceived)` | unchanged |
| only `received`, or valueless `rport` | `Invalid(MissingRport)` | unchanged |
| `received` is not an IP literal | `Invalid(NonIpReceived)` | unchanged |
| `rport` is not decimal `1..=65535` | `Invalid(InvalidRport)` | unchanged |
| one valid value of each | `Observed(SocketAddr)` | unchanged |

On each successful registration or refresh, `UserAgent` replaces the previous observation with the
final response's outcome, including `Absent` or `Invalid`. A challenge never updates it. A failed
attempt does not describe a new successful binding and therefore does not replace the observation
of the last success.

## Invariants and limits

- The observation is informational. sipx does not copy it into a future REGISTER `Contact`, dialog
  target, DNS/route selection, GRUU, Outbound or push configuration, SDP connection address, ICE
  candidate, RTP destination or device address.
- A registration succeeds when its ordinary 2xx/lease rules succeed even if the observation is
  absent or invalid. Consumers decide whether an invalid observation matters to their deployment.
- UDP, TCP and other connection-oriented responses use the same interpretation. The transport does
  not make the observation more authoritative; it merely carries the response bytes.
- No hostname resolution occurs. `received=example.test` is `NonIpReceived`, not an instruction to
  perform I/O.
- Parsing is bounded by the existing response and header limits and performs no allocation based on
  a numeric value.

## Test vectors

Given a syntactically valid top hop before the shown parameters:

| Parameters | Expected outcome |
|---|---|
| _(none)_ | `Absent` |
| `;received=203.0.113.9;rport=41234` | `Observed(203.0.113.9:41234)` |
| `;received=[2001:db8::9];rport=5060` | `Observed([2001:db8::9]:5060)` |
| `;received=registrar.example;rport=5060` | `Invalid(NonIpReceived)` |
| `;received;rport=5060` | `Invalid(NonIpReceived)` |
| `;received=203.0.113.9;rport=nope` | `Invalid(InvalidRport)` |
| `;received=203.0.113.9;rport=+5060` | `Invalid(InvalidRport)` |
| `;received=203.0.113.9;rport` | `Invalid(MissingRport)` |
| `;rport=41234` | `Invalid(MissingReceived)` |
| `;received=203.0.113.9;received=203.0.113.10;rport=41234` | `Invalid(ContradictoryReceived)` |
| `;received=203.0.113.9;rport=41234;rport=41235` | `Invalid(ContradictoryRport)` |

The end-to-end matrix sends REGISTER over UDP and TCP with learned and absent final observations.
The authenticated case puts a different observation on the 401 and the final 200 and asserts that
only the latter is retained.
