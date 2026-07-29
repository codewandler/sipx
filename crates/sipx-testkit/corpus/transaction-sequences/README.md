# Transaction-sequence seed corpus

Seed inputs for the `transaction_sequence` fuzz target, which drives `TransactionLayer` with a
decoded program of events — incoming messages, application requests and fired timers — rather
than with bytes reinterpreted as SIP.

These files are **generated**, not written by hand. Each one is
`sipx_testkit::transaction_sequence::seeds()` encoded four bytes to the event, and each program
is one of the scenarios in `docs/specs/sip-transaction.md` §7 — the rows the FSM table tests
already walk. Seeding from them is the same trick CI plays on the parser targets by seeding
them with the RFC 4475 corpus: the campaign starts from behaviour that reaches every state
machine and mutates outwards, instead of spending its first minutes rediscovering that a
response has to follow a request.

Regenerate with:

```sh
cargo run -p sipx-testkit --example dump_sequences -- --write
```

Read them with the same example and no argument, which prints each program's trace.

The corpus is committed test data and is proven unmodified two ways. The test
`the_committed_corpus_is_exactly_the_seed_programs` in `crates/sipx-sip/tests/` fails if a file
here and `seeds()` disagree, and the `fuzz smoke` CI job checks the directory is untouched after
the campaign — libFuzzer writes its finds to the *first* corpus directory it is given, so this
one is passed second, read-only.
