# RFC 4475 torture-test corpus

These 50 files are the SIP test messages published in **RFC 4475**, *Session Initiation
Protocol (SIP) Torture Test Messages* (Sparks et al., IETF, May 2006).

They are **not transcribed**. RFC 4475 Appendix A embeds a base64-encoded, gzip-compressed tar
archive of every message precisely so implementers get them bit-exactly, and
`scripts/import-rfc4475-corpus.sh` recovers that archive from the RFC text. Retyping them from
the rendered document would corrupt several cases, which is the whole point of the archive:
`escnull` carries `%00` escapes, `intmeth` carries UTF-8 inside a display name, and `trws`
turns on a trailing space that no editor preserves.

To verify the files still match the RFC:

```sh
./scripts/import-rfc4475-corpus.sh --check
```

Do not edit these files. A deliberate malformation is the test.

## What is asserted about them

The classification — which layer must object to each message, and how — lives in
`crates/sipx-testkit/src/rfc4475.rs`, next to the reasoning. `test.dat` is present in the
archive but referenced by no section of the RFC; it is carried so the corpus is a faithful
copy, and asserted on by nothing.
