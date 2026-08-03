---
title: CLI reference
description: Every sipx command, flag, exit code and JSON field — the surface a shell script can rely on.
---

# CLI reference

One binary, `sipx`. Four commands do work — `dial`, `answer`, `register` and `peers`, documented
below — alongside `help` and `version`. Global: `--json` switches the report to a single-line JSON
object on stdout; `-v`/`-vv` raise log verbosity on stderr (never stdout, so JSON stays
parseable); `-h`/`--help` on any command.

Every flag below whose name is followed by a placeholder — `--timeout <S>`, `--book <FILE>` — needs
a value, and a flag given none is a usage error (exit 2) naming the flag. It is never read as
absent: falling back to the default would run the command on something you did not ask for, and
say nothing. Both ways a value goes missing are refused:

- **Nothing after the flag.** `sipx register sip:alice@example.com --outbound --instance` is
  refused, rather than registering a device identity that was generated instead of given.
- **An empty value**, in either form — `--instance=` or `--instance ""`. No valued flag has a
  meaningful empty value, and omitting a flag is already how you ask for its default, so an empty
  one can only be a mistake. It is an easy one to make: an unset shell variable expands to exactly
  this, which is how `--target "$ADDR"` arrives with nothing in it.

Every `<S>` value is a whole number of seconds from `0` through `4294967295`. Negative values,
fractions, units such as `3s`, and values above that range are usage errors naming the flag. Zero is
deliberate per command: `--duration 0` ends an established call immediately, `--timeout 0` uses the
transaction layer's expiry, `--wait 0` returns immediately when no call is queued, and `--expires 0`
asks the registrar to remove the binding.

`--help` is answered before any of this, so it still prints when the rest of the line is wrong.

`dial`, `answer`, and `register` select `udp`, `tcp`, `tls`, `ws`, or `wss` with
`--transport <T>`. The default remains UDP; `--tcp` remains a compatible alias. TLS/WSS verify
certificates with the platform trust store plus `--tls-ca <FILE>`, and use the URI host unless
`--tls-server-name <NAME>` explicitly supplies the service identity. There is no flag that disables
verification and a `sips:` URI cannot select a cleartext transport.

`dial` and `register` may present a mutual-TLS identity with `--tls-cert <FILE>` and
`--tls-key <FILE>`. `answer` uses the same pair as its required server identity when listening on
TLS or WSS. Supplying only half the pair is a usage error before any socket is opened.

## `sipx dial <URI>`

Place a call: `sipx dial sip:bob@192.0.2.1:5060`

| Flag | Meaning |
|---|---|
| `--play <FILE>` | Play this WAV into the call (8 kHz 16-bit mono) |
| `--record <FILE>` | Record the far end to this WAV |
| `--dtmf <DIGITS>` | Send these digits once the call is up |
| `--duration <S>` | Hang up after this many seconds once connected (default 30) |
| `--timeout <S>` | Give up if not answered in this many seconds (default 20). `0` waits as long as the transaction layer does — 32 seconds |
| `--from <URI>` | Our own address (default `sip:sipx@<local>`) |
| `--local <ADDR>` | Local address to bind (default `0.0.0.0:0`) |
| `--transport <T>` | Use `udp`, `tcp`, `tls`, `ws`, or `wss` (default `udp`) |
| `--tcp` | Legacy alias for `--transport tcp` |
| `--tls-server-name <N>` | Certificate identity to verify (default URI host) |
| `--tls-ca <FILE>` | Add PEM trust roots to the platform store |
| `--tls-cert <FILE>` | Mutual-TLS client certificate chain; requires `--tls-key` |
| `--tls-key <FILE>` | Mutual-TLS client private key; requires `--tls-cert` |
| `--stats` | Report call quality on exit: loss, jitter, round trip, MOS estimate |
| `--capture <FILE>` | Record the signalling to this [pcapng](https://en.wikipedia.org/wiki/Pcap) file for a bug report. Credentials are redacted — digest responses and opaque `Bearer`/`Basic` tokens, SRTP keys (`a=crypto`, `k=`), push tokens, instance URNs. **TLS and WSS are recorded decrypted**, because capturing ciphertext from inside the process would be worse than capturing outside it. What redaction cannot remove is identity: the file still says who called whom, when, and from where, so treat it as sensitive |

Report fields: `status`, `peer`, `duration_ms`, `samples_recorded`, `heard_audio` — plus
`recording` when `--record` was given, and `loss`, `packets_lost`, `jitter_ms`, `mos`,
`round_trip_ms` under `--stats`. An explicit `--transport` also reports `requested_transport` and
`negotiated_transport`; legacy no-flag and `--tcp` output remains byte-for-byte compatible.

## `sipx answer`

Wait for a call and answer it: `sipx answer --play greeting.wav`

| Flag | Meaning |
|---|---|
| `--play <FILE>` | Play this WAV to the caller (8 kHz 16-bit mono) |
| `--record <FILE>` | Record the caller to this WAV |
| `--duration <S>` | Hang up after this many seconds (default 30) |
| `--wait <S>` | Give up if no call arrives within this many seconds (default 60) |
| `--local <ADDR>` | Local address to bind (default `0.0.0.0:5060`) |
| `--transport <T>` | Listen for `udp`, `tcp`, `tls`, `ws`, or `wss` (default keeps the historical UDP/TCP listeners) |
| `--tcp` | Select the historical TCP listener explicitly |
| `--tls-cert <FILE>` | TLS/WSS server certificate chain; requires `--tls-key` |
| `--tls-key <FILE>` | TLS/WSS server private key; requires `--tls-cert` |
| `--reject` | Answer 603 Decline instead |
| `--busy` | Answer 486 Busy Here instead |
| `--once` | Exit after one call (the default; kept for clarity in scripts) |
| `--capture <FILE>` | Record the signalling to this [pcapng](https://en.wikipedia.org/wiki/Pcap) file for a bug report. Credentials are redacted — digest responses and opaque `Bearer`/`Basic` tokens, SRTP keys (`a=crypto`, `k=`), push tokens, instance URNs. **TLS and WSS are recorded decrypted**, because capturing ciphertext from inside the process would be worse than capturing outside it. What redaction cannot remove is identity: the file still says who called whom, when, and from where, so treat it as sensitive |

Reports twice: `status: "listening"` with the bound `address` first, then
`status: "answered"` with `caller`, `duration_ms`, `samples_recorded`, `heard_audio` — plus
`dtmf` when digits arrived and `recording` when `--record` was given. Explicit selection adds the
requested transport to the listening report and both requested and negotiated transport to the
terminal report.

## `sipx register <AOR>`

Register with a registrar: `sipx register sip:alice@example.com`

| Flag | Meaning |
|---|---|
| `--password <P>` | Password. Prefer the `SIPX_PASSWORD` environment variable — argv is world-readable |
| `--target <ADDR>` | Where to send, if not derived from the AOR (`host:port`) |
| `--expires <S>` | Lease to ask for, in seconds (default 3600) |
| `--local <ADDR>` | Local address to bind (default `0.0.0.0:0`) |
| `--transport <T>` | Use `udp`, `tcp`, `tls`, `ws`, or `wss` (default `udp`) |
| `--tcp` | Legacy alias for `--transport tcp` |
| `--tls-server-name <N>` | Certificate identity to verify (default AOR domain) |
| `--tls-ca <FILE>` | Add PEM trust roots to the platform store |
| `--tls-cert <FILE>` | Mutual-TLS client certificate chain; requires `--tls-key` |
| `--tls-key <FILE>` | Mutual-TLS client private key; requires `--tls-cert` |
| `--keep-alive` | Keep refreshing until interrupted |
| `--outbound` | Register as one Outbound flow (RFC 5626): `reg-id` and `+sip.instance` on the Contact, the `outbound` option tag offered |
| `--instance <URN>` | With `--outbound`: present this device identity rather than a freshly generated one — §4.1 wants it stable across restarts, and the CLI keeps no state, so persisting one is the caller's job |
| `--push-provider <P>` | Push notification service this device can be woken through (RFC 8599). Requires `--push-prid` |
| `--push-prid <T>` | The identifier the push service knows this device by. Requires `--push-provider` |
| `--push-param <X>` | Service-specific extra, when the service needs one |
| `--capture <FILE>` | Record the signalling to this [pcapng](https://en.wikipedia.org/wiki/Pcap) file for a bug report. Credentials are redacted — digest responses and opaque `Bearer`/`Basic` tokens, SRTP keys (`a=crypto`, `k=`), push tokens, instance URNs. **TLS and WSS are recorded decrypted**, because capturing ciphertext from inside the process would be worse than capturing outside it. What redaction cannot remove is identity: the file still says who called whom, when, and from where, so treat it as sensitive |
| `--wake` | Act as though a push arrived once registered: send §4.1.3's binding-refresh REGISTER and report what it learned. Requires the push flags |

Report fields: `status`, `aor`, `expires`, `refresh_in` — plus `flow` under `--outbound`
(whether the registrar reported an Outbound registration, RFC 5626 §6) and `push` under the push
flags (whether the registrar named the same push service, RFC 8599 §8.2). `--wake` adds a second
report line with `status: "woken"` and, when the registrar assigned one, `purr`. Explicit transport
selection adds `requested_transport` and `negotiated_transport` to the registration result.

Combinations that cannot work are usage errors (exit 2), never parsed and dropped: half a push
pair, `--push-param` alone, `--wake` without the push flags, `--instance` without `--outbound`,
an `--instance` that is not a URN (RFC 5626 §4.1's grammar is `instance-val = urn`), and a
`--push-prid` that a URI parameter cannot hold. A valued flag left without a value — `--instance`
with nothing after it, or `--target=` — is refused for every command by the rule stated at the top
of this page, not by anything specific to `register`.

## `sipx peers`

List what can be called: `sipx peers --json`

| Flag | Meaning |
|---|---|
| `--book <FILE>` | Read this peer book rather than the default one |

The book is looked for in `--book`, then `$SIPX_PEERS`, then `$XDG_CONFIG_HOME/sipx/peers`, then
`$HOME/.config/sipx/peers`. It is a text file a shell can write — one peer per line, a name and a
URI separated by whitespace, `#` for a comment, blank lines ignored:

```text
# who this phone knows about
alice   sip:alice@192.0.2.17:5060
bob     sips:bob@example.com
```

```sh
echo "carol sip:carol@192.0.2.30:5060" >> ~/.config/sipx/peers
sipx peers --json | jq -r 'select(.source == "book") | .uri'
```

Reports one line per peer with `status` (always `peer`), `name`, `uri` and `source`. `source` says
where the entry was learned from — `book` is the only one today, and it is what keeps the list
extensible when other sources are merged into it.

A book that cannot be read — missing, unreadable, or holding a line that is not a name and a URI —
exits non-zero and names the file and the line. It never prints an empty list: on a fresh machine
that would read as "there is nobody to call" when the truth is "you have not been told about
anyone". A book that exists and holds no peers prints nothing and exits 0.

The command consults no network. It opens no socket and needs no registrar.

## Exit codes

Scripts branch on the exit code, not on parsing prose:

| Code | Meaning |
|---|---|
| 0 | Success |
| 1 | Failed (transport or protocol error) |
| 2 | Usage — the command line itself was wrong |
| 3 | Rejected — the far end refused |
| 4 | Unauthorized — credentials wrong or missing |
| 5 | Timeout — nothing answered in time |
| 6 | Busy |

Both `dial` and `answer` exit 0 after a completed call that received no audio. Silence is not a
signalling failure: a caller can legitimately stay quiet, a one-way announcement can send without
receiving, and `--record` asks the command to preserve whatever arrives rather than asserting that
something must arrive. Giving those successful calls a failure status would make the exit code
depend on an application policy the command was never given.

The media result remains machine-readable. A script that requires received audio uses `--json` and
requires `heard_audio: true`; `heard_audio: false` with `samples_recorded: 0` is a successfully
completed silent call. This rule is identical for `dial` and `answer` so the direction of the call
does not change what an exit status means.

## The JSON contract

`--json` emits exactly one JSON object per report, on one line, on stdout — a command with more
than one thing to report, such as `answer`'s bound address or `peers`' list, emits one such line
each; failures emit
`{"status": …, "error": …}` on **stderr**. The text and JSON forms carry the same field set —
that equality is asserted by a test, so a field you see in one is in the other.
