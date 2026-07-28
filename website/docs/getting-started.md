---
title: Getting started
description: Install the sipx CLI and place your first call between two terminals in five minutes — no PBX, no account, no configuration file.
---

# Getting started

Five minutes to a real call: two sipx processes, one dials the other, audio flows both ways.
No PBX, no account, no configuration file.

## Install

sipx is not on crates.io yet; install the CLI straight from the repository (you need a
[Rust toolchain](https://rustup.rs)):

```bash
cargo install --git https://github.com/codewandler/sipx sipx-cli
```

That builds the `sipx` binary. Check it:

```bash
sipx version
```

## Your first call

Terminal one — answer whatever calls, play a greeting, record what the caller says:

```bash
sipx answer --play greeting.wav --record caller.wav --once
```

Terminal two — call it:

```bash
sipx dial sip:you@127.0.0.1:5060 --play hello.wav --record reply.wav --duration 10
```

Both sides report what happened; `reply.wav` contains the greeting the answering side played.
WAV files are 8 kHz, 16-bit, mono — and if you have none lying around, both commands work
without `--play` (you will record silence, but the call is real).

## Register against a PBX

If you have a SIP account somewhere:

```bash
SIPX_PASSWORD='…' sipx register sip:alice@example.com --keep-alive
```

The registration is treated as a lease: sipx refreshes it before it expires, for as long as the
command runs. See [the guide](guides/register.md) for what is worth knowing.

## Scripting it

Every command speaks `--json` — one single-line JSON object on stdout — and returns a distinct
exit code per outcome (success, rejected, unauthorized, timeout, busy…), so a shell script can
branch on what actually happened:

```bash
if sipx dial sip:alice@example.com --timeout 15 --json > result.json; then
  echo "answered"
fi
```

The full command, flag and exit-code list is in the [CLI reference](reference/cli.md).

## Next

- [Place a call from Rust](guides/place-a-call.md) — the same call as a program.
- [Answer calls from Rust](guides/answer-a-call.md).
- [Does sipx fit?](guides/does-this-fit.md) — what it does and deliberately does not do.
- [The SDK preview](sdk/overview.md) — where call control without Rust is headed.
