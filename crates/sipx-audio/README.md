# sipx-audio

Telephony audio: G.711 µ-law and A-law, G.722, L16, linear PCM conversion and resampling, WAV I/O, and Opus behind the `opus` feature.

## What this is

Sample-domain audio machinery for the rest of sipx: G.711 and network-order L16 conversion, the
native G.722 wideband codec, explicit unsigned-8 and signed-16 PCM conversion with linear
resampling, WAV reading and writing, and the optional stateful Opus encoder and decoder.

Opus is off by default and crosses a native-library boundary. The deployment and advisory policy is
documented in the public
[library guide](https://codewandler.github.io/sipx/docs/guides/as-a-library#opus-packaging-policy).

## Stability

The supported and experimental surfaces are maintained in the
[crate-level Stability section](https://codewandler.github.io/sipx/api/sipx_audio/#stability).
That is the contract; it is linked rather than copied here so the two cannot drift.

## Format boundary

`PcmFormat` makes mono sample depth and rate explicit, and `LinearResampler` converts between rates
from 1 through 384,000 Hz. G.722 is implemented natively from the ITU-T recommendation and verified
against its official test sequences. Telephone events are RTP payloads and live in `sipx-rtp`
rather than being linear samples.

## See also

- [`sipx-rtp`](../sipx-rtp/README.md) — packets, telephone events, jitter, and SRTP.
- [`sipx-media`](../sipx-media/README.md) — sockets and paced media sessions.
