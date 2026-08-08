---
title: Getting started
description: Install the sipx CLI and place a first call between two terminals with WAV audio.
---

# Getting started

This walkthrough makes a real local SIP call between two `sipx` processes. It needs no PBX,
account, or configuration file. The exact public prerelease is available from crates.io; `main` may
move ahead of it.

## Install the public prerelease

Install the exact release with Rust <!-- BEGIN generated:msrv -->1.88<!-- END generated:msrv --> or newer:

```bash
cargo install --locked --version =1.0.0-rc.6 sipx-cli
```

The exact `--version` requirement makes the installation reproducible. This site follows the
newer `main` branch; to
try that development state instead, use:

```bash
cargo install --git https://github.com/codewandler/sipx \
  --branch main --locked sipx-cli
```

### Prebuilt release binaries

Release candidates and stable releases, from the first published candidate onward, also attach an
exact native archive and SPDX bill of materials for each supported target:

| Machine | Target and archive suffix |
|---|---|
| x86-64 Linux | `x86_64-unknown-linux-musl.tar.gz` |
| Arm64 Linux | `aarch64-unknown-linux-musl.tar.gz` |
| Intel macOS | `x86_64-apple-darwin.tar.gz` |
| Apple silicon macOS | `aarch64-apple-darwin.tar.gz` |
| x86-64 Windows | `x86_64-pc-windows-msvc.zip` |

For example, install the static x86-64 Linux binary after verifying the published checksum:

```bash
VERSION=1.0.0-rc.6
TARGET=x86_64-unknown-linux-musl
ARCHIVE="sipx-$VERSION-$TARGET.tar.gz"
curl --fail --location --remote-name \
  "https://github.com/codewandler/sipx/releases/download/v$VERSION/$ARCHIVE"
curl --fail --location --remote-name \
  "https://github.com/codewandler/sipx/releases/download/v$VERSION/SHA256SUMS"
grep -F "  $ARCHIVE" SHA256SUMS | sha256sum --check
tar -xzf "$ARCHIVE"
install -m 755 "sipx-$VERSION-$TARGET/sipx" "$HOME/.local/bin/sipx"
```

Use the target from the table for another machine (`shasum -a 256 --check` supplies the macOS
checksum command; Windows can compare `Get-FileHash -Algorithm SHA256` with `SHA256SUMS`). These
portable executables deliberately contain no optional native features. Use the exact Cargo install
above when you need `device-audio`, `opus` or `dtls`; the archive's `build-manifest.json` and SPDX
sidecar record that distinction.

Confirm which version was installed. This documentation build covers <!-- BEGIN generated:workspace-version -->1.0.0-rc.6<!-- END generated:workspace-version -->:

```console
$ sipx version
sipx 1.0.0-rc.6
```

## Prepare audio

The CLI is a scriptable softphone, not a desktop audio phone. WAV input through `--play` and WAV
output through `--record` are the reproducible defaults. A build with the optional `device-audio`
feature can instead open an exact microphone or speaker identifier; it does not add a graphical
device picker or mixer.

Input WAV files must be **16-bit mono PCM** and carry a supported sample rate in their header.
sipx linearly resamples them to the negotiated clock: 8 kHz for the default G.711 codecs, 44.1 or
8 kHz for L16, or 48 kHz when both ends select Opus. If you do not have one, omit `--play` on
either command below. The call will still complete, but that side sends silence.

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

For the narrower browser-compatible path, install the same exact release with the optional `opus`
and `dtls` features:

```bash
cargo install --locked --version =1.0.0-rc.6 --features opus,dtls sipx-cli
```

Then select the fail-closed `browser-audio` profile over WSS. It composes Opus, host or
server-reflexive ICE, DTLS-SRTP, and multiplexed RTP/RTCP; the
[native-browser proof](reference/browser-audio-proof.md) exercises both SIP roles and names the
exact evidence. It does not cover TURN-required networks, video, data channels, browser-facing
APIs, or a general WebRTC stack.

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
