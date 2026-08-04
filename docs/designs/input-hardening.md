# Design: input hardening

**Status:** accepted · **Pillar:** Build · **Epic:** `input-hardening` · **Stories:** X-64, X-65

## Why

sipx already refuses the malformed-input classes that most often take a SIP stack down: parser
bounds are checked before allocation (`crates/sipx-sip/src/parser.rs:18-35`), `Header::build` is
fallible so a CR or LF cannot reach the wire (`crates/sipx-sip/src/build.rs:24-63`), and five fuzz
targets run against both the datagram and the stream parser.

Every one of those properties is currently asserted by *design* and sampled by a fuzzer. None is
pinned by a named test that fails if the property is removed. That is the `X-36` shape: it looks
like coverage and is not. A refactor that moved the `Content-Length` check one line after the
allocation would leave the gate green, because nothing in the suite asks that specific question.

Three input classes are worth pinning by name, because they are the ones that recur across
independent SIP implementations rather than being particular to any design:

1. **A request missing a header that response construction assumes.** Building a response reads
   `To`, `From`, `Call-ID`, `CSeq` and the `Via` stack (RFC 3261 §8.2.6.1). A request that framed
   correctly but carries none of them must produce a typed refusal, never a panic.
2. **An allocation sized from a length the peer declared.** `Content-Length` on datagram and
   stream framing, and the WebSocket frame length on WS and WSS (RFC 7118, RFC 6455 §5.2), are all
   attacker-controlled integers that precede a buffer. Each must be bounded *before* the buffer
   exists, on every framing path independently — one path being safe says nothing about the others.
3. **A declared length that disagrees with the bytes that follow.** Short and long bodies on every
   framing path resolve to a typed error or a bounded wait, never a hang and never a read past the
   frame.

The Via branch and tag RNG (`docs/specs/sip-transport.md:110`) is the same shape: the spec says
cryptographic because a guessable branch lets an off-path attacker inject responses, and nothing
fails if someone swaps in a cheaper generator.

## Approach

- One regression suite per class, in `crates/sipx-sip/tests/` and `crates/sipx-transport/tests/`,
  each test named for the property it pins and carrying the RFC citation in a comment on the test.
- The allocation-bound class is **parameterised over every framing path** — UDP datagram, TCP/TLS
  stream, WS and WSS frame — from one table, so adding a transport that forgets the bound fails a
  test that already exists rather than needing a new one written.
- Assertions are on the typed error and on a bound that holds, not merely on "did not panic": a
  test that only proves absence of a crash passes against a stack that allocated 2 GiB first.
- The RNG property is asserted statistically over a large sample of generated branches and by
  construction (the generator's type), not by reading the call site.
- Failing-first for each: the test is written against the current tree and must be shown to fail
  when the corresponding bound is removed in a scratch edit, and that demonstration is recorded in
  the story's Progress log.

## Alternatives considered

- **Leave it to the fuzzer.** Rejected: a fuzzer samples, and the corpus that found a case is not
  the corpus that runs on the next commit. Fuzzing finds unknown inputs; a regression test pins a
  known property. They are not substitutes, and `transaction_sequence` already covers the former.
- **One "malformed input" test module.** Rejected because a failure has to name which property
  broke. Three classes, three suites, one reason each.
- **Assert the RNG by reviewing the call site.** Rejected for the reason the whole epic exists: a
  property nothing executes is a comment.

## Risks and open questions

- The allocation-bound table needs a seam that lets a test drive each framing path without a real
  socket. `sipx-testkit`'s in-process link covers the stream and datagram paths; whether WS and WSS
  can be driven at the same level without duplicating frame assembly is the open question, and the
  fallback is a per-transport test that shares its assertions through a helper.
- Statistical RNG assertions are inherently flaky at some threshold. The test must choose a bound
  whose false-failure rate is negligible over the life of the project and state the arithmetic in a
  comment on the line, or it becomes a retry.
