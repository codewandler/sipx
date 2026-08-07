# ITU-T G.722 Appendix II digital test sequences

The recommendation's own conformance vectors for the 64 kbit/s sub-band ADPCM codec, in the
16-bit **big-endian** binary form the ITU publishes. Recovered from the ITU test-signal archive
by `scripts/import-g722-corpus.sh`; run it with `--check` to verify these bytes against the
archive. Never edit these files by hand — the codec is verified against them bit-exactly, so an
edited vector is a test that proves nothing.

Word format (see `docs/specs/g722.md` §4):

- bit 0 set marks a codec state reset;
- an input sample (`.xmt`) occupies bits 15..1 — shift right one to use;
- a code word (`.cod`) carries the lower-band code in bits 13..8 and the higher-band code in
  bits 15..14;
- a reference output (`.rc*`) is the decoder's band output shifted left one.

The sequences drive the two band coders directly, with the QMF pair bypassed. `bt1c1.xmt` and
`bt1c2.xmt` must encode to `bt2r1.cod` and `bt2r2.cod`; each `.cod` file (plus the decoder-only
`bt1d3.cod`) must decode to the matching `bt3l⟨n⟩.rc⟨mode⟩` lower-band output for each of the
three operating modes and the `bt3h⟨n⟩.rc0` higher-band output. `crates/sipx-audio/src/g722.rs`
runs all eleven comparisons in its tests.
