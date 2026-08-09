---
title: Hear a call
description: Dial a SIP endpoint and hear it through your speakers, talk back through a microphone, and diagnose the answered-but-silent call.
---

# Hear a call

Every other guide here proves a call worked by reading a number: packets sent, samples recorded, a
WAV file on disk. This one proves it the way a person does — by listening. That turns out to be the
fastest way to find media problems, because a call that is measurably fine and audibly wrong is a
distinction no counter reports.

Two examples. The first listens. The second also talks, through a microphone and the DSP graph.

## Dial and listen

[`crates/sipx-call/examples/call_and_listen.rs`](https://github.com/codewandler/sipx/blob/main/crates/sipx-call/examples/call_and_listen.rs)
takes a SIP URI, dials it, and plays whatever comes back to the default output device as it arrives.
Nothing is buffered to a file first, so what you hear is what the call carried, at the rate the two
ends negotiated.

```bash
cargo run --example call_and_listen -- sip:1000@192.0.2.10:5066
cargo run --example call_and_listen -- sip:alice@192.0.2.10 --seconds 30 --record out.wav
```

It sends a 440 Hz tone by default, and that is not decoration. An **echo** service returns what you
send it, so a client that only listens hears silence — and that silence is indistinguishable from a
broken media path. `--tone 0` turns it off for a far end that talks first, like an announcement or
an IVR.

## Talk back, through the DSP graph

[`crates/sipx-call/examples/softphone.rs`](https://github.com/codewandler/sipx/blob/main/crates/sipx-call/examples/softphone.rs)
adds capture: microphone in, far end out. Everything it sends passes through an ordered outbound DSP
graph first — a real [`GraphPlan`](/docs/guides/as-a-library) attached with `attach_dsp`, the same
one a deployment would use, not a private code path for the example's benefit.

| Stage | Why it is in the chain |
|---|---|
| `HighPass` at 120 Hz | Desk rumble, mains hum and the room's low end carry no speech and eat the codec's budget. |
| `SubbandSuppressor` | Per-band noise estimate; steady noise falls away and speech does not. |
| `Peaking` at 2.6 kHz | Consonants live here. A narrow lift buys intelligibility a wideband gain cannot. |
| `Gain` | Makeup for what the two filters removed, smoothed so it cannot step. |

```bash
cargo run --example softphone -- sip:1000@192.0.2.10:5066
cargo run --example softphone -- sip:alice@192.0.2.10 --no-dsp --record raw.wav
```

`--no-dsp` sends the microphone untouched. That comparison is the point: the chain is not meant to
be audible in itself, it is meant to make the far end's return of it cleaner. Against an echo
service, use headphones — speakers and a microphone in one room will howl.

Capture negotiates whatever the device actually offers rather than demanding 8 kHz mono, and
resamples with the phase carried across callbacks. A device that advertises a rate range it will not
honour is common enough that assuming otherwise fails on ordinary hardware, and it fails as
`DeviceNotAvailable` — which reads exactly like an unplugged microphone.

## Answered, but nothing comes back

This is the failure worth spending a section on, because the instinct it provokes is wrong.

**Answering proves signalling arrived, and says nothing whatsoever about media.** The two travel
independently: signalling to the address you dialled, media to the address the far end put in its
SDP answer. Those can be on different networks, and when they are, a media socket bound for one
cannot reach the other.

The shape to look for: you dial an endpoint over a tunnel or a private route, and the far end — quite
correctly, from its point of view — answers with the public address it was configured to advertise.

```bash
ip route get 198.51.100.20   # the far end's SDP address → dev eth0,  src 192.168.1.50
ip route get 10.20.30.40     # the endpoint you dialled  → dev tun0,  src 10.99.0.3
```

Two interfaces, two source addresses. Media bound on `10.99.0.3` will never reach `198.51.100.20`,
and no amount of firewall opening changes that. **A retransmitted `200 OK` in the capture is the
tell** — the far end is not hearing your `ACK`, which took the same broken path media is about to
take. If you can, dial the far end at the address its media already uses, so both legs share one
route. If you cannot, `--advertise` puts a reachable address in SDP while the socket stays bound
locally.

## Your private range can be someone else's

The subtler version: the address you advertise is routable for you and *means something else* to the
far end.

Private ranges are drawn from a small pool, and two networks pick from it independently. If the
address in your `c=` line falls inside a range the far end already uses — its own VPC, its container
network, its virtual-service range — then it will send media there in perfect good faith, to a
destination on its own network that has nothing to do with you. Media vanishes with no error at
either end, because nothing went wrong: the packets were delivered exactly where the SDP said.

Before blaming the network, check whether the address you are advertising is inside a range the far
end could plausibly own. If it is, advertise a different one.

## Virtual service addresses are not addresses

A container platform's virtual service address — the kind that load-balances across a set of backing
instances — is usually not an address at all. It is a rule in each node's packet filter, held by no
interface. Nothing outside that platform can reach one, and a VPN that carries the platform's real
networks still cannot, because there is nothing there to carry.

Dial a backing instance's real address instead. The same applies to the platform's DNS: names in its
internal zone are served from a virtual address, so a resolver outside it will not answer them — and
the failure looks like a timeout rather than the category error it is.

## Related

- [Troubleshooting](/docs/guides/troubleshooting) — the full checklist, including one-way audio,
  authentication and captures.
- [Play audio](/docs/guides/play-audio) — playback into a call without a device.
- [Record a call](/docs/guides/record-a-call) — when you want the file rather than the sound.
