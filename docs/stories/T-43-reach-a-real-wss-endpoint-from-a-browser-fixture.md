---
id: T-43
title: Reach a real WSS endpoint from a browser fixture
pillar: Transport
status: ready
priority: 24
design: docs/specs/browser-signalling.md
epic: browser-sdk
areas: [browser, websocket, wss, ci]
predicate:
announcement:
note: T-33's last acceptance row · needs a hosted runner's browser and WebDriver
---

# Reach a real WSS endpoint from a browser fixture

## Goal

Prove in a real browser, against a real WSS endpoint, that the signalling binding registers over
TLS — the one row of `T-33` that cannot be evidenced on a developer's machine.

## Acceptance

- [ ] A fixture page loads `browser/src/` and the compiled kernel and registers against a WSS
      endpoint served with an ephemeral certificate, driven by WebDriver in the pattern
      `tests/browser-audio/` established: `SIPX_*` environment injection, an SPKI pin derived at
      run time, no fixed port, and a process-group cleanup trap.
- [ ] Evidence is structured JSON validated by a separate command, not scraped from stdout, and is
      uploaded as a CI artifact with `if-no-files-found: error`.
- [ ] An adversarial self-test of the harness runs in the local gate — the validator is shown to
      reject a mutated evidence document, including one where the registration never happened.
- [ ] The negative cases are covered from the browser: a `wss:` endpoint that selects no
      subprotocol, and a `ws:` URL under the default policy, both fail closed and typed.
- [ ] The CI job is registered in `scripts/gate.py`'s `NOT_RUN_LOCALLY` with the reason, so adding
      it forces the decision rather than hiding it.
- [ ] `T-33`'s sixth acceptance row is ticked and its `## Progress` points here.

## Progress

- Backlog. Split out of `T-33` on 2026-08-08.

## Notes

`T-33` delivered the binding and evidenced every row but its last. What it could produce locally:
44 cases against injected fakes, and 4 more driving the compiled `sipx_browser_wasm.wasm` through
the binding — including that the bytes reaching the socket are the kernel's serialisation and that
the credential is on no wire, in no URL and in no event. What it could not produce is a **real
browser** talking to a **real WSS endpoint**, which needs a matched browser and WebDriver and a TLS
listener.

That is the same constraint `browser-audio` lives under, and `scripts/gate.py` already records the
precedent verbatim: *"requires the hosted runner's matched native browser/WebDriver; the local gate
runs its adversarial harness suite"*. `docs/specs/browser-audio-proof.md` and
`tests/browser-audio/` are the pattern to copy, down to the two-claims rule — the real run and the
harness self-test are different claims and both must pass.

Deliberately narrower than `X-100`. `X-100` proves the **packaged** SDK and demo across the browser
matrix once `A-17` and `A-18` exist. This proves one thing much earlier: that the transport `T-33`
shipped reaches a real TLS endpoint from a real browser at all. If `X-100` lands first and absorbs
it, close this as superseded rather than doing it twice.
