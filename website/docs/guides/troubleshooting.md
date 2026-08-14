---
title: Troubleshooting
description: Diagnose signalling, audio, authentication, timeout, and capture problems without hiding the operational limits.
---

# Troubleshooting

Start with the smallest observable path: one direct call, a short timeout, verbose logs, and a
capture only if the logs do not settle it.

```bash
sipx -vv dial sip:bob@192.0.2.10:5060 --timeout 10 --duration 10
```

Logs go to stderr. Command results stay on stdout, so `--json` remains safe to pipe or parse.

## The command cannot bind its address

`sipx answer` binds `0.0.0.0:5060` by default. Only one process can normally own the same UDP
address and port. Stop the process already listening there or choose another port:

```bash
sipx answer --local 0.0.0.0:5070 --wait 30
```

Then dial the port that was actually bound. `dial` and `register` default to port `0`, which asks
the operating system for an ephemeral local port; set `--local` only when a firewall or deployment
requires a fixed source port.

A SIP listening port is not the RTP port. The call layer allocates media ports separately and
advertises them in SDP. Firewalls must allow both signalling and the negotiated UDP media flow.

## Bound here, advertised there

The bind address is where the local socket listens. The advertised address is what the remote
endpoint is told to contact in SIP headers and SDP. They are often different on a multi-homed host,
in a container, or behind NAT.

Never advertise `0.0.0.0`; it is not a destination. The CLI's automatic choices are intended for
direct and local-network calls, and it cannot discover an arbitrary public NAT mapping. Use
`--local` for the socket and `--advertise` for the reachable IP on `dial` and `answer`. Rust
applications set `Config::sent_by` for signalling and use `MediaAddress::with_bind` (or
`DialOptions::with_media_bind_address`) for media. `sipx-host` likewise separates a listener's
`bind` and `advertise` values and takes its media address as a process argument.

When `sipx answer` binds a wildcard address without `--advertise`, it asks the routing table which
local interface faces the caller and binds media there, so ICE can gather a real local base. It
never copies the caller's source address into local SDP. With an explicit public `--advertise`, the
media bind stays on `--local` because a NAT mapping is normally not locally owned.

If signalling arrives but replies do not, inspect the Via and Contact addresses. If the call
connects but audio does not, inspect the `c=` address and `m=` port in SDP.

## One-way or missing audio

Check these in order:

1. Confirm both sides report an answered call and that the play file was accepted.
2. Verify the SDP address and port are reachable from the other endpoint.
3. Allow the RTP and RTCP UDP ports chosen for the call through host and network firewalls.
4. Check whether a NAT rewrote only one direction or whether two layers of NAT prevent a direct
   path.
5. Use `--stats` on `dial` to distinguish packets that arrived but decoded poorly from packets that
   never arrived.

sipx supports symmetric RTP: after a valid packet arrives, media can be sent back to its observed
source instead of the address advertised in SDP. CLI calls can also gather host candidates or use a
configured STUN server to gather server-reflexive candidates and run ICE connectivity checks. A
library application can supply `sipx_call::TurnPolicy` to gather and use a configured relay when a
direct path cannot work; sipx does not operate the relay service. When ICE is enabled, the nominated
candidate pair owns the destination; ordinary RTP source learning cannot override it.
WebSocket signalling does not solve media reachability; RTP uses its own network path.

## Signalling and media took different routes

A call that answers and then carries no audio is not a half-broken call. Answering proves signalling
arrived; media travels independently, to whatever address the far end put in its SDP answer, and
that address can be on a different network than the one you dialled.

The common shape is a far end reached over a tunnel or private route that answers with the public
address it is configured to advertise. Ask the routing table about both:

```bash
ip route get 198.51.100.20   # the far end's SDP address → dev eth0,  src 192.168.1.50
ip route get 10.20.30.40     # the endpoint you dialled  → dev tun0,  src 10.99.0.3
```

Two interfaces and two source addresses mean a media socket bound for one cannot reach the other, and
no firewall rule changes that. **A retransmitted `200 OK` is the tell**: the far end is not receiving
your `ACK`, which took the same path media is about to take. Dial the far end at the address its
media already uses so both legs share one route, or set `--advertise` to an address it can reach
while the socket stays bound locally.

A related failure has no error at all: the address in your `c=` line falls inside a private range the
far end also uses, so it sends media to its own network in good faith. Private ranges come from a
small pool and two networks choose independently. Check that what you advertise is not inside a range
the far end could plausibly own.

Finally, a container platform's virtual service address is generally not an address — it is a rule in
each node's packet filter, held by no interface. Nothing outside the platform reaches one, including
a VPN carrying the platform's real networks. Dial a backing instance directly. See
[Hear a call](/docs/guides/hear-a-call) for working through this by ear.

## WAV input is rejected or sounds wrong

The default CLI uses files rather than a microphone or headset. `--play` accepts **16-bit mono PCM**
and linearly resamples the header's supported rate to the negotiated clock: 8 kHz for G.711, 44.1
or 8 kHz for L16, or 48 kHz for Opus. Convert other sample widths, channel counts, compressed audio,
or rates above 384 kHz before the call. Builds with the optional
`device-audio` feature can open an explicitly selected device. Read an input error emitted before
dialling as a local setup failure: sipx validates playback input before it lets the far end answer.

Recordings written by `--record` preserve that negotiated clock in the WAV header. Silence in a
valid output file is usually a media-path problem, not a file-format problem; return to the SDP and
firewall checks above.

## Authentication fails

For registration, prefer the environment to a command-line password because process arguments may
be visible to other local users:

```bash
SIPX_PASSWORD='secret' sipx -v register sip:alice@example.com --target 192.0.2.10:5060
```

A 401, 403, or 407 result maps to the CLI's unauthorized exit code. Confirm the address of record,
authentication username expected by the service, password, target, and whether the service expects
a protected transport. Both `register` and `dial` answer Digest challenges; prefer
`SIPX_PASSWORD` to `--password`, since process arguments may be visible to other local users.

Select `--transport tls` or `--transport wss`, add a private authority with `--tls-ca`, and set
`--tls-server-name` only when the service identity differs from the URI host. A name, issuer, or
expiry failure is reported as TLS and never retried over cleartext. See [Security](../reference/security.md).

## A call or registration times out

`dial --timeout <S>` limits how long a call may ring before sipx sends CANCEL. The default is 20
seconds. A value of `0` leaves the SIP transaction machinery to expire after about 32 seconds.
`answer --wait <S>` bounds how long the process listens for an incoming call; its default is 60
seconds. A timeout does not prove the destination is down: a wrong route, firewall drop, unusable
advertised address, or unanswered authentication challenge can look the same without logs.

Registration is a lease. Without `--keep-alive`, the command registers and exits; with it, sipx
refreshes until interrupted. If registration succeeds and incoming calls still do not arrive,
check that the registered Contact is reachable and whether the deployment requires RFC 5626
Outbound (`--outbound`) over the client-opened flow.

## Logs and captures

Use `-v` for informational logs and `-vv` for debug logs. Logs are written to stderr:

```bash
sipx -vv dial sip:bob@192.0.2.10:5060 --json >result.json 2>sipx.log
```

When logs are insufficient, add `--capture call.pcapng` to `dial`, `answer`, or `register`. The
capture contains signalling only. Credentials, push identifiers, and SDP key material are
redacted, but identities, addresses, routes, call timing, and other metadata remain. TLS and WSS
signalling is stored after decryption. Restrict access to the file and remove it after use; do not
attach it to a public issue without reviewing it first.

The [CLI reference](../reference/cli.md) lists command output, exit codes, and capture behavior in
full.
