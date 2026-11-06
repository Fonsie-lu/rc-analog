//! Filter building blocks shared by the modules: a TPT state variable filter,
//! the focus band split, the tilt / mid tone controls, cut filters, a DC
//! blocker and the 2x halfband oversampler the distortion runs in.
//!
//! Coefficients are recomputed once per chunk (`common::CHUNK`) from smoothed
//! parameters; none of these allocate.

use super::common::{fast_db_to_gain, fast_tan_pi, one_pole_coeff};

/// Q of a Butterworth 2-pole section, as SVF damping `k = 1 / Q`.
pub const BUTTERWORTH_K: f32 = std::f32::consts::SQRT_2;

/// Topology-preserving-transform state variable filter (Zavalishin / Simper).
#[derive(Clone, Copy, Debug, Default)]
pub struct Svf {
    k: f32,
    a1: f32,
    a2: f32,
    a3: f32,
    ic1: f32,
    ic2: f32,
}

#[derive(Clone, Copy, Debug)]
pub struct SvfOut {
    pub lp: f32,
    pub bp: f32,
    pub hp: f32,
}

impl Svf {
    pub fn reset(&mut self) {
        self.ic1 = 0.0;
        self.ic2 = 0.0;
    }

    /// `k` is the damping, 1 / Q.
    #[inline]
    pub fn set(&mut self, hz: f32, k: f32, sample_rate: f32) {
        let g = fast_tan_pi((hz / sample_rate).min(0.45));
        self.k = k;
        self.a1 = 1.0 / (1.0 + g * (g + k));
        self.a2 = g * self.a1;
        self.a3 = g * self.a2;
    }

    #[inline(always)]
    pub fn process(&mut self, x: f32) -> SvfOut {
        let v3 = x - self.ic2;
        let v1 = self.a1 * self.ic1 + self.a2 * v3;
        let v2 = self.ic2 + self.a2 * self.ic1 + self.a3 * v3;
        self.ic1 = 2.0 * v1 - self.ic1;
        self.ic2 = 2.0 * v2 - self.ic2;
        SvfOut { lp: v2, bp: v1, hp: x - self.k * v1 - v2 }
    }

    #[inline(always)]
    pub fn lp(&mut self, x: f32) -> f32 {
        self.process(x).lp
    }

    #[inline(always)]
    pub fn hp(&mut self, x: f32) -> f32 {
        self.process(x).hp
    }

    /// Bandpass normalised to unity gain at the centre.
    #[inline(always)]
    pub fn bp_norm(&mut self, x: f32) -> f32 {
        let k = self.k;
        self.process(x).bp * k
    }
}

/// One-pole lowpass.
#[derive(Clone, Copy, Debug, Default)]
pub struct OnePole {
    a: f32,
    y: f32,
}

impl OnePole {
    pub fn set(&mut self, hz: f32, sample_rate: f32) {
        self.a = one_pole_coeff(hz, sample_rate);
    }
    pub fn set_coeff(&mut self, a: f32) {
        self.a = a;
    }
    pub fn reset(&mut self) {
        self.y = 0.0;
    }
    #[inline(always)]
    pub fn lp(&mut self, x: f32) -> f32 {
        self.y += self.a * (x - self.y);
        self.y
    }
    #[inline(always)]
    pub fn hp(&mut self, x: f32) -> f32 {
        x - self.lp(x)
    }
}

/// Splits a signal into the part inside a frequency band and the rest, such
/// that the two sum back to the input exactly: `band = LP_hi(HP_lo(x))` (12
/// dB/oct either side), `rest = x - band`. Modules process `band` and add
/// `rest` back untouched, which is how a focus range confines an effect
/// without colouring what lies outside it. The steep band keeps lows far
/// below a focus range out of a high-gain shaper.
#[derive(Clone, Copy, Debug, Default)]
pub struct FocusSplit {
    lo: Svf,
    hi: Svf,
}

#[derive(Clone, Copy, Debug)]
pub struct Bands {
    pub band: f32,
    pub rest: f32,
}

impl FocusSplit {
    pub fn reset(&mut self) {
        self.lo.reset();
        self.hi.reset();
    }

    /// Band edges in Hz. A band that is upside down is treated as a narrow
    /// band at the lower edge.
    pub fn set(&mut self, lo_hz: f32, hi_hz: f32, sample_rate: f32) {
        let hi = hi_hz.max(lo_hz * 1.05);
        self.lo.set(lo_hz, BUTTERWORTH_K, sample_rate);
        self.hi.set(hi, BUTTERWORTH_K, sample_rate);
    }

    #[inline(always)]
    pub fn split(&mut self, x: f32) -> Bands {
        let band = self.hi.lp(self.lo.hp(x));
        Bands { band, rest: x - band }
    }
}

/// Tone control: a tilt around ~700 Hz, or a broad mid bell at 1 kHz.
/// `amount` -1 .. 1; the tilt turns highs up and lows down for positive values,
/// the bell boosts the mids for positive values.
#[derive(Clone, Copy, Debug, Default)]
pub struct Tone {
    split: OnePole,
    bell: Svf,
    low_gain: f32,
    high_gain: f32,
    bell_gain: f32,
    mid: bool,
}

impl Tone {
    /// Maximum boost or cut at either end, in dB.
    pub const RANGE_DB: f32 = 9.0;

    pub fn reset(&mut self) {
        self.split.reset();
        self.bell.reset();
    }

    pub fn set(&mut self, amount: f32, mid: bool, sample_rate: f32) {
        self.mid = mid;
        let db = amount.clamp(-1.0, 1.0) * Self::RANGE_DB;
        if mid {
            self.bell.set(1_000.0, 1.0, sample_rate);
            self.bell_gain = fast_db_to_gain(db) - 1.0;
        } else {
            self.split.set(700.0, sample_rate);
            self.low_gain = fast_db_to_gain(-db * 0.5);
            self.high_gain = fast_db_to_gain(db * 0.5);
        }
    }

    #[inline(always)]
    pub fn process(&mut self, x: f32) -> f32 {
        if self.mid {
            x + self.bell_gain * self.bell.bp_norm(x)
        } else {
            let low = self.split.lp(x);
            low * self.low_gain + (x - low) * self.high_gain
        }
    }
}

/// Low or high cut, 12 dB/oct (soft, slightly rounded knee) or 24 dB/oct
/// Butterworth (hard).
#[derive(Clone, Copy, Debug, Default)]
pub struct Cut {
    stages: [Svf; 2],
    hard: bool,
    high_pass: bool,
}

impl Cut {
    pub fn new(high_pass: bool) -> Self {
        Self { high_pass, ..Self::default() }
    }

    pub fn reset(&mut self) {
        self.stages.iter_mut().for_each(Svf::reset);
    }

    pub fn set(&mut self, hz: f32, hard: bool, sample_rate: f32) {
        self.hard = hard;
        if hard {
            self.stages[0].set(hz, 0.765_4, sample_rate);
            self.stages[1].set(hz, 1.847_8, sample_rate);
        } else {
            self.stages[0].set(hz, 1.1, sample_rate);
        }
    }

    #[inline(always)]
    pub fn process(&mut self, x: f32) -> f32 {
        let n = if self.hard { 2 } else { 1 };
        let mut y = x;
        for s in &mut self.stages[..n] {
            y = if self.high_pass { s.hp(y) } else { s.lp(y) };
        }
        y
    }
}

/// First-order DC blocker at about 10 Hz.
#[derive(Clone, Copy, Debug, Default)]
pub struct DcBlocker {
    r: f32,
    x1: f32,
    y1: f32,
}

impl DcBlocker {
    pub fn set(&mut self, sample_rate: f32) {
        self.r = 1.0 - std::f32::consts::TAU * 10.0 / sample_rate;
    }
    pub fn reset(&mut self) {
        self.x1 = 0.0;
        self.y1 = 0.0;
    }
    #[inline(always)]
    pub fn process(&mut self, x: f32) -> f32 {
        let y = x - self.x1 + self.r * self.y1;
        self.x1 = x;
        self.y1 = y;
        y
    }
}

// ---- 2x oversampling ------------------------------------------------------------

/// Length of the halfband FIR used both ways.
const HB_TAPS: usize = 47;
/// Non-zero taps of the interpolating phase (every other tap of the FIR).
const HB_PHASE: usize = HB_TAPS / 2 + 1;
/// Base-rate samples of latency added by one up + down pass.
pub const OVERSAMPLE_LATENCY: usize = (HB_TAPS - 1) / 2;
/// Centre offset of the pure-delay phase, in base-rate samples.
const HB_CENTRE: usize = (HB_TAPS - 1) / 4;
/// The FIR's centre tap seen from the decimator: the odd input sample that
/// lines up with it, counted in pairs back from the newest.
const HB_DOWN_ODD: usize = HB_CENTRE + 1;

/// Kaiser windowed halfband lowpass at a quarter of the doubled rate. Only
/// the even taps and the centre tap (0.5) are non-zero; the even taps are
/// returned normalised to sum to 0.5 so the passband gain is exactly 1.
fn halfband_taps() -> [f32; HB_PHASE] {
    fn bessel_i0(x: f64) -> f64 {
        let mut sum = 1.0;
        let mut term = 1.0;
        for k in 1..40 {
            term *= (x / (2.0 * k as f64)).powi(2);
            sum += term;
        }
        sum
    }
    let beta = 8.0;
    let centre = (HB_TAPS - 1) as f64 / 2.0;
    let mut taps = [0.0f32; HB_PHASE];
    let mut sum = 0.0;
    for (i, t) in taps.iter_mut().enumerate() {
        let n = 2 * i;
        let m = n as f64 - centre;
        let sinc = (std::f64::consts::PI * m / 2.0).sin() / (std::f64::consts::PI * m / 2.0);
        let r = m / centre;
        let w = bessel_i0(beta * (1.0 - r * r).max(0.0).sqrt()) / bessel_i0(beta);
        let h = 0.5 * sinc * w;
        *t = h as f32;
        sum += h;
    }
    for t in &mut taps {
        *t = (*t as f64 * 0.5 / sum) as f32;
    }
    taps
}

/// Halfband 2x upsampler + downsampler pair for one channel. `up` turns one
/// base-rate sample into two, `down` turns two back into one; together they
/// delay by [`OVERSAMPLE_LATENCY`] base samples.
#[derive(Clone, Debug)]
pub struct Oversampler2x {
    taps: [f32; HB_PHASE],
    up_hist: [f32; HB_PHASE],
    even_hist: [f32; HB_PHASE],
    odd_hist: [f32; HB_DOWN_ODD + 1],
}

impl Default for Oversampler2x {
    fn default() -> Self {
        Self {
            taps: halfband_taps(),
            up_hist: [0.0; HB_PHASE],
            even_hist: [0.0; HB_PHASE],
            odd_hist: [0.0; HB_DOWN_ODD + 1],
        }
    }
}

impl Oversampler2x {
    pub fn reset(&mut self) {
        self.up_hist = [0.0; HB_PHASE];
        self.even_hist = [0.0; HB_PHASE];
        self.odd_hist = [0.0; HB_DOWN_ODD + 1];
    }

    #[inline(always)]
    pub fn up(&mut self, x: f32) -> [f32; 2] {
        self.up_hist.copy_within(0..HB_PHASE - 1, 1);
        self.up_hist[0] = x;
        let mut acc = 0.0;
        for (t, h) in self.taps.iter().zip(&self.up_hist) {
            acc += t * h;
        }
        // Gain 2 restores the energy the zero stuffing removed.
        [2.0 * acc, self.up_hist[HB_CENTRE]]
    }

    #[inline(always)]
    pub fn down(&mut self, v: [f32; 2]) -> f32 {
        self.even_hist.copy_within(0..HB_PHASE - 1, 1);
        self.even_hist[0] = v[0];
        self.odd_hist.copy_within(0..HB_DOWN_ODD, 1);
        self.odd_hist[0] = v[1];
        let mut acc = 0.0;
        for (t, h) in self.taps.iter().zip(&self.even_hist) {
            acc += t * h;
        }
        acc + 0.5 * self.odd_hist[HB_DOWN_ODD]
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dsp::common::TWO_PI;

    const SR: f32 = 48_000.0;

    fn sine_gain(mut f: impl FnMut(f32) -> f32, hz: f32) -> f32 {
        let n = 48_000;
        let mut peak: f32 = 0.0;
        for i in 0..n {
            let y = f((TWO_PI * hz * i as f32 / SR).sin());
            if i > n / 2 {
                peak = peak.max(y.abs());
            }
        }
        peak
    }

    #[test]
    fn focus_split_sums_back_to_the_input() {
        let mut s = FocusSplit::default();
        s.set(300.0, 3_000.0, SR);
        let mut rng = crate::dsp::common::Rng::new(5);
        for _ in 0..10_000 {
            let x = rng.next_bipolar();
            let b = s.split(x);
            assert!((b.band + b.rest - x).abs() < 1.0e-5);
        }
    }

    #[test]
    fn focus_band_passes_the_middle_and_rejects_the_edges() {
        let band = |hz: f32| {
            let mut s = FocusSplit::default();
            s.set(300.0, 3_000.0, SR);
            sine_gain(move |x| s.split(x).band, hz)
        };
        assert!(band(1_000.0) > 0.7, "{}", band(1_000.0));
        assert!(band(50.0) < 0.1, "{}", band(50.0));
        assert!(band(15_000.0) < 0.1, "{}", band(15_000.0));
    }

    #[test]
    fn cuts_and_tone_do_what_they_say() {
        let mut lc = Cut::new(true);
        lc.set(500.0, true, SR);
        assert!(sine_gain(|x| lc.process(x), 60.0) < 0.01);
        let mut lc = Cut::new(true);
        lc.set(500.0, true, SR);
        assert!(sine_gain(|x| lc.process(x), 5_000.0) > 0.95);

        let mut tilt = Tone::default();
        tilt.set(1.0, false, SR);
        let high = sine_gain(|x| tilt.process(x), 10_000.0);
        let mut tilt = Tone::default();
        tilt.set(1.0, false, SR);
        let low = sine_gain(|x| tilt.process(x), 50.0);
        assert!(high > 1.4 && low < 0.7, "{high} {low}");

        let mut mid = Tone::default();
        mid.set(-1.0, true, SR);
        assert!(sine_gain(|x| mid.process(x), 1_000.0) < 0.5);
    }

    #[test]
    fn oversampler_round_trip_is_a_delay_in_the_passband() {
        let mut os = Oversampler2x::default();
        let hz = 1_000.0;
        let n = 4_000;
        let input: Vec<f32> = (0..n).map(|i| (TWO_PI * hz * i as f32 / SR).sin()).collect();
        let out: Vec<f32> = input.iter().map(|&x| {
            let up = os.up(x);
            os.down(up)
        }).collect();
        for i in 1_000..n {
            assert!((out[i] - input[i - OVERSAMPLE_LATENCY]).abs() < 2.0e-3, "{i}");
        }
        // An impulse comes out centred on the latency.
        let mut os = Oversampler2x::default();
        let imp: Vec<f32> = (0..100)
            .map(|i| {
                let up = os.up(if i == 0 { 1.0 } else { 0.0 });
                os.down(up)
            })
            .collect();
        let peak = imp.iter().enumerate().max_by(|a, b| a.1.abs().total_cmp(&b.1.abs())).unwrap().0;
        assert_eq!(peak, OVERSAMPLE_LATENCY);
    }

    #[test]
    fn oversampler_removes_images_near_the_doubled_rate() {
        // A tone at 20 kHz upsampled must not leave an image at 76 kHz: the
        // odd 2x samples equal the even ones' neighbours closely only if the
        // spectrum is clean. Check via the down pass of a 2x-rate tone above
        // the base Nyquist, which must be strongly attenuated.
        let mut os = Oversampler2x::default();
        let hz = 35_000.0; // at the 96 kHz doubled rate
        let mut peak: f32 = 0.0;
        for i in 0..20_000 {
            // f64 phase: an f32 argument this large is too coarse.
            let w = std::f64::consts::TAU * hz / (2.0 * SR as f64);
            let a = (w * (2 * i) as f64).sin() as f32;
            let b = (w * (2 * i + 1) as f64).sin() as f32;
            let y = os.down([a, b]);
            if i > 1_000 {
                peak = peak.max(y.abs());
            }
        }
        assert!(peak < 1.0e-3, "{peak}");
    }
}
