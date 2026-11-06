//! DIGITAL: early-sampler grit. The band between Focus Low and Focus High is
//! sample-and-held at a reduced rate and quantised to fewer bits; the big knob
//! sets how far, the Rate/Bits slider how that is shared between the two.
//! Smooth lowpasses before and after the hold (a gentler, filtered sampler);
//! Cut drops everything outside the focus band. Flux jitters the sample
//! clock, lets rate and depth wander and, past half way, makes the converter
//! sputter (held samples drop out).

use super::common::{engage, fast_exp2, Drift, Ramp, Rng, CHUNK};
use super::filters::{Bands, FocusSplit, Svf, BUTTERWORTH_K};

#[derive(Clone, Copy, Debug)]
pub struct DigitalParams {
    pub on: bool,
    pub amount: f32,
    /// 0 = all sample-rate reduction, 1 = all bit reduction.
    pub balance: f32,
    pub smooth: f32,
    pub focus_lo: f32,
    pub focus_hi: f32,
    pub cut: bool,
    pub mix: f32,
    pub flux: f32,
}

/// Octaves the rate drops at full reduction (48 kHz -> 750 Hz).
const RATE_OCTAVES: f32 = 6.0;
const MAX_BITS: f32 = 16.0;
const MIN_BITS: f32 = 2.5;

#[derive(Clone, Copy, Debug, Default)]
struct Channel {
    split: FocusSplit,
    pre: Svf,
    post: Svf,
    held: f32,
}

pub struct Digital {
    sr: f32,
    ch: [Channel; 2],
    phase: f32,
    inc: f32,
    base_inc: f32,
    levels: f32,
    smooth: f32,
    cut: bool,
    flux: f32,
    muted: bool,
    rng: Rng,
    rate_drift: Drift,
    bits_drift: Drift,
    wet: Ramp,
    rate_hz: f32,
    bits: f32,
}

impl Default for Digital {
    fn default() -> Self {
        let mut d = Self {
            sr: 48_000.0,
            ch: [Channel::default(); 2],
            phase: 0.0,
            inc: 1.0,
            base_inc: 1.0,
            levels: 32_768.0,
            smooth: 0.0,
            cut: false,
            flux: 0.0,
            muted: false,
            rng: Rng::new(41),
            rate_drift: Drift::new(42),
            bits_drift: Drift::new(43),
            wet: Ramp::new(0.0),
            rate_hz: 48_000.0,
            bits: MAX_BITS,
        };
        d.prepare(48_000.0);
        d
    }
}

impl Digital {
    pub fn prepare(&mut self, sample_rate: f32) {
        self.sr = sample_rate;
        let chunk_rate = sample_rate / CHUNK as f32;
        self.rate_drift.set_rate(0.5, chunk_rate);
        self.bits_drift.set_rate(0.3, chunk_rate);
        self.reset();
    }

    pub fn reset(&mut self) {
        for ch in &mut self.ch {
            ch.split.reset();
            ch.pre.reset();
            ch.post.reset();
            ch.held = 0.0;
        }
        self.phase = 0.0;
        self.muted = false;
        self.rng.seed(41);
        self.rate_drift.reset();
        self.bits_drift.reset();
    }

    pub fn set(&mut self, p: &DigitalParams, n: usize) {
        let sr = self.sr;
        let rate_amount = p.amount * (2.0 * (1.0 - p.balance)).min(1.0);
        let bits_amount = p.amount * (2.0 * p.balance).min(1.0);
        let octaves = (rate_amount * RATE_OCTAVES + p.flux * 0.8 * self.rate_drift.next()).max(0.0);
        self.rate_hz = sr * fast_exp2(-octaves);
        self.base_inc = (self.rate_hz / sr).min(1.0);
        self.bits = (MAX_BITS - bits_amount * (MAX_BITS - MIN_BITS) + p.flux * 1.5 * self.bits_drift.next()).clamp(MIN_BITS - 0.5, MAX_BITS);
        self.levels = fast_exp2(self.bits - 1.0);
        self.smooth = p.smooth;
        self.cut = p.cut;
        self.flux = p.flux;
        let open = (0.45 * sr).min(20_000.0);
        let cutoff = open + (0.45 * self.rate_hz - open) * p.smooth;
        for ch in &mut self.ch {
            ch.split.set(p.focus_lo, p.focus_hi, sr);
            ch.pre.set(cutoff, BUTTERWORTH_K, sr);
            ch.post.set(cutoff, BUTTERWORTH_K, sr);
        }
        self.wet.set(if p.on { engage(p.amount, 0.02) * p.mix } else { 0.0 }, n);
    }

    /// Effective sample rate (Hz) and bit depth, for the editor.
    pub fn status(&self) -> (f32, f32) {
        (self.rate_hz, self.bits)
    }

    /// 0 .. 1 for the editor: how far rate and bits are reduced.
    pub fn activity(&self) -> f32 {
        if self.wet.is_silent() {
            return 0.0;
        }
        let rate = (self.sr / self.rate_hz).log2() / RATE_OCTAVES;
        let bits = (MAX_BITS - self.bits) / (MAX_BITS - MIN_BITS);
        rate.max(bits).clamp(0.0, 1.0)
    }

    pub fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        let n = left.len().min(right.len());
        if self.wet.is_silent() {
            return;
        }
        let smooth = self.smooth;
        let sputter = (self.flux - 0.5).max(0.0) * 0.08;
        let jitter = self.flux * 0.6;
        for i in 0..n {
            let wet = self.wet.next();
            self.phase += self.inc;
            let capture = self.phase >= 1.0;
            if capture {
                self.phase -= self.phase.floor();
                self.inc = self.base_inc * (1.0 + jitter * self.rng.next_bipolar());
                self.muted = sputter > 0.0 && self.rng.next_float() < sputter;
            }
            for (ch, x) in self.ch.iter_mut().zip([&mut left[i], &mut right[i]]) {
                let input = *x;
                let Bands { band, rest } = ch.split.split(input);
                let filtered = ch.pre.lp(band);
                let src = band + (filtered - band) * smooth;
                if capture {
                    ch.held = if self.muted { 0.0 } else { (src * self.levels).round() / self.levels };
                }
                let out = ch.post.lp(ch.held);
                let crushed = ch.held + (out - ch.held) * smooth;
                let processed = if self.cut { crushed } else { rest + crushed };
                *x = input + (processed - input) * wet;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dsp::common::TWO_PI;

    fn params(amount: f32, balance: f32) -> DigitalParams {
        DigitalParams { on: true, amount, balance, smooth: 0.0, focus_lo: 20.0, focus_hi: 20_000.0, cut: false, mix: 1.0, flux: 0.0 }
    }

    fn run(p: DigitalParams, input: &[f32]) -> Vec<f32> {
        let mut d = Digital::default();
        let mut l = input.to_vec();
        let mut r = input.to_vec();
        for (cl, cr) in l.chunks_mut(CHUNK).zip(r.chunks_mut(CHUNK)) {
            d.set(&p, cl.len());
            d.process(cl, cr);
        }
        l
    }

    fn sine(hz: f32, n: usize) -> Vec<f32> {
        (0..n).map(|i| (TWO_PI * hz * i as f32 / 48_000.0).sin() * 0.8).collect()
    }

    #[test]
    fn zero_amount_is_a_bypass() {
        let input = sine(440.0, 4_000);
        assert_eq!(run(params(0.0, 0.5), &input), input);
    }

    #[test]
    fn bit_reduction_leaves_few_levels() {
        // Cut keeps only the crushed band, so the levels can be counted exactly.
        let mut p = params(1.0, 1.0);
        p.cut = true;
        let out = run(p, &sine(440.0, 48_000));
        let mut levels: Vec<i64> = out[10_000..].iter().map(|v| (v * 1.0e4).round() as i64).collect();
        levels.sort();
        levels.dedup();
        assert!(levels.len() <= 7, "{levels:?}");
    }

    #[test]
    fn rate_reduction_holds_samples() {
        let mut p = params(1.0, 0.0);
        p.cut = true;
        let out = run(p, &sine(100.0, 48_000));
        // At 750 Hz a value is held for 64 samples.
        let steps = out[10_000..].windows(2).filter(|w| (w[0] - w[1]).abs() > 1.0e-4).count();
        assert!((500..700).contains(&steps), "{steps}");
    }

    #[test]
    fn cut_removes_what_is_outside_the_focus() {
        let mut p = params(0.3, 0.5);
        p.cut = true;
        p.focus_lo = 2_000.0;
        p.focus_hi = 6_000.0;
        let out = run(p, &sine(80.0, 48_000));
        let peak = out[20_000..].iter().fold(0.0f32, |a, b| a.max(b.abs()));
        assert!(peak < 0.05, "{peak}");
    }
}
