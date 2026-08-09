# Adversarial call-audio seed corpus

Seed inputs for the `call_audio_sequence` fuzz target (`M-61`), which drives `sipx-audio`'s
`AudioAnalyzer` and `SignalReporter` with a decoded program of events — frames of hostile audio in
every supported PCM format, declared format changes, requested resets, sequence gaps and replays,
and a consumer that stops draining — rather than with bytes reinterpreted as samples.

These files are **generated**, not written by hand. Each one is
`sipx_testkit::call_audio_sequence::seeds()` encoded four bytes to the event, and each program is
one shape `M-61`'s Acceptance names: extreme amplitudes, impulses, DC, alternating full swing,
arbitrary chunking, sequence gaps, format changes, long silence, permanent activity and event
backpressure. Seeding from them is the same trick CI plays on the parser targets with the RFC 4475
corpus: the campaign starts from input that reaches every predicate in
`docs/specs/call-audio-processing.md` §5.3 and every freeze condition in its §12.6, and mutates
outwards, instead of spending its first minutes rediscovering that a frame has to carry samples.

Regenerate with:

```sh
cargo run -p sipx-testkit --example dump_call_audio_sequences -- --write
```

Read them with the same example and no argument, which prints each program's trace and any
invariant it broke.

The corpus is committed test data and is proven unmodified two ways. The test
`the_committed_corpus_is_exactly_the_seed_programs` in `crates/sipx-audio/tests/` fails if a file
here and `seeds()` disagree, and the `fuzz smoke` CI job checks the directory is untouched after
the campaign — libFuzzer writes its finds to the *first* corpus directory it is given, so this one
is passed second, read-only.

No sample of any real call is here, and none could be: a seed is a program, and the audio it
describes is minted deterministically from a pattern and an amplitude when the program runs.
