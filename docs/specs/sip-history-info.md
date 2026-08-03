# SIP diversion history and reasons

Status: normative

This specification defines sipx's user-agent behavior for `Reason` and `History-Info`. It is
limited to the UA role: sipx reads histories it receives and extends one when it retargets a
request. It does not claim the proxy/forking role.

## 1. Normative references

- RFC 3326 §§2-3 defines `Reason`, its permitted message locations, and SIP/Q.850 causes.
- RFC 7044 §§5-6 and §§9-10 defines the `History-Info` grammar, UA procedures, indexing,
  retargeting causes, and privacy.
- RFC 3261 §19.1 defines the SIP URI `headers` component used to embed `Reason` and `Privacy`.

The RFC keywords MUST, MUST NOT, SHOULD, and MAY have their RFC meanings.

## 2. Types and wire forms

`Reason` is a typed comma-separated list. Each value has a case-insensitive protocol token, one
required decimal `cause`, optional quoted `text`, and preserved extension parameters. `SIP`
causes MUST be valid SIP status codes (100 through 699). `Q.850` causes MUST fit the decimal
cause range 0 through 127. A writer MUST emit the protocol, then `cause`, then `text`, then
extensions.

`History-Info` is a typed comma-separated list of entries. Each entry contains:

- a targeted-to URI represented as a name-addr;
- one required `index` made from dot-separated decimal components;
- at most one target-change parameter: `rc`, `mp`, or `np`, whose value is an earlier index;
- preserved extension parameters.

An index component is `0` or a positive decimal integer without leading zeroes. `rc` means the
Request-URI changed without changing the target user, `mp` means it changed to another target
user, and `np` means the Request-URI did not change. A change parameter MUST refer to an entry
which precedes it on the wire.

The typed forms preserve entry order. Parsing a malformed index, a missing/duplicate `index`,
multiple target-change parameters, or a forward reference is a typed header error, never a panic.

## 3. Reason placement and call actions

The initial INVITE MUST NOT contain `Reason`. A locally generated CANCEL and BYE MUST contain one
Reason value:

| action | default wire reason |
| --- | --- |
| user cancels a pending INVITE | `Q.850;cause=16;text="Normal call clearing"` |
| INVITE attempt times out | `SIP;cause=408;text="Request Timeout"` |
| user hangs up an established call | `Q.850;cause=16;text="Normal call clearing"` |
| session expires | `SIP;cause=408;text="Request Timeout"` |

The call API MUST also accept an explicit typed reason for CANCEL or BYE. This makes the RFC 3326
§3.1 operation expressible: a controller can cancel a coupled outbound leg using, for example,
`SIP;cause=200;text="Call completed elsewhere"`. An explicit SIP or Q.850 cause MUST be emitted
unchanged after validation.

Sipx only writes `Reason` on an in-dialog request or CANCEL. It MUST NOT copy one onto an initial
INVITE or an arbitrary response.

## 4. History cache and indexing

An outbound initial INVITE that requests history MUST contain `Supported: histinfo` and one
`History-Info` entry for its Request-URI with index `1`.

For a retarget from request target `old` to `new`, a UA performs these steps:

1. Parse the received history, or create `<old>;index=1` if none exists.
2. If the final cached entry does not identify `old`, expose the missing hop by appending an entry
   for `old` whose index is the final index followed by `.0`.
3. Put the status which caused the retarget in the URI `headers` component of that old entry as a
   percent-encoded `Reason` value, unless `old` is a `tel` URI.
4. Append `new` with the old entry's index followed by `.1`, and set exactly one of `rc`, `mp`, or
   `np` to the old entry's index.

Thus the first forwarding chain is `1`, `1.1`, `1.1.1`. If an incoming cache ends at `1` while the
actual previous Request-URI is absent, its next indices are `1.0` and `1.0.1`; the zero is visible
evidence of the gap and MUST NOT be flattened away. A received forked history is read and retained
in its existing wire order; this UA does not add sibling entries on behalf of a proxy.

`Reason` is embedded using the URI `headers` component, for example
`<sip:old@example.test?Reason=SIP%3Bcause%3D302>;index=1`. Existing URI headers MUST be retained.
Because a `tel` URI has no SIP URI headers component, its reason remains available to the caller
but MUST NOT be inserted into that URI.

## 5. Responses

A caller which wants history back MUST advertise the `histinfo` option tag. For every response
other than 100, a UAS:

- returns the received History-Info cache when the request carried it;
- otherwise creates the index-`1` request-target entry when the request carried
  `Supported: histinfo`;
- omits History-Info when neither signal was present.

A 100 response MUST NOT contain History-Info. A received response's typed history remains
available to the call/user-agent consumer.

## 6. Privacy

Before a cache is emitted, message-level `Privacy: history` or `Privacy: header` anonymizes every
entry for which sipx is the responsible UA. Because sipx cannot prove which received entries belong
to a different administrative domain, its safe UA policy is to anonymize every emitted entry. The
target URI becomes `sip:anonymous@anonymous.invalid`; embedded `Privacy`, `Reason`, URI parameters,
display names, and identifying entry extensions are removed, while `index` and target-change
parameters remain so the sequence is still useful.

An entry containing the URI header `Privacy=history` is anonymized by the same rule even without a
message-level Privacy header. A malformed privacy marker fails typed decoding rather than leaking
the original URI through a best-effort rewrite.

## 7. State table

| input | state change | output |
| --- | --- | --- |
| initial outbound target | start cache | target at `1`; `Supported: histinfo` |
| received cache whose last target equals previous target | extend cache | reason on last entry; new target at `last.1` |
| received cache whose last target differs | expose gap, then extend | previous at `last.0`; new target at `last.0.1` |
| non-100 response | retain response cache | typed history available to caller |
| privacy `history`/`header` | anonymize before emission | indices retained; identifying values removed |

## 8. Byte-level vectors

The following vectors are normative and are used verbatim by tests.

**V1 — first retarget with cause**

Input target: `sip:alice@example.test`; new target: `sip:bob@example.test`; cause: SIP 302; change:
different target user.

```text
History-Info: <sip:alice@example.test?Reason=SIP%3Bcause%3D302>;index=1, <sip:bob@example.test>;index=1.1;mp=1

```

**V2 — a visible missing hop**

Input cache ends at `<sip:first@example.test>;index=1`, actual previous target is
`sip:hidden@example.test`, and the new target is `sip:last@example.test`.

```text
History-Info: <sip:first@example.test>;index=1, <sip:hidden@example.test?Reason=SIP%3Bcause%3D302>;index=1.0, <sip:last@example.test>;index=1.0.1;mp=1.0

```

**V3 — privacy**

Input entries are V1 and the message carries `Privacy: history`.

```text
History-Info: <sip:anonymous@anonymous.invalid>;index=1, <sip:anonymous@anonymous.invalid>;index=1.1;mp=1

```

**V4 — call reasons**

```text
Reason: Q.850;cause=16;text="Normal call clearing"
Reason: SIP;cause=408;text="Request Timeout"
Reason: SIP;cause=200;text="Call completed elsewhere"
```
