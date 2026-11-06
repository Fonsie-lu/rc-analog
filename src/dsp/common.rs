//! Shared small helpers for the DSP core. Nothing here allocates, and
//! everything that runs per sample avoids the general `exp2`/`sin`/`tanh`/`powf`.

pub const PI: f32 = std::f32::consts::PI;
pub const TWO_PI: f32 = std::f32::consts::TAU;

/// Samples per control chunk: parameters are smoothed and filter coefficients
/// recomputed once per chunk; gains ramp linearly across it.
pub const CHUNK: usize = 32;

#[inline(always)]
pub fn clamp01(v: f32) -> f32 {
    v.clamp(0.0, 1.0)
}

#[inline(always)]
pub fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

/// tanh approximation (Pade-style); error below 5e-3 for |x| <= 3, up to 3e-2 at the clamp edge.
#[inline(always)]
pub fn fast_tanh(x: f32) -> f32 {
    let c = x.clamp(-5.0, 5.0);
    let x2 = c * c;
    c * (27.0 + x2) / (27.0 + 9.0 * x2)
}

pub fn db_to_gain(db: f32) -> f32 {
    10.0_f32.powf(db * 0.05)
}

pub fn gain_to_db(g: f32) -> f32 {
    20.0 * g.max(1.0e-9).log10()
}

/// dB to linear gain through [`fast_exp2`]; audio-thread use.
#[inline(always)]
pub fn fast_db_to_gain(db: f32) -> f32 {
    // 10^(db/20) = 2^(db * log2(10) / 20)
    fast_exp2(db * 0.166_096_4)
}

/// 2^x with ~1e-4 relative error: exponent by bit manipulation, mantissa by a
/// minimax polynomial.
#[inline(always)]
pub fn fast_exp2(x: f32) -> f32 {
    let c = x.clamp(-126.0, 126.0);
    let whole = c.floor();
    let frac = c - whole;
    let poly = 1.0
        + frac
            * (std::f32::consts::LN_2
                + frac * (0.240_226_5 + frac * (0.055_504_1 + frac * (0.009_618_1 + frac * 0.001_333_3))));
    let scale = f32::from_bits((((whole as i32) + 127) << 23) as u32);
    poly * scale
}

/// log2(x) for x > 0: exponent straight from the bits, mantissa from a
/// 5th-order minimax polynomial.
#[inline(always)]
#[allow(clippy::excessive_precision)]
pub fn fast_log2(x: f32) -> f32 {
    if x <= 0.0 {
        return -126.0;
    }
    let bits = x.to_bits();
    let exponent = ((bits >> 23) & 0xFF) as i32 - 127;
    let m = f32::from_bits((bits & 0x007F_FFFF) | 0x3F80_0000);
    let poly = -2.800_338_6
        + m * (5.091_631_6 + m * (-3.550_697_2 + m * (1.631_092_9 + m * (-0.416_548_03 + m * 0.044_871_943))));
    exponent as f32 + poly
}

/// x^y for x >= 0.
#[inline(always)]
pub fn fast_pow(x: f32, y: f32) -> f32 {
    if x <= 0.0 {
        return 0.0;
    }
    fast_exp2(y * fast_log2(x))
}

/// sin(2*pi*p) for any p via a 7th-order odd polynomial (error < 1e-4).
#[inline(always)]
pub fn fast_sin01(p: f32) -> f32 {
    let mut x = p - p.floor();
    let mut sign = 1.0;
    if x > 0.5 {
        x -= 0.5;
        sign = -1.0;
    }
    if x > 0.25 {
        x = 0.5 - x;
    }
    let t = x * TWO_PI;
    let t2 = t * t;
    sign * t * (1.0 - t2 * (0.166_666_6 - t2 * (0.008_333_3 - t2 * 0.000_198_4)))
}

/// tan(pi * x) for x in [0, 0.49], the bilinear prewarp of a cutoff given as
/// a fraction of the sample rate. Built from [`fast_sin01`], so it is cheap
/// enough to run once per chunk for every filter.
#[inline(always)]
pub fn fast_tan_pi(x: f32) -> f32 {
    let x = x.clamp(1.0e-6, 0.49);
    fast_sin01(x * 0.5) / fast_sin01(x * 0.5 + 0.25)
}

/// Coefficient of a one-pole lowpass `y += a * (x - y)` with cutoff `hz`.
#[inline(always)]
pub fn one_pole_coeff(hz: f32, sample_rate: f32) -> f32 {
    // 1 - e^(-2 pi f / fs), e^-x = 2^(-x / ln 2)
    let w = TWO_PI * hz / sample_rate;
    (1.0 - fast_exp2(-w * std::f32::consts::LOG2_E)).clamp(0.0, 1.0)
}

/// 4-point Hermite interpolation between `x0` and `x1`.
#[inline(always)]
pub fn hermite(xm1: f32, x0: f32, x1: f32, x2: f32, t: f32) -> f32 {
    let c = (x1 - xm1) * 0.5;
    let v = x0 - x1;
    let w = c + v;
    let a = w + v + (x2 - x0) * 0.5;
    let b_neg = w + a;
    (((a * t) - b_neg) * t + c) * t + x0
}

/// Cubic fade-in shape: 0 at 0, 1 at and above `knee`. Modules use it so an
/// amount of exactly 0 is a true bypass while small amounts still fade in.
#[inline(always)]
pub fn engage(amount: f32, knee: f32) -> f32 {
    let t = (amount / knee).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// A gain that moves linearly from its value to a target across one chunk,
/// so per-chunk parameter updates never step.
#[derive(Clone, Copy, Debug, Default)]
pub struct Ramp {
    current: f32,
    step: f32,
    remaining: usize,
}

impl Ramp {
    pub fn new(v: f32) -> Self {
        Self { current: v, step: 0.0, remaining: 0 }
    }
    /// Snap to `v` without a ramp.
    pub fn reset(&mut self, v: f32) {
        *self = Self::new(v);
    }
    /// Ramp to `target` over `samples` samples.
    pub fn set(&mut self, target: f32, samples: usize) {
        if samples == 0 || target == self.current {
            self.reset(target);
            return;
        }
        self.step = (target - self.current) / samples as f32;
        self.remaining = samples;
    }
    #[inline(always)]
    #[allow(clippy::should_implement_trait)]
    pub fn next(&mut self) -> f32 {
        if self.remaining > 0 {
            self.current += self.step;
            self.remaining -= 1;
        }
        self.current
    }
    #[inline(always)]
    pub fn value(&self) -> f32 {
        self.current
    }
    /// True when the ramp sits at zero and is not moving.
    pub fn is_silent(&self) -> bool {
        self.remaining == 0 && self.current == 0.0
    }
}

/// Cheap deterministic RNG (xorshift32). Repeat renders must be bit-identical,
/// so every noise source seeds from the engine, never from the clock.
#[derive(Clone, Copy, Debug)]
pub struct Rng {
    state: u32,
}

impl Default for Rng {
    fn default() -> Self {
        Self::new(0x0123_4567)
    }
}

impl Rng {
    pub const fn new(seed: u32) -> Self {
        Self { state: if seed == 0 { 0x0123_4567 } else { seed } }
    }
    pub fn seed(&mut self, s: u32) {
        self.state = if s == 0 { 0x0123_4567 } else { s };
    }
    #[inline(always)]
    pub fn next_u32(&mut self) -> u32 {
        self.state ^= self.state << 13;
        self.state ^= self.state >> 17;
        self.state ^= self.state << 5;
        self.state
    }
    /// Uniform in [0, 1).
    #[inline(always)]
    pub fn next_float(&mut self) -> f32 {
        (self.next_u32() >> 8) as f32 * (1.0 / 16_777_216.0)
    }
    /// Uniform in [-1, 1).
    #[inline(always)]
    pub fn next_bipolar(&mut self) -> f32 {
        self.next_float() * 2.0 - 1.0
    }
}

/// Smoothly wandering random value in about -1 .. 1: white noise through two
/// one-pole lowpasses at `hz`, rescaled so the output keeps roughly unit
/// deviation whatever the rate. The "Flux" fluctuations are built from these.
#[derive(Clone, Copy, Debug)]
pub struct Drift {
    seed: u32,
    rng: Rng,
    a: f32,
    s1: f32,
    s2: f32,
    norm: f32,
}

impl Drift {
    pub fn new(seed: u32) -> Self {
        Self { seed, rng: Rng::new(seed), a: 0.0, s1: 0.0, s2: 0.0, norm: 1.0 }
    }
    /// `rate` is how often the value is advanced per second (the sample rate,
    /// or the chunk rate when stepped once per chunk).
    pub fn set_rate(&mut self, hz: f32, rate: f32) {
        self.a = one_pole_coeff(hz.max(0.01), rate);
        // Two cascaded one-poles of white noise with variance 1/3 have
        // variance ~ a / 6 for small a; scale back towards unit deviation.
        self.norm = (6.0 / self.a.max(1.0e-6)).sqrt() * 0.55;
    }
    /// Back to the start of its sequence, so renders after a reset repeat.
    pub fn reset(&mut self) {
        self.rng.seed(self.seed);
        self.s1 = 0.0;
        self.s2 = 0.0;
    }
    #[inline(always)]
    #[allow(clippy::should_implement_trait)]
    pub fn next(&mut self) -> f32 {
        let w = self.rng.next_bipolar();
        self.s1 += self.a * (w - self.s1);
        self.s2 += self.a * (self.s1 - self.s2);
        (self.s2 * self.norm).clamp(-2.5, 2.5)
    }
    #[inline(always)]
    pub fn value(&self) -> f32 {
        (self.s2 * self.norm).clamp(-2.5, 2.5)
    }
}

/// Fractional delay line with a power-of-two ring and Hermite reads.
#[derive(Clone, Debug, Default)]
pub struct DelayLine {
    buffer: Vec<f32>,
    mask: usize,
    write: usize,
}

impl DelayLine {
    /// Allocates; call from `prepare`, never from the audio thread.
    pub fn prepare(&mut self, max_samples: usize) {
        let size = (max_samples + 4).next_power_of_two();
        self.buffer.clear();
        self.buffer.resize(size, 0.0);
        self.mask = size - 1;
        self.write = 0;
    }

    pub fn reset(&mut self) {
        self.buffer.fill(0.0);
    }

    /// Longest delay a read may ask for.
    pub fn max_delay(&self) -> f32 {
        self.buffer.len().saturating_sub(4) as f32
    }

    #[inline(always)]
    pub fn push(&mut self, x: f32) {
        if self.buffer.is_empty() {
            return;
        }
        self.write = (self.write + 1) & self.mask;
        self.buffer[self.write] = x;
    }

    /// Sample written `delay` pushes ago (0 = the newest).
    #[inline(always)]
    pub fn tap(&self, delay: usize) -> f32 {
        if self.buffer.is_empty() {
            return 0.0;
        }
        self.buffer[(self.write.wrapping_sub(delay)) & self.mask]
    }

    /// Hermite read `delay` samples back; at least 1 for interpolation.
    #[inline(always)]
    pub fn read(&self, delay: f32) -> f32 {
        if self.buffer.is_empty() {
            return 0.0;
        }
        let d = delay.clamp(1.0, self.max_delay());
        let whole = d as usize;
        let frac = d - whole as f32;
        let x0 = self.tap(whole);
        let x1 = self.tap(whole + 1);
        let xm1 = self.tap(whole - 1);
        let x2 = self.tap(whole + 2);
        hermite(xm1, x0, x1, x2, frac)
    }

    /// Linear read, for the many reverb taps where Hermite costs too much.
    #[inline(always)]
    pub fn read_linear(&self, delay: f32) -> f32 {
        if self.buffer.is_empty() {
            return 0.0;
        }
        let d = delay.clamp(0.0, self.max_delay());
        let whole = d as usize;
        let frac = d - whole as f32;
        lerp(self.tap(whole), self.tap(whole + 1), frac)
    }
}

/// Sets flush-to-zero and denormals-are-zero for its lifetime and restores
/// the previous mode on drop. Reverb and filter tails decay into denormals,
/// which cost several times the normal speed on x86.
pub struct DenormalGuard {
    #[cfg(target_arch = "x86_64")]
    previous: u32,
}

impl DenormalGuard {
    #[allow(deprecated, clippy::new_without_default)]
    pub fn new() -> Self {
        #[cfg(target_arch = "x86_64")]
        {
            use std::arch::x86_64::{_mm_getcsr, _mm_setcsr};
            // SAFETY: only the FTZ (bit 15) and DAZ (bit 6) flags change; SSE is
            // always available on x86_64.
            let previous = unsafe { _mm_getcsr() };
            unsafe { _mm_setcsr(previous | 0x8040) };
            Self { previous }
        }
        #[cfg(not(target_arch = "x86_64"))]
        Self {}
    }
}

impl Drop for DenormalGuard {
    #[allow(deprecated)]
    fn drop(&mut self) {
        #[cfg(target_arch = "x86_64")]
        {
            use std::arch::x86_64::_mm_setcsr;
            // SAFETY: restores the mode read in `new`.
            unsafe { _mm_setcsr(self.previous) };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fast_math_matches_std() {
        for i in -200..200 {
            let x = i as f32 * 0.05;
            assert!((fast_exp2(x) / x.exp2() - 1.0).abs() < 2.0e-4, "exp2 {x}");
            assert!((fast_tanh(x * 0.5) - (x * 0.5).tanh()).abs() < 3.5e-2, "tanh {x}");
            assert!((fast_sin01(x) - (x * TWO_PI).sin()).abs() < 1.0e-3, "sin {x}");
        }
        for i in 1..400 {
            let x = i as f32 * 0.037;
            assert!((fast_log2(x) - x.log2()).abs() < 1.0e-3, "log2 {x}");
        }
        for i in 1..49 {
            let x = i as f32 * 0.01;
            let want = (PI * x).tan();
            assert!((fast_tan_pi(x) / want - 1.0).abs() < 2.0e-3, "tan {x}");
        }
        assert!((fast_db_to_gain(-6.0) - db_to_gain(-6.0)).abs() < 1.0e-4);
    }

    #[test]
    fn ramp_reaches_target_in_the_given_samples() {
        let mut r = Ramp::new(0.0);
        r.set(1.0, 4);
        let v: Vec<f32> = (0..6).map(|_| r.next()).collect();
        assert_eq!(v, [0.25, 0.5, 0.75, 1.0, 1.0, 1.0]);
        r.set(0.0, 2);
        r.next();
        r.next();
        assert!(r.is_silent());
    }

    #[test]
    fn delay_line_reads_back_what_was_written() {
        let mut d = DelayLine::default();
        d.prepare(100);
        for i in 0..50 {
            d.push(i as f32);
        }
        assert_eq!(d.tap(0), 49.0);
        assert_eq!(d.tap(10), 39.0);
        assert!((d.read(10.0) - 39.0).abs() < 1.0e-5);
        assert!((d.read(10.5) - 38.5).abs() < 1.0e-4);
    }

    #[test]
    fn drift_stays_bounded_and_moves() {
        let mut d = Drift::new(3);
        d.set_rate(2.0, 48_000.0);
        let mut lo: f32 = 0.0;
        let mut hi: f32 = 0.0;
        let mut sq = 0.0;
        let n = 480_000;
        for _ in 0..n {
            let v = d.next();
            lo = lo.min(v);
            hi = hi.max(v);
            sq += v * v;
        }
        let rms = (sq / n as f32).sqrt();
        assert!(hi > 0.3 && lo < -0.3, "{lo} {hi}");
        assert!((0.2..2.0).contains(&rms), "rms {rms}");
    }
}
