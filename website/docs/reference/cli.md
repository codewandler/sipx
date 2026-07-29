---
title: CLI reference
description: Every sipx command, flag, exit code and JSON field — the surface a shell script can rely on.
---

# CLI reference

One binary, `sipx`. Four commands do work — `dial`, `answer`, `register` and `peers`, documented
below — alongside `help` and `version`. Global: `--json` switches the report to a single-line JSON
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

## The JSON contract

`--json` emits exactly one JSON object per report, on one line, on stdout — a command with more
than one thing to report, such as `answer`'s bound address or `peers`' list, emits one such line
each; failures emit
`{"status": …, "error": …}` on **stderr**. The text and JSON forms carry the same field set —
that equality is asserted by a test, so a field you see in one is in the other.
