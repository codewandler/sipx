---
id: S-27
title: Refuse a `sips:` URI the CLI cannot honour securely, instead of sending it in the clear
pillar: Signalling
status: done
design: docs/designs/sip-transport.md
epic: conformance
areas: [sipx-cli]
note: found by X-33's implementor — dial.rs:231 strips `sips:` exactly as `sip:` and defaults to port 5060, and dial.rs:49 only ever chooses UDP or TCP, so `sipx dial sips:…` sends the INVITE in cleartext and says nothing
---

# Refuse a `sips:` URI the CLI cannot dial securely, instead of dialling it in the clear

## Goal
Stop `sipx dial sips:alice@host` from placing an unencrypted call. Either dial it over TLS, or refuse
it — but do not silently downgrade a URI whose entire meaning is "TLS on every hop".

## Acceptance
- [x] **`sipx dial` never sends a `sips:` INVITE over UDP or TCP.** Verified today:
      `crates/sipx-cli/src/dial.rs:228-240` (`target_of`) strips `sips:` in the same
      `or_else` as `sip:`, discards the distinction entirely, and defaults to **port 5060** —
      not 5061 — while `dial.rs:49` selects the transport from one flag: `if args.flag("tcp")`.
      There is no TLS branch in the dial path at all.
- [x] **Refusing is an acceptable answer and is probably the right first one.** RFC 3261 §26.2.2
      requires TLS on every hop for a `sips:` URI; §19.1.1 makes that the URI's meaning rather than a
      hint. A clear refusal naming the missing capability is honest. A downgrade is not, and it is the
      failure mode a user cannot see.
- [x] If it refuses, the exit code and message follow the CLI's existing vocabulary
      (`crates/sipx-cli/src/output.rs`'s `Exit`), and the message says *why* — that the CLI has no TLS
      transport, not that the URI is malformed. It is not malformed.
- [x] **`register` is the model, not a second thing to build.**
      `crates/sipx-cli/src/register.rs:234` already exercises `resolve_target(..., TransportKind::Tls)`
      and `register.rs:144` does the same `sips:` strip — so registration can do TLS and dial cannot.
      Check whether `register` *also* downgrades, and say so either way: the same `strip_prefix`
      shape is there, and it having a `Tls` kind available does not prove it uses it for `sips:`.
- [~] The default port for `sips:` is **5061**, not 5060 (RFC 3261 §19.1.2). Whether or not TLS lands,
      this is wrong today and silently sends to the wrong port for any peer that follows the RFC.
- [x] Failing-first test: `target_of("sips:bob@192.0.2.1")` returns `192.0.2.1:5060` today, and the
      dial path builds a UDP target from it. Name the test that fails on that.

## Progress
- **Done, and it was two defects rather than one.** `register` has the identical shape and I nearly
  shipped a fix that covered only `dial`: `register.rs:58-61` selects the transport from `--tcp`
  alone exactly as `dial.rs:49` does, and `register.rs:144` strips `sips:` exactly as `target_of`
  does. `TransportKind::Tls` appears in that file only inside a **test** of `resolve_target`
  (`register.rs:234`), never in the path a command takes — which is precisely why reading "register
  can do TLS" off a grep was wrong. **On `register` the downgrade costs more than on `dial`**,
  because what travels in the clear is a digest credential.
- The refusal therefore lives in `main.rs` as `insecure_scheme_refusal`, not in either command.
  Putting it in one of them is how the second one came to be missed in the first place.
- **Failing-first, by mutation rather than by argument.** With the call-site disabled and the test
  otherwise unchanged, both tests fail `left: 5, right: 2` — `Exit::Timeout` instead of
  `Exit::Usage` — and the runs take **22 s** and **32 s** respectively, because the command really
  does attempt the cleartext send. That wall-clock is the defect being demonstrated, not a slow test.
- **My first version of the behavioural test was vacuous, and the mutation caught it.** It passed the
  URI as the only element of the argument slice, but `Args::positional` skips index 0 as the
  subcommand name (`main.rs:137-139`), so there was no positional at all and the test was asserting
  the "a URI to call is required" path — which returns `Exit::Usage` whether or not the scheme is ever
  checked. It passed with the fix disabled. The corrected tests pass `["dial", …]` and
  `["register", …]`, and the reason is written at both sites so the next person does not repeat it.
- **Port 5061 is left, deliberately, and marked `[~]`.** `target_of` still defaults a `sips:` URI to
  port 5060, but no command can reach that code with a `sips:` URI any more — both refuse first. The
  remaining wrongness is unreachable, and fixing it would mean writing the port for a transport this
  CLI does not have. It becomes real the moment a `--tls` flag exists, and belongs with that work.
  A test pins the current behaviour so the refusal is what protects the caller, not luck. Found by `X-33`'s implementor as an adjacent observation while reading `sipx-cli` for
  the reachability check, and **verified independently at integration** — I read `target_of` and the
  transport selection myself rather than filing on the report.

## Notes
- **This is the most serious defect found on 2026-07-29, and it is a shipped path**, not a registry
  claim or a doc over-claim. Everything else found that day was sipx being *wrong about itself*; this
  is sipx doing the wrong thing quietly. `sips:` is the one piece of SIP syntax whose whole purpose is
  to say "do not send this in the clear".
- **Alpha predicate 4** is *"no known-wrong shipped path"*. This is now one, so the predicate cannot be
  met until this story closes or the behaviour is deliberately documented — and documenting a silent
  security downgrade is not a real option.
- **It compounds a claim `X-35` just corrected.** `X-35` added a warning to
  `website/docs/reference/cli.md` that the CLI dials over UDP or TCP only, so encrypted media needs the
  library. That warning is true and now published — but it describes the *absence* of a feature, while
  this story is about the CLI accepting a URI that demands the feature and proceeding anyway. The doc
  fix does not cover the behaviour.
- Priority 1. It is small, it is verified, it is security-relevant, and the honest minimum — refuse —
  requires no new transport work.
- Adjacent, and probably one story with this one: **`sipx dial --password` is accepted and discarded.**
  `crates/sipx-cli/src/main.rs:168` lists `--password` among the valued flags and `main.rs:193` tests
  `["dial", "--password", "secret", "sip:a@b"]` parsing, but `crates/sipx-cli/src/dial.rs` never reads
  it — only `register.rs:53` does. So a call challenged with 407 fails instead of authenticating, and
  the user who supplied a password is told nothing. Filed as `P-7`.
