# Linear PCM application boundary

**Status:** normative · **Story:** M-43 · **Crates:** `sipx-audio`, `sipx-media`, `sipx-call`,
`sipx-cli` · **RFCs:** 3264, 3551

## 1. Boundary and supported formats

Codec packets are not an application audio API. The application boundary is owned mono linear PCM
with an explicit sample rate and one of two byte depths:

| Encoding | Rust representation | Zero | Full-scale conversion |
|---|---|---|---|
| unsigned 8-bit | `Vec<u8>` | 128 | `(sample - 128) << 8` |
| signed 16-bit | `Vec<i16>` | 0 | unchanged |

`PcmFormat` contains the sample rate and encoding; `Pcm` contains that format and samples of the
matching representation. Rates from 1 through 384,000 Hz are supported. Zero and larger rates are
typed refusals before allocation. Conversion clips at the destination range and never wraps.

The public type is owned deliberately. Playback is queued beyond the method call, and a borrowed raw
byte slice would either be copied behind an API that implied otherwise or tied to a worker lifetime.
The enum also makes an 8-bit buffer impossible to reinterpret as pairs of 16-bit samples: depth is a
type choice, not a number passed beside untyped bytes.

## 2. Resampling

`sipx-audio::LinearResampler` performs streaming linear interpolation between any two supported
rates. It retains the previous sample and rational source position across `push_i16` calls, so
packet boundaries do not create discontinuities or cumulative integer drift. Starting a converter
with different rates starts a new stream. Equal rates are an exact conversion, including the first
and last sample.

The interpolation result is rounded and clipped to signed 16-bit. Resampling makes no loudness,
channel, speech or codec decision. It is the shared mechanism for the media boundary and the
diagnostic device driver.

## 3. Playback and capture

`MediaSession::play_pcm` converts a complete `Pcm` to signed 16-bit at the negotiated media clock,
then queues it using the session's own packet size. Existing `play(&[i16], packet_samples)` remains
the lower-level codec-rate operation.

`MediaSession::capture(format)` returns the sole-consumer `PcmCapture` for a chosen format. The
capture owns a resampler so successive RTP frames form one continuous output stream.
`PcmCapture::recv` returns the next converted chunk; `record_at_least` waits for a caller-selected
number of output samples under the same failure-bound semantics as the codec-rate method. Capture
never guesses a rate from buffer length.

| Input | Output | Required observation |
|---|---|---|
| unsigned 8-bit, 8 kHz playback into an 8 kHz session | decoded signed samples | unsigned midpoint and extrema map correctly |
| signed 16-bit, 16 kHz playback into an 8 kHz session | decoded signed samples | duration and interpolation points are preserved |
| 8 kHz received media captured at 16 kHz | signed 16-bit PCM | output count doubles without packet-boundary discontinuities |
| unsupported depth/rate | typed error | no queued frame and no allocation based on the rejected rate |

## 4. L16 on RTP (RFC 3551 §4.5.11)

L16 samples are signed 16-bit network byte order. Payload type 11 statically means mono L16 at
44.1 kHz; payload type 10 is stereo and is refused because this boundary is mono. Every other rate,
including 8 kHz mono, uses a dynamic payload number with `a=rtpmap:<pt> L16/<rate>/1`.

The `L16` codec selection offers static mono 44.1 kHz on type 11 and mono 8 kHz on dynamic type 96.
An answer may select either offered assignment. A peer offer of dynamic mono L16 at a supported
rate, or of static type 11, is negotiable; an absent `rtpmap` on another dynamic number identifies
nothing.
The negotiated RTP clock is retained independently of the codec name and drives packet sizing,
timestamps, playback conversion and capture conversion.

## 5. CLI and documentation

WAV input uses its header's signed-16 sample rate and `play_pcm`; it is no longer refused merely for
differing from the negotiated clock. WAV output records the media session's decoded signed-16
samples and writes the negotiated rate in its header. Device conversion uses the public audio
resampler rather than a private copy. The library guide shows 8-bit playback and caller-rate
capture, including the typed unsupported-format result.

## 6. Vectors

| ID | Vector | Expected |
|---|---|---|
| PCM-1 | `u8 [0, 128, 255]` at 8 kHz → signed 16-bit at 8 kHz | `[-32768, 0, 32512]` |
| PCM-2 | signed ramp at 16 kHz → 8 kHz | every other source point, duration preserved |
| PCM-3 | two adjacent pushes at 8 kHz → 16 kHz | same bytes as one combined push |
| PCM-4 | rate 0 or 384,001 | `UnsupportedSampleRate` |
| L16-1 | samples `[-32768, 0, 32767]` | bytes `80 00 00 00 7f ff`, and exact decode |
| L16-2 | `m=audio ... 11` without `rtpmap` | mono L16, 44.1 kHz |
| L16-3 | `m=audio ... 96` + `a=rtpmap:96 L16/8000/1` | mono L16, 8 kHz |
| L16-4 | `m=audio ... 10` or `L16/<rate>/2` | refused; stereo is outside the boundary |
