# RFC 5118 IPv6 torture-test corpus

These 12 files are the SIP test messages published in **RFC 5118**, *Session Initiation Protocol
(SIP) Torture Test Messages for IPv6* (Gurbani et al., IETF, February 2008).

They are **not transcribed**. RFC 5118 Appendix A embeds a base64-encoded, gzip-compressed tar
archive of every message — "an encoded, gzip compressed TAR archive of files that represent each
of the example messages discussed in Section 4" — and
`scripts/import-rfc5118-corpus.sh` recovers it from the RFC text. Retyping matters more here than
for RFC 4475, not less: every case turns on the exact placement of `:`, `[` and `]`, and the RFC's
body text wraps two messages across lines with an `<allOneLine>` convention that a transcriber has
to unwrap by hand. The archived files are already unwrapped.

To verify the files still match the RFC:

```sh
./scripts/import-rfc5118-corpus.sh --check
```

Do not edit these files. The one deliberate malformation (`ipv6-bad`) is the test.

## Ten sections, twelve files

Two sections carry a contrast pair, which is why the counts differ:

| Section | Files |
| --- | --- |
| 4.1 Valid SIP Message with an IPv6 Reference | `ipv6-good` |
| 4.2 Invalid SIP Message with an IPv6 Reference | `ipv6-bad` |
| 4.3 Port Ambiguous in a SIP URI | `port-ambiguous` |
| 4.4 Port Unambiguous in a SIP URI | `port-unambiguous` |
| 4.5 IPv6 Reference Delimiters in Via Header | `via-received-param-with-delim`, `via-received-param-no-delim` |
| 4.6 IPv6 Addresses in an SDP Body | `ipv6-in-sdp` |
| 4.7 Multiple IP Addresses in SIP Headers | `mult-ip-in-header` |
| 4.8 Multiple IP Addresses in SDP | `mult-ip-in-sdp` |
| 4.9 IPv4-Mapped IPv6 Addresses | `ipv4-mapped-ipv6` |
| 4.10 IPv6 Reference Bug in RFC 3261 ABNF | `ipv6-bug-abnf-3-colons`, `ipv6-correct-abnf-2-colons` |

The file names are the archive's own, without an added extension: RFC 5118 labels each message
with that name ("Message Details: ipv6-good"), so the name is the link from a fixture back to the
prose describing it.

## These files are not wire bytes

Unlike RFC 4475's archive, **RFC 5118's files are terminated with bare LF** — there is not one CR
octet in any of the twelve — and the two §4.10 files carry no terminating blank line at all. SIP
requires CRLF (RFC 3261 §7), so the archived bytes are not legal SIP messages as shipped.

They are kept bit-exact regardless, because that is what `--check` verifies against the RFC.
`Case::wire()` in `crates/sipx-testkit/src/rfc5118.rs` applies the one documented transformation
that yields on-the-wire bytes — LF becomes CRLF, and an unterminated header section gets its blank
line — and a test there asserts that the transformation changes nothing but line terminators.

## What is asserted about them

The classification — which layer must object to each message, and how — lives in
`crates/sipx-testkit/src/rfc5118.rs`, next to the reasoning, and reuses the `Expect` vocabulary
that `crates/sipx-testkit/src/rfc4475.rs` defines so the two corpora can be read side by side.

Only §4.2 is a rejection. The other eleven messages are the RFC's demonstrations that a parser
must accept constructs it may not expect, so the load-bearing assertion for this corpus is the
converse one: that nothing valid is refused.

The assertions themselves are split across the two layers the corpus reaches:

- `crates/sipx-sip/tests/rfc5118_corpus.rs` — parsing, re-serialization, Via and URI handling.
- `crates/sipx-sdp/tests/rfc5118_sdp.rs` — the `o=` and `c=` lines of §4.6, §4.8 and §4.9.
