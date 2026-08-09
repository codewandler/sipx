//! The integer arithmetic every built-in shares: `docs/specs/call-dsp-effects.md` §3.
//!
//! Nothing here is floating point, and that is a decision rather than an omission. The processor
//! contract's §4.6 restricts a processor to the exactly-rounded IEEE 754 operations and forbids a
//! transcendental anywhere in the sample path or in coefficient derivation, so that a fixture
//! computed on one machine stays evidence on another. Integer arithmetic at stated widths carries
//! that property without anyone having to reason about which floating-point operations a compiler
//! is allowed to contract — and §4.5 already requires that it never wrap silently, which is what
//! the saturating operations and [`clamp_sample`] are for.

/// `value · numerator / denominator`, rounded half **away from zero**.
///
/// Away from zero rather than toward it, so the rounding is symmetric about zero: a signal and its
/// negation produce exactly negated output, which is the property [`super::level::Polarity`] and
/// every gain vector are written against. `denominator` is a positive constant at every call site;
/// a zero one yields zero rather than dividing.
pub(in crate::dsp) fn scaled(value: i64, numerator: i64, denominator: i64) -> i64 {
    if denominator <= 0 {
        return 0;
    }
    let product = value.saturating_mul(numerator);
    let half = denominator / 2;
    if product >= 0 {
        product.saturating_add(half) / denominator
    } else {
        product.saturating_sub(half) / denominator
    }
}

/// Narrow to `i32`, saturating rather than wrapping.
///
/// The intermediate width every effect works in is `i64`; this is where it comes back down, and it
/// comes down by clamping because §4.5 admits saturation as a declared behaviour and never a wrap.
pub(in crate::dsp) fn narrow(value: i64) -> i32 {
    i32::try_from(value).unwrap_or(if value > 0 { i32::MAX } else { i32::MIN })
}

/// Clamp to the representable sample range, reporting whether it had to.
///
/// The flag is what becomes [`crate::dsp::DspObservation::Saturated`], whose contract meaning is
/// narrow on purpose: output clamped to **full scale**. An effect that limits to a threshold of its
/// own — hard clipping to a declared ceiling, say — is applying its transfer function and is not
/// saturating, so it never reaches this.
pub(in crate::dsp) fn clamp_sample(value: i64) -> (i16, bool) {
    match i16::try_from(value) {
        Ok(sample) => (sample, false),
        Err(_) => {
            if value > 0 {
                (i16::MAX, true)
            } else {
                (i16::MIN, true)
            }
        }
    }
}

/// A linear parameter transition counted in positions (§5).
///
/// A ramp is a pure function of how many positions have been consumed since the set was applied,
/// which is what makes a smoothed change independent of how the caller cut the stream: the same
/// positions produce the same values whether they arrived as one frame or as twenty. The last step
/// assigns the target exactly rather than accumulating toward it, so a transition always lands.
#[derive(Debug, Clone, Copy)]
pub(super) struct Ramp {
    start: i32,
    target: i32,
    value: i32,
    length: u32,
    step: u32,
}

impl Ramp {
    /// A ramp at rest on `value`.
    pub(super) const fn new(value: i32) -> Self {
        Self {
            start: value,
            target: value,
            value,
            length: 0,
            step: 0,
        }
    }

    /// Begin a transition to `target` over `length` positions. A length of zero applies it whole.
    pub(super) const fn to(&mut self, target: i32, length: u32) {
        self.start = self.value;
        self.target = target;
        self.length = length;
        self.step = 0;
        if length == 0 {
            self.start = target;
            self.value = target;
        }
    }

    /// The value in force for the next position, advancing the transition by one position.
    pub(super) fn advance(&mut self) -> i32 {
        if self.step < self.length {
            self.step = self.step.saturating_add(1);
            let span = i64::from(self.target) - i64::from(self.start);
            let moved = scaled(span, i64::from(self.step), i64::from(self.length));
            self.value = narrow(i64::from(self.start) + moved);
        }
        self.value
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

    /// §3: rounding is symmetric about zero, so negating the input negates the output exactly.
    #[test]
    fn rounding_is_symmetric_about_zero() {
        for value in [-3i64, -2, -1, 0, 1, 2, 3, 32_767] {
            assert_eq!(
                scaled(value, 1_500, 1_000),
                -scaled(-value, 1_500, 1_000),
                "{value}"
            );
        }
        assert_eq!(scaled(1, 1_500, 1_000), 2);
        assert_eq!(scaled(3, 1_500, 1_000), 5);
        assert_eq!(scaled(0, 1_500, 0), 0, "a zero denominator divides nothing");
    }

    /// §3: full scale clamps rather than wrapping, and reports that it did.
    #[test]
    fn clamping_reports_itself() {
        assert_eq!(clamp_sample(32_768), (i16::MAX, true));
        assert_eq!(clamp_sample(-32_769), (i16::MIN, true));
        assert_eq!(clamp_sample(-32_768), (i16::MIN, false));
        assert_eq!(narrow(i64::from(i32::MAX) + 1), i32::MAX);
        assert_eq!(narrow(i64::from(i32::MIN) - 1), i32::MIN);
    }

    /// §5: a ramp lands exactly on its target, and settling completes it at once.
    #[test]
    fn a_ramp_lands_on_its_target() {
        let mut ramp = Ramp::new(1_000);
        ramp.to(2_000, 4);
        assert_eq!(
            [
                ramp.advance(),
                ramp.advance(),
                ramp.advance(),
                ramp.advance(),
                ramp.advance()
            ],
            [1_250, 1_500, 1_750, 2_000, 2_000]
        );

        let mut ramp = Ramp::new(1_000);
        ramp.to(0, 0);
        assert_eq!(ramp.advance(), 0, "a zero-length transition applies whole");
    }
}
