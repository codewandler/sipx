//! What a call actually sounded like: loss, jitter, round-trip time, and an estimate of how
//! bad that combination was.
//!
//! Everything here is derived from numbers the RTCP exchange already carries. Nothing is
//! guessed, and the one figure that *is* an estimate — the mean opinion score — says so in its
//! own documentation, because a number between 1 and 5 that looks like a measurement is
//! exactly the kind of number someone will make a decision on.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Seconds between the NTP epoch (1900-01-01) and the Unix one (1970-01-01).
///
/// Including the leap days: NTP counts 70 years of which 17 were leap years.
const NTP_EPOCH_OFFSET: u64 = 2_208_988_800;

/// Now, as a 64-bit NTP timestamp: seconds in the high half, fraction in the low half.
///
/// A clock that has never been set gives a time near the Unix epoch, which becomes an NTP
/// timestamp in 1970 — wrong, but consistently wrong, and the round-trip calculation below
/// works on *differences* of these, so a constant offset cancels. What would not cancel is a
/// clock that steps mid-call, which is why round-trip times are reported as a most recent
/// sample rather than accumulated into an average.
#[must_use]
pub fn ntp_now() -> u64 {
    let since_epoch = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or(Duration::ZERO);
    let seconds = since_epoch.as_secs().saturating_add(NTP_EPOCH_OFFSET);
    // The fraction is in units of 2^-32 seconds.
    let fraction = (u64::from(since_epoch.subsec_nanos()) << 32) / 1_000_000_000;
    (seconds << 32) | fraction
}

/// The middle 32 bits of an NTP timestamp: 16 bits of seconds, 16 of fraction.
///
/// This is what a report block echoes back (RFC 3550 §6.4.1), and the truncation is why the
/// round trip below wraps rather than saturates: the field rolls over every 18 hours.
#[must_use]
pub fn middle_32(ntp: u64) -> u32 {
    ((ntp >> 16) & 0xFFFF_FFFF) as u32
}

/// The round-trip time from a report block, per RFC 3550 §6.4.1.
///
/// `now` is the middle 32 bits of our clock when the report arrived, `last_sender_report` is
/// what the peer echoed of ours, and `delay` is how long the peer sat on it. Subtracting the
/// peer's own delay is the whole point: without it, an implementation that reports every five
/// seconds looks five seconds away.
///
/// `None` when there is nothing to compute from — a peer that has had no sender report from us
/// echoes zero — or when the arithmetic comes out negative, which means one of the two clocks
/// moved and the answer would be fiction.
#[must_use]
pub fn round_trip(now: u32, last_sender_report: u32, delay: u32) -> Option<Duration> {
    if last_sender_report == 0 {
        return None;
    }
    // Wrapping, because the field is a truncation of a larger counter and rolls over.
    let elapsed = now.wrapping_sub(last_sender_report);
    let round_trip = elapsed.checked_sub(delay)?;
    // The units are 1/65536 of a second.
    Some(Duration::from_nanos(
        u64::from(round_trip) * 1_000_000_000 / 65_536,
    ))
}

/// How a call is going.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Quality {
    /// Loss since the last report, as a fraction between 0 and 1.
    pub loss: f64,
    /// Packets lost since the stream began. Signed, because duplicates can make it go down.
    pub cumulative_lost: i64,
    /// Interarrival jitter (RFC 3550 §6.4.1).
    pub jitter: Duration,
    /// The most recent round-trip time, if a report has come back with one.
    pub round_trip: Option<Duration>,
    /// An estimated mean opinion score. See [`Quality::mos`] for what it is and is not.
    pub mos: f64,
}

impl Quality {
    /// Estimate a mean opinion score from loss, jitter and round-trip time.
    ///
    /// **This is an estimate, not a measurement.** A real MOS comes from people listening. What
    /// this computes is the ITU-T G.107 E-model's transmission rating `R`, converted to a score
    /// by G.107's own formula, from impairment terms that are the common simplification rather
    /// than the full model: delay is folded into one term, and loss is charged at a flat rate
    /// that is roughly right for G.711 and wrong for a codec with packet loss concealment.
    ///
    /// It is worth having anyway, because it collapses three numbers that trade against each
    /// other into one that can be compared between calls. It is not worth reporting to four
    /// decimal places, and sipx does not.
    #[must_use]
    pub fn mos(loss: f64, jitter: Duration, round_trip: Option<Duration>) -> f64 {
        let latency_ms = round_trip.unwrap_or(Duration::ZERO).as_secs_f64() * 1000.0;
        let jitter_ms = jitter.as_secs_f64() * 1000.0;
        // Two jitter buffers' worth of jitter, plus a nominal 10 ms for everything else in the
        // path that is not measured here.
        let effective = latency_ms + jitter_ms * 2.0 + 10.0;

        // The knee at 160 ms is where added delay starts to hurt sharply rather than gently —
        // the point conversation stops feeling immediate.
        let mut rating = if effective < 160.0 {
            93.2 - effective / 40.0
        } else {
            93.2 - (effective - 120.0) / 10.0
        };
        // 2.5 rating points per percent lost. Flat, which is the simplification: real
        // impairment is steeply non-linear and depends on the codec's concealment.
        rating -= loss * 100.0 * 2.5;

        Self::score_from_rating(rating)
    }

    /// G.107's own conversion from the transmission rating `R` to a score.
    ///
    /// Exact, unlike the impairment terms feeding it: this part is the standard's formula.
    #[must_use]
    pub fn score_from_rating(rating: f64) -> f64 {
        if rating <= 0.0 {
            return 1.0;
        }
        if rating >= 100.0 {
            return 4.5;
        }
        let score = 1.0 + 0.035 * rating + 7.0e-6 * rating * (rating - 60.0) * (100.0 - rating);
        score.clamp(1.0, 4.5)
    }
}

#[cfg(test)]
#[cfg_attr(coverage_nightly, coverage(off))]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
mod tests {
    use super::*;

    /// The units are the point. A quarter of a second is 16384 in 1/65536ths, and getting the
    /// scale wrong here produces round-trip times that are plausible and off by 65536.
    #[test]
    fn a_round_trip_is_the_gap_minus_the_peers_own_delay() {
        // The peer held the report for 1/8 s; a further 1/8 s of that gap was the network.
        let lsr = 1_000_000;
        let now = lsr + 16_384; // a quarter second later
        let delay = 8_192; // an eighth of it was the peer thinking

        let trip = round_trip(now, lsr, delay).expect("computable");
        assert!(
            (trip.as_secs_f64() - 0.125).abs() < 0.001,
            "expected an eighth of a second, got {trip:?}"
        );
    }

    /// Without subtracting the peer's delay, an implementation that reports every five seconds
    /// would look five seconds away.
    #[test]
    fn the_peers_own_delay_does_not_count_as_distance() {
        let lsr = 500_000;
        let now = lsr + 5 * 65_536 + 655; // five seconds and ten milliseconds
        let trip = round_trip(now, lsr, 5 * 65_536).expect("computable");
        assert!(
            trip < Duration::from_millis(50),
            "the five seconds were the peer's, not the network's: {trip:?}"
        );
    }

    #[test]
    fn a_peer_that_has_heard_no_sender_report_yields_nothing() {
        assert!(round_trip(1_000, 0, 0).is_none());
    }

    /// Rather than a number that is fiction. If the arithmetic goes negative, one of the two
    /// clocks moved, and reporting a plausible-looking round trip would be worse than
    /// reporting none.
    #[test]
    fn nonsense_arithmetic_yields_nothing_rather_than_a_guess() {
        assert!(
            round_trip(1_000, 900, 500).is_none(),
            "delay exceeds the gap"
        );
    }

    /// The field is a truncation of a wider counter and rolls over every 18 hours. A call
    /// spanning the rollover must not report a round trip of half a day.
    #[test]
    fn the_calculation_survives_the_field_wrapping() {
        let lsr = u32::MAX - 100;
        let now = lsr.wrapping_add(6_553); // a tenth of a second later, across the wrap
        let trip = round_trip(now, lsr, 0).expect("computable");
        assert!(
            trip < Duration::from_millis(200),
            "the wrap is a continuation, not 18 hours: {trip:?}"
        );
    }

    /// A perfect call scores at the top of the scale, and the scale tops out at 4.5 — which is
    /// what G.711 can achieve, not 5. A stack reporting 5.0 for a toll-quality call is
    /// reporting something the codec cannot deliver.
    #[test]
    fn a_clean_call_scores_near_the_top() {
        let mos = Quality::mos(0.0, Duration::ZERO, Some(Duration::from_millis(20)));
        assert!(mos > 4.2, "a clean call should score well: {mos}");
        assert!(
            mos <= 4.5,
            "and never above what the codec can deliver: {mos}"
        );
    }

    #[test]
    fn loss_lowers_the_score_and_more_loss_lowers_it_further() {
        let clean = Quality::mos(0.0, Duration::ZERO, Some(Duration::from_millis(20)));
        let some = Quality::mos(0.02, Duration::ZERO, Some(Duration::from_millis(20)));
        let lots = Quality::mos(0.10, Duration::ZERO, Some(Duration::from_millis(20)));
        assert!(some < clean, "{some} should be worse than {clean}");
        assert!(lots < some, "{lots} should be worse than {some}");
    }

    #[test]
    fn delay_lowers_the_score_too() {
        let near = Quality::mos(0.0, Duration::ZERO, Some(Duration::from_millis(20)));
        let far = Quality::mos(0.0, Duration::ZERO, Some(Duration::from_millis(600)));
        assert!(
            far < near,
            "a satellite hop should score worse: {far} vs {near}"
        );
    }

    /// The scale has a bottom. A call this bad is unusable, and saying 0.3 rather than 1.0
    /// would be reporting a score off the end of the scale it claims to be on.
    #[test]
    fn the_score_never_leaves_its_scale() {
        let dreadful = Quality::mos(
            0.9,
            Duration::from_millis(500),
            Some(Duration::from_secs(3)),
        );
        assert!((1.0..=4.5).contains(&dreadful), "{dreadful}");
        assert!((1.0..=4.5).contains(&Quality::score_from_rating(-50.0)));
        assert!((1.0..=4.5).contains(&Quality::score_from_rating(200.0)));
    }

    /// A constant offset between the two clocks cancels, so a machine whose clock has never
    /// been set still measures round trips correctly.
    #[test]
    fn an_ntp_timestamp_is_in_the_right_century() {
        let now = ntp_now();
        let seconds = now >> 32;
        // 2020-01-01 and 2200-01-01 in NTP seconds. Wide on purpose: this is a check that the
        // epoch offset was applied at all, not a check on the system clock.
        assert!(
            (3_786_825_600..9_467_308_800).contains(&seconds),
            "NTP seconds {seconds} is not a plausible date; the epoch offset is wrong"
        );
    }

    #[test]
    fn the_middle_bits_are_the_middle_bits() {
        assert_eq!(middle_32(0x0000_1234_5678_0000), 0x1234_5678);
    }
}
