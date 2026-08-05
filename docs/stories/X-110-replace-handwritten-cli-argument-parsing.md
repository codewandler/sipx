---
id: X-110
title: "Replace handwritten CLI argument parsing"
pillar: "Build"
status: in-progress
epic: diagnostic-automation
areas: [sipx-cli, cli]
design: docs/designs/diagnostic-automation.md
note: "external review findings 4, 7 and 12 · replace the custom Args scanner and flag registries with one typed parser; preserve the shipped CLI contract"
priority: 1
---

# Replace handwritten CLI argument parsing

## Goal

Replace `sipx-cli`'s custom raw-argument scanner and its hand-maintained flag registries with one
declarative, typed command model, so adding an option cannot silently create a second parser or
forget a global list.

## Acceptance

- [ ] One declarative parser owns the root command, every shipped subcommand, global output and
      verbosity options, positional arguments, repeated options, defaults, conflicts and value
      validation. Command implementations receive typed command values rather than raw argv or a
      string lookup facade.
- [ ] The custom `Args` type, `arguments`, `wants_help`, `VALUED_FLAGS`, `NUMERIC_FLAGS`, raw command
      dispatch and every production `std::env::args`/`args_os` read are removed from `sipx-cli`.
      A focused repository check fails if any of those manual-parser shapes return.
- [ ] The shipped command contract is preserved deliberately: command and option names, aliases,
      repeatability and ordering, environment fallbacks, defaults, help/version success, unknown or
      malformed input refusal, JSON/text result separation, verbosity semantics and exit codes.
- [ ] Values are parsed once into their semantic types where the parser can own the rule; remaining
      cross-field and protocol-policy validation is named in the command layer and does not rescan
      argv. Non-Unicode input is refused as a usage error rather than panicking before `main`.
- [ ] Failing-first tests cover at least a missing value, a value that begins with `-`, a repeated
      ordered option, a conflicting option pair, clustered verbosity, help mixed with malformed
      input, a non-Unicode argument on Unix and one option shared across multiple commands.
- [ ] The generated CLI reference and public examples are synchronized from the parser-owned help;
      no second hand-written command/flag inventory remains in docs or tests.
- [ ] Focused CLI tests, no-default/all-feature builds, strict Clippy, docs synchronization and the
      complete repository gate are green.
