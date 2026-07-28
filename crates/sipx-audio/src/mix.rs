//! Adding audio streams together.
//!
//! Two things decide whether a conference sounds like a conference.
//!
//! **Saturation, not wrapping.** Two people talking loudly at once produce sums outside the
//! range of an `i16`. Wrapping turns the loudest moment of a call — the moment two people
//! interrupt each other — into a full-scale discontinuity, which is heard as a bang. Clipping
//! the sum instead sounds like a loud moment, which is what it is.
//!
//! **N-1, not N.** Each participant hears everyone *except themselves*. Including their own
//! audio sends their voice back to them delayed by the round trip, and delayed sidetone is
//! close to unbearable: it is the effect used deliberately to stop people speaking. Anyone
//! building a mixer discovers this within a minute of trying it, and the reason it is worth
//! writing down is that N-1 costs a mix per participant rather than one for everybody, and it
//! is tempting to do the cheap thing.

/// Add `source` into `into`, clipping rather than wrapping.
///
/// The shorter of the two decides how much is mixed: a participant whose frame is short has
/// nothing to contribute past its end, and padding it with silence would be the same thing at
/// more cost.
pub fn mix_into(into: &mut [i16], source: &[i16]) {
    for (target, add) in into.iter_mut().zip(source) {
        *target = saturating_add(*target, *add);
    }
}

/// Add two samples, clipping at the ends of the range.
///
/// `i16::saturating_add` already does exactly this. It is worth having a named function anyway,
/// because the mistake this prevents is not writing the wrong addition — it is writing `+`.
#[must_use]
pub fn saturating_add(one: i16, two: i16) -> i16 {
    one.saturating_add(two)
}

/// Mix everything in `sources` except the one at `exclude`.
///
/// This is the N-1 mix, done for one participant. `exclude` out of range mixes everything,
/// which is what a listener who is not a contributor wants.
#[must_use]
pub fn mix_excluding(sources: &[Vec<i16>], exclude: usize, samples: usize) -> Vec<i16> {
    let mut out = vec![0i16; samples];
    for (index, source) in sources.iter().enumerate() {
        if index == exclude {
            continue;
        }
        mix_into(&mut out, source);
    }
    out
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
mod tests {
    use super::*;

    #[test]
    fn mixing_adds_the_samples() {
        let mut into = vec![100, 200, 300];
        mix_into(&mut into, &[10, 20, 30]);
        assert_eq!(into, vec![110, 220, 330]);
    }

    /// The case the whole module exists for. Two loud speakers sum past the range, and wrapping
    /// would turn the loudest instant of the call into a full-scale discontinuity — heard as a
    /// bang, at the exact moment two people are trying to talk over each other.
    #[test]
    fn a_sum_past_the_range_clips_rather_than_wrapping() {
        let mut into = vec![i16::MAX, i16::MIN];
        mix_into(&mut into, &[i16::MAX, i16::MIN]);
        assert_eq!(
            into,
            vec![i16::MAX, i16::MIN],
            "wrapping would send +32767 to -2 and be heard as a bang"
        );
    }

    #[test]
    fn clipping_is_symmetric() {
        assert_eq!(saturating_add(30_000, 10_000), i16::MAX);
        assert_eq!(saturating_add(-30_000, -10_000), i16::MIN);
    }

    /// A short frame contributes what it has and nothing more. Reading past it would be an
    /// out-of-bounds read; padding it would be the same result at more cost.
    #[test]
    fn a_short_source_mixes_only_as_far_as_it_goes() {
        let mut into = vec![1, 1, 1, 1];
        mix_into(&mut into, &[10, 10]);
        assert_eq!(into, vec![11, 11, 1, 1]);
    }

    #[test]
    fn a_long_source_does_not_overrun_the_target() {
        let mut into = vec![1, 1];
        mix_into(&mut into, &[10, 10, 10, 10]);
        assert_eq!(into, vec![11, 11]);
    }

    /// N-1. A participant hearing themselves hears their own voice a round trip late, which is
    /// the single most disorienting thing a conference can do.
    #[test]
    fn a_participant_is_excluded_from_their_own_mix() {
        let sources = vec![vec![100; 4], vec![20; 4], vec![3; 4]];
        assert_eq!(mix_excluding(&sources, 0, 4), vec![23; 4]);
        assert_eq!(mix_excluding(&sources, 1, 4), vec![103; 4]);
        assert_eq!(mix_excluding(&sources, 2, 4), vec![120; 4]);
    }

    /// A listener who contributes nothing hears everybody.
    #[test]
    fn excluding_nobody_mixes_everybody() {
        let sources = vec![vec![100; 4], vec![20; 4], vec![3; 4]];
        assert_eq!(mix_excluding(&sources, usize::MAX, 4), vec![123; 4]);
    }

    #[test]
    fn one_participant_hears_silence_rather_than_themselves() {
        let sources = vec![vec![1000; 4]];
        assert_eq!(mix_excluding(&sources, 0, 4), vec![0; 4]);
    }

    /// The N-1 mix clips too. A mix that saturated per-pair but not overall would still bang
    /// with three loud speakers.
    #[test]
    fn the_excluding_mix_clips_as_well() {
        let sources = vec![vec![20_000; 2], vec![20_000; 2], vec![20_000; 2]];
        assert_eq!(mix_excluding(&sources, 2, 2), vec![i16::MAX; 2]);
    }
}
