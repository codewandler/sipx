# Design: Diagnostic phone

**Status:** accepted · **Pillar:** Application · **Epic:** `phone` ·
**Stories:** `P-1` … `P-4`, `P-7` … `P-13` ·
**Spec:** [diagnostic-phone](../specs/diagnostic-phone.md)

## Why

The phone is both the product's front door and its most demanding integration test. Vision
principle 6 says a feature that cannot be asserted from a script is not finished. The existing
binary has a strong WAV-oriented shell contract, but it cannot select every transport, codec,
security or NAT path already present in the libraries, cannot use a sound device, and exposes no
bounded call-load command.

This epic closes that reachability gap without turning the binary into a desktop softphone. The
product is a diagnostic endpoint: deterministic when driven from files or generators, usable by a
person through a sound device, and machine-readable in both cases.

## Approach

[`diagnostic-phone.md`](../specs/diagnostic-phone.md) is the normative command and event contract.
The implementation is one call-control path with interchangeable drivers:

- signalling selects UDP, TCP, TLS, WS or WSS through the existing transport layer;
- media selects an ordered codec set, media-security policy and ICE policy through one call-level
  policy rather than command-specific booleans;
- sources and sinks are files, devices, deterministic generators or null endpoints;
- interactive automation is newline-delimited JSON with correlated commands and events; and
- load generation reuses the bounded testkit model and always has a finite call or time budget.

Platform audio dependencies remain feature-gated in `sipx-cli`. They never enter `sipx-audio`,
`sipx-media`, `sipx-sip` or `sipx-sdp`. The file-only binary continues to build without a system
audio stack.

## Decisions

- **Script protocol, not a TUI.** NDJSON is composable from a terminal, a pipe or a test harness and
  does not introduce a second state model.
- **Selection is explicit and fail closed.** A requested transport, codec or security mode either
  becomes the negotiated path or produces a typed error. There is no silent downgrade.
- **No sleeps in scenarios.** A scenario waits for a named event with a deadline. Wall-clock sleeps
  cannot stand in for a causal signal.
- **Custom does not mean transaction-owned.** A caller may add validated end-to-end or extension
  headers, but cannot override Via, route-set, dialog-identity, sequence or framing fields owned by
  the stack.
- **Transcription is out.** It introduces model, privacy and service policy unrelated to proving a
  SIP/media path.

## Risks

- Device enumeration and timing differ by platform. The coordinated operational baseline publishes
  Linux binaries and compile-checks the device feature on macOS and Windows; deterministic release
  proofs use a virtual loopback device rather than a human microphone.
- Scripts will depend on the event schema immediately. The existing versioned JSON envelope and
  additive evolution rules apply; the interactive mode does not invent a second schema.
- A load tool can become an accidental denial-of-service tool. Concurrency, rate and total work are
  explicit, finite and validated before the first call starts. Admission is the existing
  `sipx-testkit` paced scheduler; a shared stop signal closes admission and asks every owned call to
  end, and the command joins those calls under the specification's finite cleanup budget before it
  emits its sole summary.

## Acceptance / done

The union of `P-8` … `P-13`: one documented shell scenario selects every released signalling
transport, G.711 or Opus, plain RTP or a supported SRTP keying, ICE where configured, WAV or live
device audio, interactive in-call actions and bounded load; the structured result proves what was
selected and what actually negotiated.
