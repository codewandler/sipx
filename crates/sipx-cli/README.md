# sipx-cli

`sipx` — a command line SIP softphone.

## What this is

A scriptable diagnostic phone for registering, placing, answering, and inspecting calls. It also
provides a finite, machine-ready `load-responder` with explicit admission, dialog-lifetime and
cleanup bounds for reproducible signalling measurements. Its JSON output, exit statuses, and
separation of logs from stdout make the call outcome assertable from a shell.

WAV input is mono 16-bit PCM with an explicit header rate. The phone linearly resamples supported
rates to the negotiated media clock—8 kHz for G.711, 44.1 or 8 kHz for L16, and 48 kHz for Opus—
takes packet size from the running session in both call roles, and writes that clock into recording
headers.

## Stability

The supported command-line contract is maintained in the
[binary documentation's Stability section](https://codewandler.github.io/sipx/api/sipx/#stability).
That is the contract; it is linked rather than copied here so the two cannot drift.

## Deliberately absent

This package has no Rust library target. Applications embedding sipx should depend on the relevant
library crates instead of treating command internals as an API.

## See also

- [CLI reference](https://codewandler.github.io/sipx/docs/reference/cli) — commands, output, and
  exit statuses.
- [`sipx-call`](../sipx-call/README.md) — the Rust call framework used by the phone.
