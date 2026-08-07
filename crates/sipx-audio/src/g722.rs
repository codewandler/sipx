//! G.722 wideband audio (ITU-T G.722), 64 kbit/s sub-band ADPCM.
//!
//! A 24-tap quadrature-mirror filter pair splits 16 kHz audio into two 8 kHz bands; the lower
//! band is coded with a 6-bit adaptive quantizer and the higher band with a 2-bit one, and each
//! pair of input samples becomes one output octet. Both directions carry adaptive predictor and
//! scale-factor state, so [`Encoder`] and [`Decoder`] are streams rather than pure functions —
//! the same shape Opus has, without the native dependency.
//!
//! Everything here is the recommendation's own fixed-point arithmetic: saturating 16- and 32-bit
//! operations, in the exact order the specification performs them. That is not pedantry — it is
//! what makes the output *bit-exact* against the official ITU-T G.722 Appendix II digital test
//! sequences (`crates/sipx-audio/corpus/g722/`, recovered from the ITU archive by
//! `scripts/import-g722-corpus.sh`), which is the only verification stronger than a round trip.
//! See `docs/specs/g722.md`.
//!
//! On RTP this format is static payload type 9, and RFC 3551 §4.5.2 preserves a historical
//! error deliberately: the RTP timestamp clock advances at [`CLOCK_RATE`] = 8000 while the
//! audio is sampled at [`SAMPLE_RATE`] = 16000. The session layer owns that split; this module
//! only encodes and decodes.

/// The audio sampling rate, in samples per second.
pub const SAMPLE_RATE: u32 = 16_000;

/// The RTP timestamp clock rate — 8000, *not* the sampling rate (RFC 3551 §4.5.2).
pub const CLOCK_RATE: u32 = 8_000;

// ---------------------------------------------------------------------------------------------
// The recommendation's saturating fixed-point operator set. G.722 is specified over these exact
// operations, and the test sequences notice any deviation — an unsaturated add is not "close
// enough", it is a different codec.
// ---------------------------------------------------------------------------------------------

fn sat16(value: i32) -> i16 {
    i16::try_from(value.clamp(i32::from(i16::MIN), i32::from(i16::MAX))).unwrap_or(0)
}

fn add(a: i16, b: i16) -> i16 {
    sat16(i32::from(a) + i32::from(b))
}

fn sub(a: i16, b: i16) -> i16 {
    sat16(i32::from(a) - i32::from(b))
}

/// Q15 multiply: `(a * b) >> 15`, saturated.
fn mult(a: i16, b: i16) -> i16 {
    sat16((i32::from(a) * i32::from(b)) >> 15)
}

/// Saturating left shift. Every shift in this codec is by a small constant.
fn shl(a: i16, bits: u32) -> i16 {
    sat16(i32::from(a) << bits)
}

fn negate(a: i16) -> i16 {
    if a == i16::MIN { i16::MAX } else { -a }
}

/// A table read whose index the caller has already bounded. The fallback is unreachable; it
/// exists so a table lookup can never panic on network input.
fn tab<const N: usize>(table: &[i16; N], index: i16) -> i16 {
    usize::try_from(index)
        .ok()
        .and_then(|at| table.get(at))
        .copied()
        .unwrap_or(0)
}

/// Clamp a reconstructed band sample to the recommendation's 15-bit range.
fn limit(value: i16) -> i16 {
    value.clamp(-16_384, 16_383)
}

// ---------------------------------------------------------------------------------------------
// Quantizer and adaptation tables, from the recommendation.
// ---------------------------------------------------------------------------------------------

/// 6-bit lower-band output codes for a negative difference signal, by decision interval.
const MISIL_NEGATIVE: [i16; 32] = [
    0x00, 0x3F, 0x3E, 0x1F, 0x1E, 0x1D, 0x1C, 0x1B, 0x1A, 0x19, 0x18, 0x17, 0x16, 0x15, 0x14,
    0x13, 0x12, 0x11, 0x10, 0x0F, 0x0E, 0x0D, 0x0C, 0x0B, 0x0A, 0x09, 0x08, 0x07, 0x06, 0x05,
    0x04, 0x00,
];

/// 6-bit lower-band output codes for a positive difference signal.
const MISIL_POSITIVE: [i16; 32] = [
    0x00, 0x3D, 0x3C, 0x3B, 0x3A, 0x39, 0x38, 0x37, 0x36, 0x35, 0x34, 0x33, 0x32, 0x31, 0x30,
    0x2F, 0x2E, 0x2D, 0x2C, 0x2B, 0x2A, 0x29, 0x28, 0x27, 0x26, 0x25, 0x24, 0x23, 0x22, 0x21,
    0x20, 0x00,
];

/// Lower-band quantizer decision levels.
const Q6: [i16; 31] = [
    0, 35, 72, 110, 150, 190, 233, 276, 323, 370, 422, 473, 530, 587, 650, 714, 786, 858, 940,
    1023, 1121, 1219, 1339, 1458, 1612, 1765, 1980, 2195, 2557, 2919, 3200,
];

/// The 4-bit projection of a 6-bit lower-band code, used by the predictor's inverse quantizer.
const RIL4: [i16; 16] = [0, 7, 6, 5, 4, 3, 2, 1, 7, 6, 5, 4, 3, 2, 1, 0];

/// Sign of the 4-bit projection: 0 positive, -1 negative.
const RISI4: [i16; 16] = [0, -1, -1, -1, -1, -1, -1, -1, 0, 0, 0, 0, 0, 0, 0, 0];

/// 4-bit inverse quantizer output levels.
const OQ4: [i16; 8] = [0, 150, 323, 530, 786, 1121, 1612, 2557];

/// 5-bit decoder projection and signs (mode 2), and its output levels.
const RIL5: [i16; 32] = [
    1, 1, 15, 14, 13, 12, 11, 10, 9, 8, 7, 6, 5, 4, 3, 2, 15, 14, 13, 12, 11, 10, 9, 8, 7, 6, 5,
    4, 3, 2, 1, 1,
];
const RISI5: [i16; 32] = [
    -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    0, 0, 0, 0, 0, -1,
];
const OQ5: [i16; 16] = [
    0, 35, 110, 190, 276, 370, 473, 587, 714, 858, 1023, 1219, 1458, 1765, 2195, 2919,
];

/// 6-bit decoder projection and signs (mode 1), and its output levels.
const RIL6: [i16; 64] = [
    1, 1, 1, 1, 30, 29, 28, 27, 26, 25, 24, 23, 22, 21, 20, 19, 18, 17, 16, 15, 14, 13, 12, 11,
    10, 9, 8, 7, 6, 5, 4, 3, 30, 29, 28, 27, 26, 25, 24, 23, 22, 21, 20, 19, 18, 17, 16, 15, 14,
    13, 12, 11, 10, 9, 8, 7, 6, 5, 4, 3, 2, 1, 2, 1,
];
const RISI6: [i16; 64] = [
    -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1, -1,
    -1, -1, -1, -1, -1, -1, -1, -1, -1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
    0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, -1, -1,
];
const OQ6: [i16; 31] = [
    0, 17, 54, 91, 130, 170, 211, 254, 300, 347, 396, 447, 501, 558, 618, 682, 750, 822, 899,
    982, 1072, 1170, 1279, 1399, 1535, 1689, 1873, 2088, 2376, 2738, 3101,
];

/// Lower-band logarithmic scale-factor multipliers.
const WL: [i16; 8] = [-60, -30, 58, 172, 334, 538, 1198, 3042];

/// Higher-band code projections and inverse quantizer values.
const IH2: [i16; 4] = [2, 1, 2, 1];
const SIH: [i16; 4] = [-1, -1, 0, 0];
const OQ2: [i16; 3] = [0, 202, 926];

/// Higher-band logarithmic scale-factor multipliers.
const WH: [i16; 3] = [0, -214, 798];

/// Higher-band output codes by sign and decision interval.
const MISIH_NEGATIVE: [i16; 3] = [0, 1, 0];
const MISIH_POSITIVE: [i16; 3] = [0, 3, 2];

/// The antilogarithm table behind both bands' scale factors.
const ILA: [i16; 353] = [
    1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2, 2,
    2, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 3, 4, 4, 4, 4, 4, 4, 4, 4, 5, 5, 5, 5, 5, 5, 5, 6, 6, 6, 6,
    6, 6, 7, 7, 7, 7, 7, 7, 8, 8, 8, 8, 8, 9, 9, 9, 9, 10, 10, 10, 10, 11, 11, 11, 11, 12, 12,
    12, 13, 13, 13, 13, 14, 14, 15, 15, 15, 16, 16, 16, 17, 17, 18, 18, 18, 19, 19, 20, 20, 21,
    21, 22, 22, 23, 23, 24, 24, 25, 25, 26, 27, 27, 28, 28, 29, 30, 31, 31, 32, 33, 33, 34, 35,
    36, 37, 37, 38, 39, 40, 41, 42, 43, 44, 45, 46, 47, 48, 49, 50, 51, 52, 54, 55, 56, 57, 58,
    60, 61, 63, 64, 65, 67, 68, 70, 71, 73, 75, 76, 78, 80, 82, 83, 85, 87, 89, 91, 93, 95, 97,
    99, 102, 104, 106, 109, 111, 113, 116, 118, 121, 124, 127, 129, 132, 135, 138, 141, 144, 147,
    151, 154, 157, 161, 165, 168, 172, 176, 180, 184, 188, 192, 196, 200, 205, 209, 214, 219,
    223, 228, 233, 238, 244, 249, 255, 260, 266, 272, 278, 284, 290, 296, 303, 310, 316, 323,
    331, 338, 345, 353, 361, 369, 377, 385, 393, 402, 411, 420, 429, 439, 448, 458, 468, 478,
    489, 500, 511, 522, 533, 545, 557, 569, 582, 594, 607, 621, 634, 648, 663, 677, 692, 707,
    723, 739, 755, 771, 788, 806, 823, 841, 860, 879, 898, 918, 938, 958, 979, 1001, 1023, 1045,
    1068, 1092, 1115, 1140, 1165, 1190, 1216, 1243, 1270, 1298, 1327, 1356, 1386, 1416, 1447,
    1479, 1511, 1544, 1578, 1613, 1648, 1684, 1721, 1759, 1797, 1837, 1877, 1918, 1960, 2003,
    2047, 2092, 2138, 2185, 2232, 2281, 2331, 2382, 2434, 2488, 2542, 2598, 2655, 2713, 2773,
    2833, 2895, 2959, 3024, 3090, 3157, 3227, 3297, 3370, 3443, 3519, 3596, 3675, 3755, 3837,
    3921, 4007, 4095,
];

/// QMF coefficients, common to analysis and synthesis.
const QMF_COEF: [i16; 24] = [
    6, -22, -22, 106, 24, -312, 64, 724, -420, -1610, 1902, 7752, 7752, 1902, -1610, -420, 724,
    64, -312, 24, 106, -22, -22, 6,
];

// ---------------------------------------------------------------------------------------------
// One band's ADPCM state and its adaptation, shared by encoder and decoder — the recommendation
// specifies them as the same blocks, which is what keeps the two ends converged.
// ---------------------------------------------------------------------------------------------

/// Predictor, scale factor and signal history for one sub-band.
#[derive(Debug, Clone)]
struct Band {
    /// Quantizer scale factor.
    det: i16,
    /// Logarithmic scale factor.
    nb: i16,
    /// Predictor output.
    s: i16,
    /// Pole-section contribution to the predictor.
    sp: i16,
    /// Zero-section contribution to the predictor.
    sz: i16,
    /// Pole predictor coefficients; index 0 is unused, as in the recommendation's numbering.
    a: [i16; 3],
    /// Zero predictor coefficients; index 0 is unused.
    b: [i16; 7],
    /// Quantized difference signal history.
    d: [i16; 7],
    /// Partially reconstructed signal history.
    p: [i16; 3],
    /// Reconstructed signal history.
    r: [i16; 3],
}

impl Band {
    /// A lower band in the reset state the recommendation defines.
    fn lower() -> Self {
        Self {
            det: 32,
            ..Self::silent()
        }
    }

    /// A higher band in the reset state.
    fn higher() -> Self {
        Self {
            det: 8,
            ..Self::silent()
        }
    }

    fn silent() -> Self {
        Self {
            det: 0,
            nb: 0,
            s: 0,
            sp: 0,
            sz: 0,
            a: [0; 3],
            b: [0; 7],
            d: [0; 7],
            p: [0; 3],
            r: [0; 3],
        }
    }

    /// The predictor adaptation common to both bands and both directions: reconstruct, update
    /// the zero and pole sections, and form the next prediction.
    fn adapt(&mut self, quantized_difference: i16) {
        self.d[0] = quantized_difference;
        self.p[0] = add(quantized_difference, self.sz);
        self.r[0] = add(self.s, quantized_difference);
        self.upzero();
        self.uppol2();
        self.uppol1();
        self.sz = self.filtez();
        self.sp = self.filtep();
        self.s = add(self.sp, self.sz);
    }

    /// Update the zero-section coefficients and shift the difference-signal history.
    fn upzero(&mut self) {
        let gain = if self.d[0] == 0 { 0 } else { 128 };
        let sign = self.d[0] >> 15;
        // Each coefficient update reads only its own tap of the pre-shift history, so the
        // updates commute and the history shift can happen once at the end.
        for (coefficient, history) in self.b.iter_mut().skip(1).zip(self.d.iter().skip(1)) {
            let nudge = if sign == (history >> 15) {
                add(0, gain)
            } else {
                sub(0, gain)
            };
            *coefficient = add(nudge, mult(*coefficient, 32_640));
        }
        self.d.copy_within(0..6, 1);
    }

    /// Update the second pole coefficient.
    fn uppol2(&mut self) {
        let sg0 = self.p[0] >> 15;
        let sg1 = self.p[1] >> 15;
        let sg2 = self.p[2] >> 15;
        let wd1 = shl(self.a[1], 2);
        let wd2 = if sg0 == sg1 { sub(0, wd1) } else { add(0, wd1) };
        let wd2 = wd2 >> 7;
        let wd3 = if sg0 == sg2 { 128 } else { -128 };
        let leaked = mult(self.a[2], 32_512);
        self.a[2] = add(add(wd2, wd3), leaked).clamp(-12_288, 12_288);
    }

    /// Update the first pole coefficient and shift the partial reconstruction history.
    fn uppol1(&mut self) {
        let sg0 = self.p[0] >> 15;
        let sg1 = self.p[1] >> 15;
        let wd1 = if sg0 == sg1 { 192 } else { -192 };
        let wd2 = mult(self.a[1], 32_640);
        let mut apl1 = add(wd1, wd2);
        let wd3 = sub(15_360, self.a[2]);
        if apl1 > wd3 {
            apl1 = wd3;
        } else if add(apl1, wd3) < 0 {
            apl1 = negate(wd3);
        }
        self.p[2] = self.p[1];
        self.p[1] = self.p[0];
        self.a[1] = apl1;
    }

    /// The zero-section filter over the shifted difference history.
    ///
    /// Accumulated highest tap first, as the recommendation orders it — the adds saturate, so
    /// the order is part of the arithmetic rather than a style choice.
    fn filtez(&self) -> i16 {
        let mut sum = 0i16;
        for (coefficient, history) in self.b.iter().skip(1).zip(self.d.iter().skip(1)).rev() {
            sum = add(sum, mult(add(*history, *history), *coefficient));
        }
        sum
    }

    /// The pole-section filter, shifting the reconstruction history as it reads it.
    fn filtep(&mut self) -> i16 {
        self.r[2] = self.r[1];
        self.r[1] = self.r[0];
        let wd1 = mult(self.a[1], add(self.r[1], self.r[1]));
        let wd2 = mult(self.a[2], add(self.r[2], self.r[2]));
        add(wd1, wd2)
    }
}

/// Quantize one lower-band difference to a 6-bit code.
fn quantl(el: i16, detl: i16) -> i16 {
    let sil = el >> 15;
    let wd = if sil == 0 {
        el
    } else {
        sub(i16::MAX, el & i16::MAX)
    };
    let mut mil: i16 = 0;
    let mut val = mult(shl(tab(&Q6, mil), 3), detl);
    while sub(val, wd) <= 0 {
        if mil == 30 {
            break;
        }
        mil = add(mil, 1);
        val = mult(shl(tab(&Q6, mil), 3), detl);
    }
    if sil == 0 {
        tab(&MISIL_POSITIVE, mil)
    } else {
        tab(&MISIL_NEGATIVE, mil)
    }
}

/// Quantize one higher-band difference to a 2-bit code.
fn quanth(eh: i16, deth: i16) -> i16 {
    const Q2: i16 = 564;
    let sih = eh >> 15;
    let wd = if sih == 0 {
        eh
    } else {
        sub(i16::MAX, eh & i16::MAX)
    };
    let mih = if sub(wd, mult(shl(Q2, 3), deth)) >= 0 {
        2
    } else {
        1
    };
    if sih == 0 {
        tab(&MISIH_POSITIVE, mih)
    } else {
        tab(&MISIH_NEGATIVE, mih)
    }
}

/// The predictor's 4-bit inverse quantizer for the lower band.
fn invqal(il: i16, detl: i16) -> i16 {
    let ril = il >> 2;
    let magnitude = shl(tab(&OQ4, tab(&RIL4, ril)), 3);
    let signed = if tab(&RISI4, ril) == 0 {
        magnitude
    } else {
        negate(magnitude)
    };
    mult(detl, signed)
}

/// The decoder's mode-dependent inverse quantizer for the lower band.
fn invqbl(ilr: i16, detl: i16, mode: Mode) -> i16 {
    let (magnitude, sign) = match mode {
        Mode::Bits6 => {
            let ril = ilr;
            (shl(tab(&OQ6, tab(&RIL6, ril)), 3), tab(&RISI6, ril))
        }
        Mode::Bits5 => {
            let ril = ilr >> 1;
            (shl(tab(&OQ5, tab(&RIL5, ril)), 3), tab(&RISI5, ril))
        }
        Mode::Bits4 => {
            let ril = ilr >> 2;
            (shl(tab(&OQ4, tab(&RIL4, ril)), 3), tab(&RISI4, ril))
        }
    };
    let signed = if sign == 0 {
        add(0, magnitude)
    } else {
        sub(0, magnitude)
    };
    mult(detl, signed)
}

/// The higher band's 2-bit inverse quantizer.
fn invqah(ih: i16, deth: i16) -> i16 {
    let magnitude = shl(tab(&OQ2, tab(&IH2, ih)), 3);
    let signed = if tab(&SIH, ih) == 0 {
        magnitude
    } else {
        negate(magnitude)
    };
    mult(signed, deth)
}

/// Adapt the lower band's logarithmic scale factor.
fn logscl(il: i16, nbl: i16) -> i16 {
    let ril = il >> 2;
    add(mult(nbl, 32_512), tab(&WL, tab(&RIL4, ril))).clamp(0, 18_432)
}

/// Adapt the higher band's logarithmic scale factor.
fn logsch(ih: i16, nbh: i16) -> i16 {
    add(mult(nbh, 32_512), tab(&WH, tab(&IH2, ih))).clamp(0, 22_528)
}

/// The lower band's scale factor from its logarithm.
fn scalel(nbpl: i16) -> i16 {
    let wd = ((nbpl >> 6) & 511) + 64;
    shl(add(tab(&ILA, wd), 1), 2)
}

/// The higher band's scale factor from its logarithm.
fn scaleh(nbph: i16) -> i16 {
    let wd = (nbph >> 6) & 511;
    shl(add(tab(&ILA, wd), 1), 2)
}

/// Encode one lower-band sample; returns the 6-bit code.
fn lsb_encode(band: &mut Band, xl: i16) -> i16 {
    let el = sub(xl, band.s);
    let il = quantl(el, band.det);
    let d0 = invqal(il, band.det);
    band.nb = logscl(il, band.nb);
    band.det = scalel(band.nb);
    band.adapt(d0);
    il
}

/// Encode one higher-band sample; returns the 2-bit code.
fn hsb_encode(band: &mut Band, xh: i16) -> i16 {
    let eh = sub(xh, band.s);
    let ih = quanth(eh, band.det);
    let d0 = invqah(ih, band.det);
    band.nb = logsch(ih, band.nb);
    band.det = scaleh(band.nb);
    band.adapt(d0);
    ih
}

/// The decoder's lower-band operating mode: how many of the 6 code bits carry audio.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    /// 64 kbit/s — all six bits. What RTP payload type 9 carries.
    Bits6,
    /// 56 kbit/s. Never carried on RTP here; constructed by the Appendix II decoder
    /// sequences in this module's tests, which verify all three modes.
    #[allow(dead_code, reason = "exercised by the ITU decoder test sequences")]
    Bits5,
    /// 48 kbit/s. Same status as `Bits5`.
    #[allow(dead_code, reason = "exercised by the ITU decoder test sequences")]
    Bits4,
}

/// Decode one lower-band code; returns the reconstructed band sample.
fn lsb_decode(band: &mut Band, ilr: i16, mode: Mode) -> i16 {
    let dl = invqbl(ilr, band.det, mode);
    let yl = limit(add(band.s, dl));
    let d0 = invqal(ilr, band.det);
    band.nb = logscl(ilr, band.nb);
    band.det = scalel(band.nb);
    band.adapt(d0);
    yl
}

/// Decode one higher-band code; returns the reconstructed band sample.
fn hsb_decode(band: &mut Band, ih: i16) -> i16 {
    let d0 = invqah(ih, band.det);
    band.nb = logsch(ih, band.nb);
    band.det = scaleh(band.nb);
    band.adapt(d0);
    limit(band.r[0])
}

// ---------------------------------------------------------------------------------------------
// The quadrature-mirror filters. One delay line each; the accumulation is 32-bit saturating.
// ---------------------------------------------------------------------------------------------

fn sat32(value: i64) -> i32 {
    i32::try_from(value.clamp(i64::from(i32::MIN), i64::from(i32::MAX))).unwrap_or(0)
}

/// The even/odd filter sums over a delay line.
fn qmf_sums(delay: &[i16; 24]) -> (i32, i32) {
    let mut even = 0i32;
    let mut odd = 0i32;
    for (index, (coefficient, held)) in QMF_COEF.iter().zip(delay.iter()).enumerate() {
        let product = i32::from(*coefficient) * i32::from(*held);
        if index % 2 == 0 {
            even = even.saturating_add(product);
        } else {
            odd = odd.saturating_add(product);
        }
    }
    (even, odd)
}

/// Age a QMF delay line by one sample pair.
fn qmf_shift(delay: &mut [i16; 24]) {
    delay.copy_within(0..22, 2);
}

/// Split one pair of 16 kHz samples into a lower- and higher-band sample.
fn qmf_analysis(delay: &mut [i16; 24], first: i16, second: i16) -> (i16, i16) {
    delay[1] = first;
    delay[0] = second;
    let (even, odd) = qmf_sums(delay);
    qmf_shift(delay);
    let low = sat32(i64::from(even.saturating_add(odd)) * 2);
    let high = sat32(i64::from(even.saturating_sub(odd)) * 2);
    (
        limit(sat16(low >> 16)),
        limit(sat16(high >> 16)),
    )
}

/// Merge one lower- and higher-band sample back into a pair of 16 kHz samples.
fn qmf_synthesis(delay: &mut [i16; 24], low: i16, high: i16) -> (i16, i16) {
    delay[1] = add(low, high);
    delay[0] = sub(low, high);
    let (even, odd) = qmf_sums(delay);
    qmf_shift(delay);
    (
        sat16(sat32(i64::from(even) << 4) >> 16),
        sat16(sat32(i64::from(odd) << 4) >> 16),
    )
}

// ---------------------------------------------------------------------------------------------
// The public stream types.
// ---------------------------------------------------------------------------------------------

/// A G.722 encoder: 16 kHz signed 16-bit mono in, 64 kbit/s payload octets out.
#[derive(Debug)]
pub struct Encoder {
    lower: Band,
    higher: Band,
    qmf_delay: [i16; 24],
}

impl Encoder {
    /// An encoder in the recommendation's reset state.
    #[must_use]
    pub fn new() -> Self {
        Self {
            lower: Band::lower(),
            higher: Band::higher(),
            qmf_delay: [0; 24],
        }
    }

    /// Encode a frame of 16 kHz samples; every pair of samples becomes one octet.
    ///
    /// G.722 operates on sample pairs, so a frame with an odd sample count encodes its
    /// `len - 1` leading samples and ignores the last; RTP packetization always hands this
    /// even counts.
    #[must_use]
    pub fn encode(&mut self, samples: &[i16]) -> Vec<u8> {
        let mut payload = Vec::with_capacity(samples.len() / 2);
        for pair in samples.chunks_exact(2) {
            let &[first, second] = pair else {
                // Unreachable: `chunks_exact(2)` yields complete pairs only.
                break;
            };
            let (xl, xh) = qmf_analysis(&mut self.qmf_delay, first, second);
            let il = lsb_encode(&mut self.lower, xl);
            let ih = hsb_encode(&mut self.higher, xh);
            let code = (u16::from(ih.unsigned_abs()) << 6) | u16::from(il.unsigned_abs() & 0x3F);
            payload.push(u8::try_from(code & 0xFF).unwrap_or(0));
        }
        payload
    }
}

impl Default for Encoder {
    fn default() -> Self {
        Self::new()
    }
}

/// A G.722 decoder: 64 kbit/s payload octets in, 16 kHz signed 16-bit mono out.
///
/// Decodes at 64 kbit/s (mode 1), which is what RTP payload type 9 carries. Any byte sequence
/// is decodable — a stream joined mid-call converges, and nothing panics.
#[derive(Debug)]
pub struct Decoder {
    lower: Band,
    higher: Band,
    qmf_delay: [i16; 24],
}

impl Decoder {
    /// A decoder in the recommendation's reset state.
    #[must_use]
    pub fn new() -> Self {
        Self {
            lower: Band::lower(),
            higher: Band::higher(),
            qmf_delay: [0; 24],
        }
    }

    /// Decode payload octets; every octet becomes a pair of 16 kHz samples.
    #[must_use]
    pub fn decode(&mut self, payload: &[u8]) -> Vec<i16> {
        let mut samples = Vec::with_capacity(payload.len() * 2);
        for &code in payload {
            let il = i16::from(code & 0x3F);
            let ih = i16::from((code >> 6) & 0x03);
            let rl = lsb_decode(&mut self.lower, il, Mode::Bits6);
            let rh = hsb_decode(&mut self.higher, ih);
            let (first, second) = qmf_synthesis(&mut self.qmf_delay, rl, rh);
            samples.push(first);
            samples.push(second);
        }
        samples
    }
}

impl Default for Decoder {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    clippy::cast_possible_wrap
)]
mod tests {
    use super::*;

    /// One committed Appendix II sequence, as 16-bit words (the files are big-endian).
    fn sequence(name: &str) -> Vec<u16> {
        let path = format!("{}/corpus/g722/{name}", env!("CARGO_MANIFEST_DIR"));
        let bytes = std::fs::read(&path)
            .unwrap_or_else(|error| panic!("reading {path}: {error}; run scripts/import-g722-corpus.sh"));
        bytes
            .chunks_exact(2)
            .map(|pair| u16::from_be_bytes([pair[0], pair[1]]))
            .collect()
    }

    /// Drive the band encoders the way the recommendation's test harness does: QMF bypassed,
    /// the same word to both bands, bit 0 of each input word signalling a state reset, and the
    /// produced code word carried in the output's high byte.
    fn encode_sequence(input: &[u16]) -> Vec<u16> {
        let mut lower = Band::lower();
        let mut higher = Band::higher();
        input
            .iter()
            .map(|&word| {
                if word & 1 == 1 {
                    lower = Band::lower();
                    higher = Band::higher();
                    1
                } else {
                    let sample = (word as i16) >> 1;
                    let il = lsb_encode(&mut lower, sample);
                    let ih = hsb_encode(&mut higher, sample);
                    (((ih as u16) << 6 | (il as u16)) << 8) & 0xFF00
                }
            })
            .collect()
    }

    /// Drive the band decoders over a code sequence in one mode; returns the (lower, higher)
    /// outputs in the reference files' left-shifted format.
    fn decode_sequence(codes: &[u16], mode: Mode) -> (Vec<u16>, Vec<u16>) {
        let mut lower = Band::lower();
        let mut higher = Band::higher();
        let mut low_out = Vec::with_capacity(codes.len());
        let mut high_out = Vec::with_capacity(codes.len());
        for &word in codes {
            if word & 1 == 1 {
                lower = Band::lower();
                higher = Band::higher();
                low_out.push(1);
                high_out.push(1);
            } else {
                let il = ((word >> 8) & 0x3F) as i16;
                let ih = ((word >> 14) & 0x03) as i16;
                let rl = lsb_decode(&mut lower, il, mode);
                let rh = hsb_decode(&mut higher, ih);
                low_out.push(((rl as u16) << 1) & 0xFFFE);
                high_out.push(((rh as u16) << 1) & 0xFFFE);
            }
        }
        (low_out, high_out)
    }

    fn assert_words_equal(produced: &[u16], expected: &[u16], what: &str) {
        assert_eq!(produced.len(), expected.len(), "{what}: length");
        for (index, (got, want)) in produced.iter().zip(expected.iter()).enumerate() {
            assert_eq!(
                got, want,
                "{what} diverges from the reference at word {index}: got {got:#06x}, want {want:#06x}"
            );
        }
    }

    /// G722-1: the encoder reproduces the recommendation's own code sequences bit-exactly.
    /// Round-tripping cannot show this — two mirrored defects round-trip perfectly.
    #[test]
    fn the_encoder_matches_the_itu_reference_sequences() {
        for (input, expected) in [("bt1c1.xmt", "bt2r1.cod"), ("bt1c2.xmt", "bt2r2.cod")] {
            assert_words_equal(
                &encode_sequence(&sequence(input)),
                &sequence(expected),
                &format!("{input} -> {expected}"),
            );
        }
    }

    /// G722-2: the decoder reproduces the reference outputs bit-exactly, in all three
    /// operating modes, for all three code sequences — nine lower-band comparisons and the
    /// higher band alongside each.
    #[test]
    fn the_decoder_matches_the_itu_reference_sequences_in_all_modes() {
        for (codes, low_prefix, high) in [
            ("bt2r1.cod", "bt3l1", "bt3h1.rc0"),
            ("bt2r2.cod", "bt3l2", "bt3h2.rc0"),
            ("bt1d3.cod", "bt3l3", "bt3h3.rc0"),
        ] {
            let code_words = sequence(codes);
            let expected_high = sequence(high);
            for (mode, suffix) in [(Mode::Bits6, "rc1"), (Mode::Bits5, "rc2"), (Mode::Bits4, "rc3")]
            {
                let (low_out, high_out) = decode_sequence(&code_words, mode);
                let low_reference = format!("{low_prefix}.{suffix}");
                assert_words_equal(
                    &low_out,
                    &sequence(&low_reference),
                    &format!("{codes} -> {low_reference}"),
                );
                assert_words_equal(&high_out, &expected_high, &format!("{codes} -> {high}"));
            }
        }
    }

    /// G722-3 and G722-4: one octet per two 16 kHz samples, both ways. This byte-per-sample
    /// arithmetic is what makes the RFC 3551 §4.5.2 clock work out: 160 octets describe 20 ms.
    #[test]
    fn a_20_ms_frame_is_320_samples_and_160_octets() {
        let samples: Vec<i16> = (0..320).map(|i| (i * 97 % 4001) - 2000).collect();
        let payload = Encoder::new().encode(&samples);
        assert_eq!(payload.len(), 160);
        let decoded = Decoder::new().decode(&payload);
        assert_eq!(decoded.len(), 320);
    }

    /// The round trip is lossy by design, so equality is not the test — but the decoded audio
    /// must correlate strongly with what went in, which catches a band swap or a QMF ordering
    /// defect that the pure counting test above would miss.
    #[test]
    fn the_round_trip_preserves_the_signal_shape() {
        // Two full packets of a 400 Hz tone at 16 kHz.
        let samples: Vec<i16> = (0..640)
            .map(|i| {
                let phase = f64::from(i) * 400.0 * 2.0 * std::f64::consts::PI / 16_000.0;
                (phase.sin() * 8_000.0).round() as i16
            })
            .collect();
        let mut encoder = Encoder::new();
        let mut decoder = Decoder::new();
        let decoded = decoder.decode(&encoder.encode(&samples));
        assert_eq!(decoded.len(), samples.len());

        // The QMF pair delays the signal; correlate at the codec's known group delay rather
        // than demanding sample alignment.
        let delay = 23;
        let mut correlation = 0f64;
        let mut input_energy = 0f64;
        let mut output_energy = 0f64;
        for i in 0..(samples.len() - delay) {
            let x = f64::from(samples[i]);
            let y = f64::from(decoded[i + delay]);
            correlation += x * y;
            input_energy += x * x;
            output_energy += y * y;
        }
        let normalized = correlation / (input_energy.sqrt() * output_energy.sqrt());
        assert!(
            normalized > 0.9,
            "decoded audio no longer resembles the input: correlation {normalized:.3}"
        );
    }

    /// No byte sequence may panic the decoder, and a decoder joining a stream mid-call keeps
    /// producing two samples per octet.
    #[test]
    fn any_payload_is_decodable() {
        let mut decoder = Decoder::new();
        let hostile: Vec<u8> = (0..=255).collect();
        assert_eq!(decoder.decode(&hostile).len(), 512);
        assert_eq!(decoder.decode(&[0xFF; 160]).len(), 320);
    }

    /// An odd trailing sample is ignored rather than corrupting the pair stream.
    #[test]
    fn an_odd_sample_count_encodes_the_pairs_it_has() {
        assert_eq!(Encoder::new().encode(&[0i16; 321]).len(), 160);
        assert_eq!(Encoder::new().encode(&[123i16]).len(), 0);
    }
}
