---
title: CLI reference
description: Every sipx command, flag, exit code and JSON field — the surface a shell script can rely on.
---

# CLI reference

One binary, `sipx`, three commands. Global: `--json` switches the report to a single-line JSON
object on stdout; `-v`/`-vv` raise log verbosity on stderr (never stdout, so JSON stays
parseable); `-h`/`--help` on any command.

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
| `--tcp` | Use TCP rather than UDP |
| `--stats` | Report call quality on exit: loss, jitter, round trip, MOS estimate |

Report fields: `status`, `peer`, `duration_ms`, `samples_recorded`, `heard_audio` — plus
`recording` when `--record` was given, and `loss`, `packets_lost`, `jitter_ms`, `mos`,
`round_trip_ms` under `--stats`.

## `sipx answer`

Wait for a call and answer it: `sipx answer --play greeting.wav`

| Flag | Meaning |
|---|---|
| `--play <FILE>` | Play this WAV to the caller (8 kHz 16-bit mono) |
| `--record <FILE>` | Record the caller to this WAV |
| `--duration <S>` | Hang up after this many seconds (default 30) |
| `--wait <S>` | Give up if no call arrives within this many seconds (default 60) |
| `--local <ADDR>` | Local address to bind (default `0.0.0.0:5060`) |
| `--reject` | Answer 603 Decline instead |
| `--busy` | Answer 486 Busy Here instead |
| `--once` | Exit after one call (the default; kept for clarity in scripts) |

Reports twice: `status: "listening"` with the bound `address` first, then
`status: "answered"` with `caller`, `duration_ms`, `samples_recorded`, `heard_audio` — plus
`dtmf` when digits arrived and `recording` when `--record` was given.

## `sipx register <AOR>`

Register with a registrar: `sipx register sip:alice@example.com`

| Flag | Meaning |
|---|---|
| `--password <P>` | Password. Prefer the `SIPX_PASSWORD` environment variable — argv is world-readable |
| `--target <ADDR>` | Where to send, if not derived from the AOR (`host:port`) |
| `--expires <S>` | Lease to ask for, in seconds (default 3600) |
| `--local <ADDR>` | Local address to bind (default `0.0.0.0:0`) |
| `--tcp` | Use TCP rather than UDP |
| `--keep-alive` | Keep refreshing until interrupted |

Report fields: `status`, `aor`, `expires`, `refresh_in`.

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

## The JSON contract

`--json` emits exactly one JSON object per report, on one line, on stdout; failures emit
`{"status": …, "error": …}` on **stderr**. The text and JSON forms carry the same field set —
that equality is asserted by a test, so a field you see in one is in the other.
