# Adversarial supervised-worker protocol seed corpus

Seed inputs for the `worker_protocol_sequence` fuzz target (`M-122`), which drives
[`docs/specs/call-dsp-graph.md`](../../../../docs/specs/call-dsp-graph.md) §7.4's decoder in both
directions — the runtime reading a worker's `Result`s, and a worker reading the runtime's `Hello`
and `Frame`s — with a decoded program of message specifications rather than with bytes hoping to
land on the magic.

These files are **generated**, not written by hand. Each one is
`sipx_testkit::worker_protocol_sequence::seeds()` encoded two bytes of preamble plus four bytes
to the step, and each program reaches one row of §7.4's refusal table or one shape of a well-formed
exchange: bad magic, a set reserved octet, four undefined type octets, an undersized message of each
type, four oversized lengths including the two whose doubling saturates, a count that disagrees with
its length, truncation inside the header and inside the samples, every undefined field value, a
ceiling at the contract's bound and one past it, arbitrary chunking, and a lockstep call in each
direction.

**One refusal per seed, and it is the last message.** A refusal is terminal for the connection, so a
seed carrying two of them only ever exercises the first — the corpus would claim a coverage it does
not have, which is the failure a generated corpus exists to rule out. The test
`no_seed_hides_a_refusal_behind_another` in `crates/sipx-testkit/tests/` enforces it.

Regenerate with:

```sh
cargo run -p sipx-testkit --example dump_worker_protocol_sequences -- --write
```

Read them with the same example and no argument, which prints each exchange's trace and any
invariant it broke.

The corpus is committed test data and is proven unmodified two ways. The test
`the_committed_corpus_is_exactly_the_seed_programs` in `crates/sipx-testkit/tests/` fails if a file
here and `seeds()` disagree, and the `fuzz smoke` CI job runs `scripts/check-corpus-untouched.sh`
after the campaign — libFuzzer writes its finds to the *first* corpus directory it is given, so this
one is passed second, read-only.

No sample of any real call is here, and none could be: a seed is a program, and the octets it
describes are minted deterministically from a step and a table when the program runs.
