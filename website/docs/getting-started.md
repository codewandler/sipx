---
title: Getting started
description: Install the sipx CLI and place a first call between two terminals with WAV audio.
---

# Getting started

This walkthrough makes a real local SIP call between two `sipx` processes. It needs no PBX,
account, or configuration file. The exact public beta is available from crates.io; `main` may move
ahead of it.

## Install the public beta

Install the exact release with Rust <!-- BEGIN generated:msrv -->1.88<!-- END generated:msrv --> or newer:

```bash
cargo install --locked --version =1.0.0-beta.1 sipx-cli
```

The exact `--version` requirement makes the installation reproducible. This site follows the
newer `main` branch; to
try that development state instead, use:

```bash
cargo install --git https://github.com/codewandler/sipx \
  --branch main --locked sipx-cli
```

Confirm which version was installed. This documentation build covers
<!-- BEGIN generated:workspace-version -->1.0.0-beta.1<!-- END generated:workspace-version -->:

```console
$ sipx version
sipx 1.0.0-beta.1
```

## Prepare audio

The CLI is a scriptable softphone, not a desktop audio phone. WAV input through `--play` and WAV
output through `--record` are the reproducible defaults. A build with the optional `device-audio`
feature can instead open an exact microphone or speaker identifier; it does not add a graphical
device picker or mixer.

Input WAV files must be **16-bit mono PCM at the negotiated codec clock**: 8 kHz for the default
G.711 codecs or 48 kHz when both ends select Opus. sipx refuses a mismatched rate instead of
silently changing the clip's speed. If you do not have one, omit `--play` on either command below.
The call will still complete, but that side sends silence.

## Make a call

In terminal one, listen on the default local address, play a greeting, and record the caller:

```bash
sipx answer --play greeting.wav --record caller.wav --once
```

The first report identifies the listening socket:

```text
status   listening
address  0.0.0.0:5060
```

In terminal two, call that listener for ten seconds:

```bash
sipx dial sip:you@127.0.0.1:5060 \
  --play hello.wav --record reply.wav --duration 10
```

Both commands finish with an `answered` report. `caller.wav` contains audio sent by the
dialler, and `reply.wav` contains the answerer's greeting. The reports also say how many
samples were recorded and whether any audio was heard.

Outbound `dial` and `register` commands default to UDP. For compatibility, `answer` without a
transport flag listens on both UDP and TCP; select exactly one of UDP, TCP, TLS, WebSocket, or
secure WebSocket with `--transport`. Secure paths verify the URI host against the peer certificate
and never retry over cleartext; use `--tls-ca` to add a private authority.

## Register an address

If you have SIP account credentials, keep the password out of the process list:

```bash
SIPX_PASSWORD='your-password' \
  sipx register sip:alice@example.com --keep-alive
```

Registration is a lease. `--keep-alive` refreshes it until the command is interrupted. The CLI
can use UDP, TCP, TLS, WebSocket, or secure WebSocket for registration; see
[Register against a PBX](guides/register.md) for target selection, certificate verification,
Outbound, and the library API.

## Script the result

Add `--json` to emit one single-line JSON object on stdout. Commands also use distinct exit
codes for success, rejection, authentication failure, timeout, busy, usage error, and other
failure:

```bash
if sipx dial sip:alice@192.0.2.10:5060 --timeout 15 --json >result.json; then
  echo "answered"
fi
```

Logs go to stderr, so JSON on stdout remains parseable. See the [CLI reference](reference/cli.md)
for every flag, output field, and exit code.

## Next

- [Use sipx as a library](guides/as-a-library.md).
- [Place a call from Rust](guides/place-a-call.md).
- [Answer calls from Rust](guides/answer-a-call.md).
- [Does sipx fit?](guides/does-this-fit.md).
- [Troubleshooting](guides/troubleshooting.md).
