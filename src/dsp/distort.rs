//! DISTORT: saturation in five flavours, confined to a focus band. The band
//! between Focus Low and Focus High is driven through the shaper at twice the
//! sample rate; what lies outside the band passes untouched (delayed to stay
//! aligned). An RMS follower takes most of the level change back out, so the
//! big knob changes character more than loudness. Flux wobbles the drive.
//!
//! The 2x oversampler adds [`LATENCY`] samples. The module always delays by
//! that much, on or off, so the plugin reports one fixed latency.

use super::common::{engage, fast_db_to_gain, fast_pow, fast_tanh, one_pole_coeff, DelayLine, Drift, Ramp, CHUNK};
use super::filters::{Bands, DcBlocker, FocusSplit, OnePole, Oversampler2x, Svf, BUTTERWORTH_K, OVERSAMPLE_LATENCY};
use super::params::DistortType;

pub const LATENCY: usize = OVERSAMPLE_LATENCY;

#[derive(Clone, Copy, Debug)]
pub struct DistortParams {
    pub on: bool,
    pub amount: f32,
    pub kind: DistortType,
    pub focus_lo: f32,
    pub focus_hi: f32,
    pub mix: f32,
    pub flux: f32,
}

/// Most drive the big knob adds, in dB.
const MAX_DRIVE_DB: f32 = 36.0;

#[derive(Clone, Debug, Default)]
struct Channel {
    split: FocusSplit,
    os: Oversampler2x,
    /// Tape-style pre-emphasis before and de-emphasis after the shaper (2x rate).
    emphasis_in: OnePole,
    emphasis_out: OnePole,
    /// Cassette band limit (2x rate).
    cassette_lp: Svf,
    dc: DcBlocker,
    /// Delays dry and out-of-band signal by the oversampler latency.
    dry: DelayLine,
    rest: DelayLine,
    env_in: f32,
    env_out: f32,
}

pub struct Distort {
    sr: f32,
    kind: DistortType,
    ch: [Channel; 2],
    drive: Ramp,
    wet: Ramp,
    flux: Drift,
    flux_amount: f32,
    env_coeff: f32,
    makeup: f32,
    activity: f32,
}

impl Default for Distort {
    fn default() -> Self {
        let mut d = Self {
            sr: 48_000.0,
            kind: DistortType::Tape,
            ch: [Channel::default(), Channel::default()],
            drive: Ramp::new(1.0),
            wet: Ramp::new(0.0),
            flux: Drift::new(31),
            flux_amount: 0.0,
            env_coeff: 0.0,
            makeup: 1.0,
            activity: 0.0,
        };
        d.prepare(48_000.0);
        d
    }
}

/// The shapers, all with unity slope at zero and a ceiling near 1.
#[inline(always)]
fn shape(kind: DistortType, x: f32) -> f32 {
    match kind {
        // Gentle and nearly symmetric, a whisper of even harmonics.
        DistortType::Tape => fast_tanh(x + 0.08) - 0.079_83,
        // Biased: positive half compresses later than the negative one.
        DistortType::Tube => {
            if x >= 0.0 {
                fast_tanh(x * 0.8) * 1.25
            } else {
                fast_tanh(x * 1.6) * 0.625
            }
        }
        DistortType::Cassette => {
            let y = fast_tanh(x * 1.4) / 1.4;
            y + 0.08 * y * y
        }
        // Cubic soft clip with a hard ceiling.
        DistortType::Transistor => {
            let c = x.clamp(-1.0, 1.0);
            1.5 * c - 0.5 * c * c * c
        }
        // Asymmetric hard clip plus a touch of rectification.
        DistortType::Fuzz => {
            let c = (x * 2.0).clamp(-0.45, 1.0);
            c + 0.25 * c.abs()
        }
    }
}

impl Distort {
    pub fn prepare(&mut self, sample_rate: f32) {
        self.sr = sample_rate;
        for ch in &mut self.ch {
            ch.dry.prepare(LATENCY + 4);
            ch.rest.prepare(LATENCY + 4);
            ch.dc.set(sample_rate);
            ch.emphasis_in.set(2_500.0, 2.0 * sample_rate);
            ch.emphasis_out.set(2_500.0, 2.0 * sample_rate);
            ch.cassette_lp.set(9_000.0, BUTTERWORTH_K, 2.0 * sample_rate);
        }
        self.flux.set_rate(4.0, sample_rate / CHUNK as f32);
        self.env_coeff = one_pole_coeff(3.0, sample_rate);
        self.reset();
    }

    pub fn reset(&mut self) {
        for ch in &mut self.ch {
            ch.split.reset();
            ch.os.reset();
            ch.emphasis_in.reset();
            ch.emphasis_out.reset();
            ch.cassette_lp.reset();
            ch.dc.reset();
            ch.dry.reset();
            ch.rest.reset();
            ch.env_in = 0.0;
            ch.env_out = 0.0;
        }
        self.flux.reset();
        self.makeup = 1.0;
        self.activity = 0.0;
    }

    pub fn set(&mut self, p: &DistortParams, n: usize) {
        self.kind = p.kind;
        let flux_db = p.flux * 12.0 * self.flux.next();
        let drive_db = (p.amount * MAX_DRIVE_DB + flux_db).max(-12.0);
        self.drive.set(fast_db_to_gain(drive_db), n);
        self.wet.set(if p.on { engage(p.amount, 0.02) * p.mix } else { 0.0 }, n);
        self.flux_amount = p.flux;
        // Makeup from the louder channel's envelopes, once per chunk (never
        // per process call, so the result does not depend on block sizes).
        let (ein, eout) = self.ch.iter().fold((0.0f32, 0.0f32), |(a, b), c| (a.max(c.env_in), b.max(c.env_out)));
        let target = if eout > 1.0e-9 && ein > 1.0e-9 { fast_pow(ein / eout, 0.425).clamp(0.06, 2.0) } else { self.makeup };
        self.makeup += 0.15 * (target - self.makeup);
        self.activity = (1.0 - self.makeup).max(0.0);
        let sr = self.sr;
        for ch in &mut self.ch {
            ch.split.set(p.focus_lo, p.focus_hi, sr);
        }
    }

    /// 0 .. 1 for the editor: gain reduction the level match applies.
    pub fn activity(&self) -> f32 {
        self.activity.clamp(0.0, 1.0)
    }

    pub fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        let n = left.len().min(right.len());
        if self.wet.is_silent() {
            // Off: a pure delay, so the latency stays what was reported.
            for i in 0..n {
                for (ch, x) in self.ch.iter_mut().zip([&mut left[i], &mut right[i]]) {
                    ch.dry.push(*x);
                    *x = ch.dry.tap(LATENCY);
                }
            }
            self.activity *= 0.9;
            return;
        }
        let kind = self.kind;
        let env_coeff = self.env_coeff;
        for i in 0..n {
            let drive = self.drive.next();
            let wet = self.wet.next();
            for (ch, x) in self.ch.iter_mut().zip([&mut left[i], &mut right[i]]) {
                let input = *x;
                let Bands { band, rest } = ch.split.split(input);
                ch.dry.push(input);
                ch.rest.push(rest);

                let mut up = ch.os.up(band * drive);
                for s in &mut up {
                    let mut v = *s;
                    if matches!(kind, DistortType::Tape | DistortType::Cassette) {
                        // Pre-emphasis: highs saturate first, as on tape.
                        let lo = ch.emphasis_in.lp(v);
                        v = lo + (v - lo) * 2.0;
                    }
                    let mut y = shape(kind, v);
                    if matches!(kind, DistortType::Tape | DistortType::Cassette) {
                        let lo = ch.emphasis_out.lp(y);
                        y = lo + (y - lo) * 0.5;
                    }
                    if kind == DistortType::Cassette {
                        y = ch.cassette_lp.lp(y);
                    }
                    *s = y;
                }
                let shaped = ch.dc.process(ch.os.down(up));

                // Level match on the band: follow input and output RMS.
                let band_in = band;
                ch.env_in += env_coeff * (band_in * band_in - ch.env_in);
                ch.env_out += env_coeff * (shaped * shaped - ch.env_out);

                let dry = ch.dry.tap(LATENCY);
                let processed = ch.rest.tap(LATENCY) + shaped * self.makeup;
                *x = dry + (processed - dry) * wet;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dsp::common::TWO_PI;

    fn params(amount: f32, kind: DistortType) -> DistortParams {
        DistortParams { on: true, amount, kind, focus_lo: 20.0, focus_hi: 20_000.0, mix: 1.0, flux: 0.0 }
    }

    fn run(p: DistortParams, input: &[f32]) -> Vec<f32> {
        let mut d = Distort::default();
        let mut l = input.to_vec();
        let mut r = input.to_vec();
        for (cl, cr) in l.chunks_mut(CHUNK).zip(r.chunks_mut(CHUNK)) {
            d.set(&p, cl.len());
            d.process(cl, cr);
        }
        l
    }

    #[test]
    fn off_and_zero_drive_are_a_pure_delay() {
        let input: Vec<f32> = (0..2_000).map(|i| (i as f32 * 0.03).sin() * 0.8).collect();
        for p in [params(0.0, DistortType::Fuzz), DistortParams { on: false, ..params(1.0, DistortType::Fuzz) }] {
            let out = run(p, &input);
            for i in LATENCY..input.len() {
                assert_eq!(out[i], input[i - LATENCY], "{i}");
            }
        }
    }

    #[test]
    fn every_type_adds_harmonics_and_stays_bounded() {
        let sr = 48_000.0;
        let input: Vec<f32> = (0..48_000).map(|i| (TWO_PI * 200.0 * i as f32 / sr).sin() * 0.5).collect();
        for kind in DistortType::ALL {
            let out = run(params(0.8, kind), &input);
            let peak = out.iter().fold(0.0f32, |a, b| a.max(b.abs()));
            assert!(peak < 2.0 && peak > 0.05, "{kind:?} {peak}");
            // Distortion: the output is no longer a scaled copy of the input.
            let tail = &out[24_000..];
            let src = &input[24_000 - LATENCY..48_000 - LATENCY];
            let dot: f32 = tail.iter().zip(src).map(|(a, b)| a * b).sum();
            let ee: f32 = tail.iter().map(|a| a * a).sum();
            let ss: f32 = src.iter().map(|a| a * a).sum();
            let corr = dot / (ee * ss).sqrt();
            assert!(corr < 0.995, "{kind:?} still clean: {corr}");
        }
    }

    #[test]
    fn focus_band_leaves_the_lows_alone() {
        let sr = 48_000.0;
        let input: Vec<f32> = (0..48_000).map(|i| (TWO_PI * 60.0 * i as f32 / sr).sin() * 0.7).collect();
        let mut p = params(1.0, DistortType::Fuzz);
        p.focus_lo = 2_000.0;
        let out = run(p, &input);
        for i in 30_000..48_000 {
            assert!((out[i] - input[i - LATENCY]).abs() < 0.05, "{i}");
        }
    }
}
