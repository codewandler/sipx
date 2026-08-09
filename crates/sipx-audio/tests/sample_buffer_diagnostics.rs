//! Every public type of this crate that holds call audio, rendered (`M-107`).
//!
//! `call_audio_adversarial.rs` holds the analyser's own frame to this — `M-61` fixed that one after
//! finding its derived `Debug` rendering all 65,536 samples it borrowed, in a record reachable from
//! a refusal `sipx-call` already wrote. This file is the sentence that story's implementor could
//! not write: *and every sibling of it*. A fix for one type and a shrug for the others is how the
//! same derive was still sitting on `sipx_media::PcmFrame` two epics later.
//!
//! Each test below builds the type with audio in it, renders it, and asserts two things — the
//! samples are absent, and the record's length is bounded by the implementation rather than by the
//! audio. The second is not decoration: a redaction that still grew with the buffer would have
//! fixed the retention half of the defect and left the unbounded-diagnostic half.
//!
//! `scripts/check-audio-claims.py` holds the *population*: it reports any reachable public type
//! that holds a buffer of PCM samples and implements no `Debug` of its own. What it cannot read is
//! what an implementation prints, which is what is here.

#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]

use sipx_audio::analysis::AudioDirection;
use sipx_audio::dsp::effects::Stutter;
use sipx_audio::dsp::{DspFrame, FrameProcessor, FrameSink, Scratch, StreamFormat};
use sipx_audio::g722::{Decoder, Encoder};
use sipx_audio::{Pcm, PcmEncoding, PcmFormat, PcmSamples, Wav};

/// A sample value that is not a count, a rate, a capacity or an index anywhere in this crate.
///
/// 30,011 is prime. Finding its decimal spelling in a diagnostic record can only mean the samples
/// themselves were rendered.
const SENTINEL: i16 = 30_011;

const FRAME: usize = 160;

/// What every record here has to satisfy: no audio, and a length this crate chose.
///
/// One frame of samples listed out is about a kilobyte — 160 values of up to five digits, each
/// with its separator — so 400 octets is comfortably above every implementation here and
/// comfortably below any rendering that started listing a buffer. A record that leaked the audio
/// in some formatting the sentinel did not survive still fails on length.
///
/// That is a *proxy* for the bound; `a_record_does_not_grow_with_the_audio_it_describes` asserts
/// the property itself.
fn carries_no_audio(what: &str, record: &str) {
    assert!(
        !record.contains("30011"),
        "{what} rendered a sample value: {record}"
    );
    assert!(
        record.len() < 400,
        "{what} wrote a record whose length is the audio's: {record}"
    );
}

fn narrowband() -> PcmFormat {
    PcmFormat::new(8_000, PcmEncoding::Signed16).expect("a supported format")
}

/// The owned buffer every other carrier in the workspace is made of.
///
/// It is the one place raw call audio is actually held, so a container that derives `Debug` over a
/// [`Pcm`] is safe because of what is written here. Both depths, because a redaction that covered
/// only the one the tests happened to use is a redaction on one code path.
#[test]
fn owned_samples_render_their_depth_and_their_count() {
    let signed = PcmSamples::Signed16(vec![SENTINEL; FRAME]);
    let record = format!("{signed:?}");
    carries_no_audio("PcmSamples::Signed16", &record);
    assert!(record.contains("Signed16"), "{record}");
    assert!(record.contains("160"), "how much audio there was: {record}");

    let unsigned = PcmSamples::Unsigned8(vec![0xFE; FRAME]);
    let record = format!("{unsigned:?}");
    assert!(record.contains("Unsigned8"), "{record}");
    assert!(record.contains("160"), "{record}");
    assert!(
        !record.contains("254"),
        "an eight-bit sample is still a sample: {record}"
    );
}

/// The bound is on the implementation and not on the audio, asserted rather than approximated.
///
/// A hundredfold more samples is the *same record*, longer by exactly the extra digits of the
/// count. This is the half of the defect a redaction can silently fail to fix: rendering "the
/// first thirty-two samples, and then the rest" would pass every assertion about the sentinel and
/// still write a record whose length is the buffer's.
#[test]
fn a_record_does_not_grow_with_the_audio_it_describes() {
    for (what, small, large) in [
        (
            "Pcm",
            format!("{:?}", Pcm::from_i16(narrowband(), vec![SENTINEL; FRAME])),
            format!(
                "{:?}",
                Pcm::from_i16(narrowband(), vec![SENTINEL; 100 * FRAME])
            ),
        ),
        (
            "PcmSamples",
            format!("{:?}", PcmSamples::Signed16(vec![SENTINEL; FRAME])),
            format!("{:?}", PcmSamples::Signed16(vec![SENTINEL; 100 * FRAME])),
        ),
        (
            "Wav",
            format!("{:?}", Wav::narrowband(vec![SENTINEL; FRAME])),
            format!("{:?}", Wav::narrowband(vec![SENTINEL; 100 * FRAME])),
        ),
    ] {
        // 160 samples to 16,000: the count gains two digits and nothing else changes.
        assert_eq!(
            large.len(),
            small.len() + 2,
            "{what}'s record grew with the audio: {small} then {large}"
        );
    }
}

/// [`Pcm`] derives its `Debug` and is safe by composition rather than by accident.
#[test]
fn a_buffer_with_its_format_renders_the_format_and_a_count() {
    let pcm = Pcm::from_i16(narrowband(), vec![SENTINEL; FRAME]);
    let record = format!("{pcm:?}");
    carries_no_audio("Pcm", &record);
    assert!(record.contains("8000"), "at what rate: {record}");
    assert!(record.contains("160"), "and how much of it: {record}");
}

/// A clip is a whole file, so the unbounded half of the defect is worse here than on a frame.
#[test]
fn a_wav_clip_renders_its_rate_and_its_length() {
    let clip = Wav::narrowband(vec![SENTINEL; 8_000]);
    let record = format!("{clip:?}");
    carries_no_audio("Wav", &record);
    assert!(record.contains("8000"), "{record}");
}

/// The DSP contract's frame, on the analysis contract's terms: a graph that refuses a frame writes
/// its record with the frame in hand.
#[test]
fn a_dsp_frame_renders_its_identity_and_its_sample_count() {
    let format = StreamFormat::new(8_000, 1).expect("a supported stream format");
    let samples = vec![SENTINEL; FRAME];
    let frame = DspFrame::new(AudioDirection::Inbound, format, 320, &samples);

    let record = format!("{frame:?}");
    carries_no_audio("DspFrame", &record);
    assert!(record.contains("Inbound"), "which side: {record}");
    assert!(record.contains("320"), "where on the timeline: {record}");
    assert!(record.contains("160"), "and how many samples: {record}");
}

/// The caller-owned working memory a processor is lent, and where it writes its output.
///
/// Scratch contents are unspecified on entry and are the processor's working copy of the frame
/// once it starts; the sink's output region is the processed audio itself. Both are rendered by
/// whatever record reports a `ScratchExhausted` or an `OutputOverflow`.
#[test]
fn the_lent_buffers_render_their_capacities_and_never_their_contents() {
    let mut region = vec![SENTINEL; 64];
    let scratch = Scratch::new(&mut region);
    let record = format!("{scratch:?}");
    carries_no_audio("Scratch", &record);
    assert!(record.contains("64"), "how much was lent: {record}");

    let mut output = vec![SENTINEL; FRAME];
    let mut observations = Vec::new();
    let sink = FrameSink::new(&mut output, &mut observations, 8);
    let record = format!("{sink:?}");
    carries_no_audio("FrameSink", &record);
    assert!(record.contains("160"), "the sink's capacity: {record}");
}

/// A delay line holds the call's own audio for as long as the effect is configured.
#[test]
fn a_delay_line_renders_its_length_and_never_the_line() {
    let format = StreamFormat::new(8_000, 1).expect("a supported stream format");
    let mut stutter = Stutter::new(160).expect("a line inside the declared bound");
    stutter
        .prepare(AudioDirection::Inbound, format)
        .expect("a format inside the declaration");

    let samples = vec![SENTINEL; FRAME];
    let frame = DspFrame::new(AudioDirection::Inbound, format, 0, &samples);
    let mut region: Vec<i16> = Vec::new();
    let mut scratch = Scratch::new(&mut region);
    let mut output = vec![0; FRAME];
    let mut observations = Vec::new();
    let mut sink = FrameSink::new(&mut output, &mut observations, 8);
    stutter
        .process(&frame, &mut scratch, &mut sink)
        .expect("a frame inside the declaration");

    let record = format!("{stutter:?}");
    carries_no_audio("Stutter", &record);
    assert!(
        record.contains("repeat"),
        "how the effect is configured is what a record is for: {record}"
    );
}

/// A codec's whole state is the call's audio or a function of it, so its record is its name.
///
/// The QMF delay line is twenty-four samples verbatim and each band's `d`, `p` and `r` are
/// sub-band signal history. There is no identity, position or count here to report in their place
/// — a codec is inspected in a debugger, not in a record somebody pastes into a ticket.
#[test]
fn a_codec_renders_its_name_and_none_of_its_state() {
    let mut encoder = Encoder::new();
    let payload = encoder.encode(&vec![SENTINEL; 2 * FRAME]);
    assert_eq!(payload.len(), FRAME, "the encoder consumed the audio");
    let record = format!("{encoder:?}");
    carries_no_audio("g722::Encoder", &record);
    assert_eq!(record, "Encoder { .. }");

    let mut decoder = Decoder::new();
    assert!(!decoder.decode(&payload).is_empty());
    let record = format!("{decoder:?}");
    carries_no_audio("g722::Decoder", &record);
    assert_eq!(record, "Decoder { .. }");
}
