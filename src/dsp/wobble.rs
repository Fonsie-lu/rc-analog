//! WOBBLE: pitch and speed instability of a tape transport. Wow (slow) and
//! flutter (fast) modulate a delay line read position; the Balance slider
//! shares the depth between them. Stereo moves the right channel's wow a
//! quarter cycle away, which turns the module into a chorus, especially with
//! Mix below 100 %. Flux replaces part of the regular LFOs with smoothed
//! random motion and lets the rates wander, like a real worn transport.
//!
//! The depth is a pitch deviation, so the delay swing shrinks as the rate
//! goes up: the same knob position sounds equally deep at any rate.

use super::common::{engage, fast_sin01, DelayLine, Drift, Ramp, CHUNK, PI};

#[derive(Clone, Copy, Debug)]
pub struct WobbleParams {
    pub on: bool,
    pub amount: f32,
    /// 0 = all wow, 1 = all flutter.
    pub balance: f32,
    pub wow_rate: f32,
    pub flutter_rate: f32,
    pub stereo: bool,
    pub mix: f32,
    pub flux: f32,
}

/// Peak pitch deviation at full depth, as a fraction (0.02 = 2 %, ~34 cents).
const WOW_DEVIATION: f32 = 0.02;
const FLUTTER_DEVIATION: f32 = 0.006;
/// Longest wow swing, seconds peak to peak.
const MAX_SWING: f32 = 0.05;
/// Fixed read offset so the Hermite read always has a sample ahead.
const BASE_DELAY: f32 = 2.0;

pub struct Wobble {
    sr: f32,
    lines: [DelayLine; 2],
    wow_phase: f32,
    flutter_phase: f32,
    wow_drift: [Drift; 2],
    flutter_drift: Drift,
    rate_drift: Drift,
    wow_swing: Ramp,
    flutter_swing: Ramp,
    wet: Ramp,
    wow_inc: f32,
    flutter_inc: f32,
    stereo: bool,
    flux: f32,
    last_delay: f32,
    activity: f32,
}

impl Default for Wobble {
    fn default() -> Self {
        let mut w = Self {
            sr: 48_000.0,
            lines: [DelayLine::default(), DelayLine::default()],
            wow_phase: 0.0,
            flutter_phase: 0.0,
            wow_drift: [Drift::new(21), Drift::new(22)],
            flutter_drift: Drift::new(23),
            rate_drift: Drift::new(24),
            wow_swing: Ramp::new(0.0),
            flutter_swing: Ramp::new(0.0),
            wet: Ramp::new(0.0),
            wow_inc: 0.0,
            flutter_inc: 0.0,
            stereo: false,
            flux: 0.0,
            last_delay: BASE_DELAY,
            activity: 0.0,
        };
        w.prepare(48_000.0);
        w
    }
}

impl Wobble {
    pub fn prepare(&mut self, sample_rate: f32) {
        self.sr = sample_rate;
        let max = (MAX_SWING * sample_rate) as usize + 64;
        for line in &mut self.lines {
            line.prepare(max);
        }
        for d in &mut self.wow_drift {
            d.set_rate(0.8, sample_rate);
        }
        self.flutter_drift.set_rate(7.0, sample_rate);
        self.rate_drift.set_rate(0.3, sample_rate / CHUNK as f32);
        self.reset();
    }

    pub fn reset(&mut self) {
        for line in &mut self.lines {
            line.reset();
        }
        self.wow_phase = 0.0;
        self.flutter_phase = 0.0;
        for d in &mut self.wow_drift {
            d.reset();
        }
        self.flutter_drift.reset();
        self.rate_drift.reset();
        self.last_delay = BASE_DELAY;
        self.activity = 0.0;
    }

    pub fn set(&mut self, p: &WobbleParams, n: usize) {
        let sr = self.sr;
        let depth = p.amount * p.amount.sqrt();
        let wow_share = (2.0 * (1.0 - p.balance)).min(1.0);
        let flutter_share = (2.0 * p.balance).min(1.0);
        let wander = 1.0 + 0.35 * p.flux * self.rate_drift.next();
        let wow_hz = (p.wow_rate * wander).max(0.05);
        let flutter_hz = (p.flutter_rate * wander).max(1.0);
        self.wow_inc = wow_hz / sr;
        self.flutter_inc = flutter_hz / sr;
        // Deviation d of a sinusoidal delay with swing S (peak to peak) at f:
        // d = pi * f * S, so S = d / (pi f).
        let wow = (depth * wow_share * WOW_DEVIATION / (PI * wow_hz)).min(MAX_SWING) * sr;
        let flutter = depth * flutter_share * FLUTTER_DEVIATION / (PI * flutter_hz) * sr;
        self.wow_swing.set(wow, n);
        self.flutter_swing.set(flutter, n);
        self.wet.set(if p.on { engage(p.amount, 0.02) * p.mix } else { 0.0 }, n);
        self.stereo = p.stereo;
        self.flux = p.flux;
    }

    pub fn is_silent(&self) -> bool {
        self.wet.is_silent()
    }

    /// 0 .. 1 for the editor: the momentary pitch deviation relative to the
    /// wow maximum.
    pub fn activity(&self) -> f32 {
        (self.activity / WOW_DEVIATION).clamp(0.0, 1.0)
    }

    pub fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        let n = left.len().min(right.len());
        if self.wet.is_silent() {
            // Keep the lines filled so switching on does not read stale audio.
            for i in 0..n {
                self.lines[0].push(left[i]);
                self.lines[1].push(right[i]);
            }
            self.wow_swing.set(self.wow_swing.value(), 0);
            self.flutter_swing.set(self.flutter_swing.value(), 0);
            self.activity *= 0.9;
            return;
        }
        let flux = self.flux;
        let regular = 1.0 - 0.6 * flux;
        let mut deviation: f32 = 0.0;
        for i in 0..n {
            let wow_swing = self.wow_swing.next();
            let flutter_swing = self.flutter_swing.next();
            let wet = self.wet.next();
            let ph = self.wow_phase;
            self.wow_phase = (ph + self.wow_inc).fract();
            let fph = self.flutter_phase;
            self.flutter_phase = (fph + self.flutter_inc).fract();

            let flutter = ((0.7 * fast_sin01(fph) + 0.3 * fast_sin01(fph * 2.0 + 0.13)) * (1.0 - 0.5 * flux)
                + 0.6 * flux * self.flutter_drift.next())
            .clamp(-1.0, 1.0);
            let wow_l = (regular * fast_sin01(ph) + flux * self.wow_drift[0].next()).clamp(-1.0, 1.0);
            let wow_r = if self.stereo {
                (regular * fast_sin01(ph + 0.25) + flux * self.wow_drift[1].next()).clamp(-1.0, 1.0)
            } else {
                wow_l
            };
            let common = BASE_DELAY + flutter_swing * (0.5 + 0.5 * flutter);
            let d_l = common + wow_swing * (0.5 + 0.5 * wow_l);
            let d_r = common + wow_swing * (0.5 + 0.5 * wow_r);

            self.lines[0].push(left[i]);
            self.lines[1].push(right[i]);
            let wl = self.lines[0].read(d_l);
            let wr = self.lines[1].read(d_r);
            left[i] += (wl - left[i]) * wet;
            right[i] += (wr - right[i]) * wet;

            deviation = deviation.max((d_l - self.last_delay).abs());
            self.last_delay = d_l;
        }
        self.activity = self.activity * 0.7 + deviation * 0.3;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dsp::common::TWO_PI;

    fn params(amount: f32, balance: f32) -> WobbleParams {
        WobbleParams { on: true, amount, balance, wow_rate: 1.0, flutter_rate: 9.0, stereo: false, mix: 1.0, flux: 0.0 }
    }

    /// Instantaneous frequency of a wobbled 1 kHz sine, from zero crossings.
    fn crossing_rates(p: WobbleParams) -> (f32, f32) {
        let sr = 48_000.0;
        let mut w = Wobble::default();
        let n = 96_000;
        let mut l: Vec<f32> = (0..n).map(|i| (TWO_PI * 1_000.0 * i as f32 / sr).sin()).collect();
        let mut r = l.clone();
        for (cl, cr) in l.chunks_mut(CHUNK).zip(r.chunks_mut(CHUNK)) {
            w.set(&p, cl.len());
            w.process(cl, cr);
        }
        let mut last = None;
        let (mut lo, mut hi) = (f32::MAX, 0.0f32);
        for i in 24_000..n - 1 {
            if l[i] <= 0.0 && l[i + 1] > 0.0 {
                let t = i as f32 + l[i] / (l[i] - l[i + 1]);
                if let Some(prev) = last {
                    let f = sr / (t - prev);
                    lo = lo.min(f);
                    hi = hi.max(f);
                }
                last = Some(t);
            }
        }
        (lo, hi)
    }

    #[test]
    fn zero_depth_is_a_bypass() {
        let mut w = Wobble::default();
        let mut l: Vec<f32> = (0..1_000).map(|i| (i as f32 * 0.01).sin()).collect();
        let orig = l.clone();
        let mut r = l.clone();
        for (cl, cr) in l.chunks_mut(CHUNK).zip(r.chunks_mut(CHUNK)) {
            w.set(&params(0.0, 0.3), cl.len());
            w.process(cl, cr);
        }
        assert_eq!(l, orig);
    }

    #[test]
    fn full_wow_bends_pitch_by_about_two_percent() {
        let (lo, hi) = crossing_rates(params(1.0, 0.0));
        assert!(lo < 990.0 && hi > 1_010.0, "{lo} {hi}");
        assert!(lo > 960.0 && hi < 1_040.0, "{lo} {hi}");
    }

    #[test]
    fn flutter_is_shallower_than_wow() {
        let (lo, hi) = crossing_rates(params(1.0, 1.0));
        assert!(hi - lo > 4.0 && hi - lo < 20.0, "{lo} {hi}");
    }
}
