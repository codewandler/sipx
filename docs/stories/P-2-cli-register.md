---
id: P-2
title: Implement `sipx register`
pillar: Phone
status: done
priority: 2
design: docs/designs/phone.md
epic: cli
areas: [sipx-cli]
note:
---

# Implement `sipx register`

## Goal
Register against a registrar from the command line, and stay registered.

## Acceptance
- [x] `sipx register sip:user@domain --password X` registers and reports the granted lease.
- [x] `--keep-alive` refreshes until interrupted; without it the command registers once and
      exits.
- [x] A wrong password exits with the authentication code and says so, rather than retrying.
- [x] Credentials can come from the environment rather than the command line, since a password
      in `argv` is visible to every process on the machine.
- [x] Failing-first test: `register_reports_the_granted_lease`, against the Kamailio fixture.

## Progress
- Done. `sipx register`, with `--keep-alive` for refreshing.
- The password is read from `SIPX_PASSWORD` as well as `--password`, because argv is readable
  by every process on the machine.
- A domain with nothing to resolve it says to pass `--target`, rather than failing later in a
  way that looks like a network problem. Wiring the DNS resolver from `T-5` into the command
  is a small follow-up.
