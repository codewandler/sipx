# sipx-audio

Telephony audio: G.711 µ-law and A-law, PCM mixing, WAV I/O, and Opus behind the `opus` feature.

## What this is

Sample-domain audio machinery for the rest of sipx: G.711 conversion, saturating PCM mixing, WAV
reading and writing, and the optional stateful Opus encoder and decoder.

## Stability

The supported and experimental surfaces are maintained in the
[crate-level Stability section](https://codewandler.github.io/sipx/api/sipx_audio/#stability).
That is the contract; it is linked rather than copied here so the two cannot drift.

## Deliberately absent

This crate does not resample, implement G.722, or carry telephone events. Telephone events are RTP
payloads and live in `sipx-rtp`; a caller must supply audio at the negotiated sample rate.

## See also

- [`sipx-rtp`](../sipx-rtp/README.md) — packets, telephone events, jitter, and SRTP.
- [`sipx-media`](../sipx-media/README.md) — sockets and paced media sessions.
