# Design: Phone CLI

**Status:** outline · **Pillar:** Application · **Epic:** `phone` · **Stories:** _to be cut_

## Why

The phone is both the product's front door and its most demanding integration test. Vision
principle 6 says a feature that cannot be asserted on from a script is not finished — the CLI
is where that principle is cashed in, because a shell script that places a call, sends DTMF,
records the answer and checks the samples exercises every layer at once.

## Approach

_To be written when the epic starts. In outline: `sipx dial | answer | register | loadtest`;
media sourced from and sunk to files, devices, generators or a log; DTMF with configurable
timing; custom headers and response code override for testing servers; results emitted as
structured records so a script can assert on them; load testing with ramped call rates and
alarm thresholds on RTP and SIP metrics._

## Alternatives considered

- **An interactive TUI first.** Rejected for now: scriptability is the north star for this
  epic, and an interactive mode is easy to add on top of a scriptable core, not the reverse.

## Risks & open questions

- Audio device access pulls in a platform dependency; it should be feature-gated so the binary
  builds and its file-based modes work with no audio stack present.
- What the structured output schema is, and how stable it needs to be — scripts will depend on
  it immediately.

## Acceptance / done

A shell script places a call to a third-party server, sends DTMF, records the far end and
verifies the recording, using only documented CLI output.
