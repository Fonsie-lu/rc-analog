//! LOSS: what a worn tape does to level and top end. Wear is a slow, uneven
//! level fluctuation (oxide worn thin), Flutter a fast one (the capstan), and
//! the Wear/Flutter slider shares the depth between them. Dropouts are
//! sudden, short dips. Gap is spacing loss: the farther the tape rides off
//! the head, the more highs it loses, so the lowpass closes further whenever
//! wear or a dropout pulls the level down. Stereo gives each channel its own
//! wear and dropouts; Mono keeps them identical. Flux makes wear more
//! erratic and the flutter rate unsteady. The big knob scales wear, flutter
//! and dropouts together.

use super::common::{engage, fast_db_to_gain, fast_exp2, fast_sin01, one_pole_coeff, Drift, Ramp, Rng, CHUNK};
use super::filters::Svf;

#[derive(Clone, Copy, Debug)]
pub struct LossParams {
    pub on: bool,
    pub amount: f32,
    /// 0 = all wear, 1 = all flutter.
    pub balance: f32,
    pub dropouts: f32,
    pub gap: f32,
    pub stereo: bool,
    pub flux: f32,
}

/// Deepest wear dip and flutter dip at full amount, dB.
const WEAR_DB: f32 = 12.0;
const FLUTTER_DB: f32 = 6.0;
/// Dropout events per second at full Dropouts and amount.
const DROPOUT_RATE: f32 = 2.5;

#[derive(Clone, Copy, Debug)]
struct Modulators {
    wear_slow: Drift,
    wear_fast: Drift,
    rng: Rng,
    /// Current dropout gain (linear) and where it heads.
    dropout: f32,
    dropout_target: f32,
    hold: usize,
}

impl Modulators {
    fn new(seed: u32) -> Self {
        Self {
            wear_slow: Drift::new(seed),
            wear_fast: Drift::new(seed + 1),
            rng: Rng::new(seed + 2),
            dropout: 1.0,
            dropout_target: 1.0,
            hold: 0,
        }
    }

    fn reset(&mut self, seed: u32) {
        *self = Self { wear_slow: self.wear_slow, wear_fast: self.wear_fast, ..Self::new(seed) };
        self.wear_slow.reset();
        self.wear_fast.reset();
    }
}

#[derive(Clone, Copy, Debug, Default)]
struct Channel {
    gain: Ramp,
    lp: Svf,
}

pub struct Loss {
    sr: f32,
    mods: [Modulators; 2],
    ch: [Channel; 2],
    wet: Ramp,
    flutter_phase: f32,
    flutter_inc: f32,
    flutter_depth: f32,
    flutter_drift: Drift,
    attack: f32,
    release: f32,
    activity: f32,
}

impl Default for Loss {
    fn default() -> Self {
        let mut l = Self {
            sr: 48_000.0,
            mods: [Modulators::new(61), Modulators::new(71)],
            ch: [Channel::default(); 2],
            wet: Ramp::new(0.0),
            flutter_phase: 0.0,
            flutter_inc: 0.0,
            flutter_depth: 0.0,
            flutter_drift: Drift::new(81),
            attack: 0.0,
            release: 0.0,
            activity: 0.0,
        };
        l.prepare(48_000.0);
        l
    }
}

impl Loss {
    pub fn prepare(&mut self, sample_rate: f32) {
        self.sr = sample_rate;
        let chunk_rate = sample_rate / CHUNK as f32;
        for m in &mut self.mods {
            m.wear_slow.set_rate(0.5, chunk_rate);
            m.wear_fast.set_rate(3.0, chunk_rate);
        }
        self.flutter_drift.set_rate(1.5, chunk_rate);
        self.attack = one_pole_coeff(60.0, chunk_rate);
        self.release = one_pole_coeff(6.0, chunk_rate);
        self.reset();
    }

    pub fn reset(&mut self) {
        for (k, m) in self.mods.iter_mut().enumerate() {
            m.reset(61 + 10 * k as u32);
        }
        for ch in &mut self.ch {
            ch.gain.reset(1.0);
            ch.lp.reset();
        }
        self.flutter_phase = 0.0;
        self.flutter_drift.reset();
        self.activity = 0.0;
    }

    /// Advances wear and dropouts by one chunk; returns the gain (linear).
    fn step(m: &mut Modulators, p: &LossParams, depth: f32, wear_share: f32, chunk_s: f32, attack: f32, release: f32) -> f32 {
        let slow = m.wear_slow.next();
        let fast = m.wear_fast.next();
        let wear = (1.0 - 0.6 * p.flux) * slow + (0.25 + 0.75 * p.flux) * fast;
        let worn = (0.5 + 0.4 * wear).clamp(0.0, 1.0);
        let wear_gain = fast_db_to_gain(-WEAR_DB * depth * wear_share * worn);

        if m.hold > 0 {
            m.hold -= 1;
            if m.hold == 0 {
                m.dropout_target = 1.0;
            }
        } else if m.rng.next_float() < p.dropouts * depth * DROPOUT_RATE * chunk_s {
            let r = m.rng.next_float();
            let db = 6.0 + 30.0 * r * p.dropouts.max(0.3);
            m.dropout_target = fast_db_to_gain(-db);
            let seconds = 0.02 + 0.2 * m.rng.next_float();
            m.hold = (seconds / chunk_s) as usize + 1;
        }
        let c = if m.dropout_target < m.dropout { attack } else { release };
        m.dropout += c * (m.dropout_target - m.dropout);
        wear_gain * m.dropout
    }

    pub fn set(&mut self, p: &LossParams, n: usize) {
        let sr = self.sr;
        let depth = p.amount;
        let wear_share = (2.0 * (1.0 - p.balance)).min(1.0);
        let flutter_share = (2.0 * p.balance).min(1.0);
        let chunk_s = CHUNK as f32 / sr;
        let mut gains = [1.0f32; 2];
        for (k, m) in self.mods.iter_mut().enumerate() {
            if k == 1 && !p.stereo {
                gains[1] = gains[0];
                // Keep the second set running so switching to stereo is smooth.
                Self::step(m, p, depth, wear_share, chunk_s, self.attack, self.release);
                continue;
            }
            gains[k] = Self::step(m, p, depth, wear_share, chunk_s, self.attack, self.release);
        }
        let flutter_hz = 14.0 * (1.0 + 0.4 * p.flux * self.flutter_drift.next());
        self.flutter_inc = flutter_hz.max(2.0) / sr;
        self.flutter_depth = 1.0 - fast_db_to_gain(-FLUTTER_DB * depth * flutter_share);
        // Spacing loss: the gap sets the resting cutoff (20 kHz down to about
        // 1.2 kHz); level dips close it further.
        let resting = 20_000.0 * fast_exp2(-4.0 * p.gap);
        for (ch, g) in self.ch.iter_mut().zip(gains) {
            ch.gain.set(g, n);
            let closed = resting * (0.25 + 0.75 * g * g);
            ch.lp.set(closed.clamp(400.0, 0.45 * sr), 1.2, sr);
        }
        self.wet.set(if p.on { engage(p.amount, 0.02) } else { 0.0 }, n);
        self.activity = 1.0 - gains[0].min(gains[1]);
    }

    /// 0 .. 1 for the editor: momentary level loss.
    pub fn activity(&self) -> f32 {
        if self.wet.is_silent() {
            0.0
        } else {
            self.activity.clamp(0.0, 1.0)
        }
    }

    pub fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        let n = left.len().min(right.len());
        if self.wet.is_silent() {
            for ch in &mut self.ch {
                for _ in 0..n {
                    ch.gain.next();
                }
            }
            return;
        }
        for i in 0..n {
            let wet = self.wet.next();
            let ph = self.flutter_phase;
            self.flutter_phase = (ph + self.flutter_inc).fract();
            let flutter = 1.0 - self.flutter_depth * (0.5 + 0.5 * fast_sin01(ph));
            for (ch, x) in self.ch.iter_mut().zip([&mut left[i], &mut right[i]]) {
                let g = ch.gain.next() * flutter;
                let y = ch.lp.lp(*x) * g;
                *x += (y - *x) * wet;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn params(amount: f32) -> LossParams {
        LossParams { on: true, amount, balance: 0.3, dropouts: 0.5, gap: 0.0, stereo: false, flux: 0.2 }
    }

    /// Level of a constant input in 10 ms windows.
    fn levels(p: LossParams, seconds: f32) -> (Vec<f32>, Vec<f32>) {
        let mut loss = Loss::default();
        let n = (48_000.0 * seconds) as usize;
        let mut l = vec![0.5f32; n];
        let mut r = vec![0.5f32; n];
        for (cl, cr) in l.chunks_mut(CHUNK).zip(r.chunks_mut(CHUNK)) {
            loss.set(&p, cl.len());
            loss.process(cl, cr);
        }
        let win = |v: &[f32]| v.chunks(480).map(|w| w.iter().sum::<f32>() / w.len() as f32).collect::<Vec<_>>();
        (win(&l), win(&r))
    }

    #[test]
    fn zero_amount_is_a_bypass() {
        let (l, _) = levels(params(0.0), 0.5);
        assert!(l.iter().all(|v| (v - 0.5).abs() < 1.0e-6));
    }

    #[test]
    fn full_amount_makes_the_level_wander_and_drop_out() {
        let (l, _) = levels(params(1.0), 8.0);
        let lo = l[20..].iter().cloned().fold(f32::MAX, f32::min);
        let hi = l[20..].iter().cloned().fold(0.0, f32::max);
        assert!(hi <= 0.5001, "{hi}");
        assert!(lo < 0.2, "deepest dip {lo}");
        assert!(hi - lo > 0.2, "{lo} {hi}");
    }

    #[test]
    fn stereo_decorrelates_the_channels_and_mono_does_not() {
        let (l, r) = levels(params(1.0), 3.0);
        assert!(l.iter().zip(&r).all(|(a, b)| (a - b).abs() < 1.0e-6));
        let mut p = params(1.0);
        p.stereo = true;
        let (l, r) = levels(p, 3.0);
        let diff = l.iter().zip(&r).map(|(a, b)| (a - b).abs()).fold(0.0, f32::max);
        assert!(diff > 0.02, "{diff}");
    }
}
