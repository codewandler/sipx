# RFC 7714 AES-GCM test vectors

These two files are sections **16** and **17** of **RFC 7714**, *AES-GCM Authenticated Encryption
in the Secure Real-time Transport Protocol (SRTP)* (McGrew and Igoe, IETF, December 2015), as the
RFC editor serves them.

They are **not transcribed**. RFC 7714 embeds no archive the way RFC 4475 and RFC 5118 do, so what
is imported is the document's own text: `scripts/import-rfc7714-corpus.sh` fetches the RFC, slices
the two sections between their headings, drops the running page header and footer and the form
feed between them, and writes the result here. No hex digit is retyped and no block is reflowed —
`crates/sipx-rtp/tests/rfc7714_vectors.rs` reads the labelled lines straight out of these files.

Why that matters more than usual: an AES-GCM SRTP transform whose IV formation or associated-data
boundary is wrong is still perfectly self-consistent. Two endpoints running the same wrong code
protect and unprotect each other's packets and every round-trip test passes. The RFC's own numbers
are the only thing that can tell that apart from a transform that interoperates — which is exactly
why a fixture nudged by hand to match a disagreeing implementation is the failure to guard against.

To verify the files still match the RFC:

```sh
./scripts/import-rfc7714-corpus.sh --check
```

Do not edit these files.
