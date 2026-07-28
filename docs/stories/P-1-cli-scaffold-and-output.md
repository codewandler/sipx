---
id: P-1
title: Build the CLI scaffold and machine-readable output
pillar: Phone
status: done
priority: 1
design: docs/designs/cli.md
epic: cli
areas: [sipx-cli]
note:
---

# Build the CLI scaffold and machine-readable output

## Goal
Give `sipx` its command structure and an output contract a script can rely on, before any
command does anything, so every later command inherits both.

## Acceptance
- [x] `sipx` parses subcommands with `--help` for each, and a `--version` that reports the
      crate version.
- [x] Every command emits machine-readable output on `--json`, and human-readable output
      otherwise. The two carry the same facts.
- [x] Exit codes are documented and distinct: success, call rejected, authentication failed,
      timeout, and usage error are told apart by a script without parsing text.
- [x] Errors go to stderr and results to stdout, so `sipx ... | jq` works while errors stay
      visible.
- [x] `-v`/`-vv` control logging, and logging never goes to stdout where it would corrupt the
      JSON.
- [x] Failing-first test: `json_output_is_parseable_and_carries_the_same_facts_as_the_text`.

## Progress
- Done. `crates/sipx-cli/src/output.rs` and the argument parsing in `main.rs`.
- Three rules drive it, each because breaking it makes the tool unusable in a pipeline:
  results on stdout and everything else on stderr; logging never on stdout, since one stray
  line turns valid JSON into a parse error where the cause is invisible; and the two formats
  carrying the same facts, because otherwise whichever a person reads is the one they believe.
- Values are escaped on the way out. A reason phrase comes off the network and can contain a
  quote, a newline or a control character — this is the boundary where untrusted text becomes
  something a script will parse.
- Exit codes are distinct per outcome, so busy and no-answer can be told apart without
  matching on English.
