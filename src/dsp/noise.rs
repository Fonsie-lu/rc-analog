//! NOISE: synthesised background noises - record crackle, tape and cassette
//! hiss, VHS, mains hum, radio static, white and pink noise. Nothing is
//! sampled; every type is built from seeded generators, so renders repeat
//! exactly.
//!
//! The module only adds noise, it never changes the incoming audio. The
//! engine places the noise before the effects (Pre, so it gets wobbled and
//! crushed along with the source) or after the master EQ (Post). Follow makes
//! the level track the input, Duck pushes it down under input peaks, Tone is a
//! tilt, Flux makes level and crackle density wander. When the host transport
//! stops and the input falls silent the noise fades out.

use super::common::{engage, fast_db_to_gain, fast_sin01, one_pole_coeff, Drift, Ramp, Rng, CHUNK};
use super::filters::{OnePole, Svf, Tone, BUTTERWORTH_K};
use super::params::NoiseType;

#[derive(Clone, Copy, Debug)]
pub struct NoiseParams {
    pub on: bool,
    /// Big knob after Magnitude, 0 .. 1.
    pub amount: f32,
    pub kind: NoiseType,
    pub tone: f32,
    pub follow: f32,
    pub duck: f32,
    pub flux: f32,
}

/// Paul Kellet's refined pink noise filter.
#[derive(Clone, Copy, Debug, Default)]
struct Pink {
    b: [f32; 7],
}

impl Pink {
    #[inline(always)]
    fn next(&mut self, white: f32) -> f32 {
        let b = &mut self.b;
        b[0] = 0.99886 * b[0] + white * 0.055_517_9;
        b[1] = 0.99332 * b[1] + white * 0.075_075_9;
        b[2] = 0.96900 * b[2] + white * 0.153_852;
        b[3] = 0.86650 * b[3] + white * 0.310_485_6;
        b[4] = 0.55000 * b[4] + white * 0.532_952_2;
        b[5] = -0.7616 * b[5] - white * 0.016_898_0;
        let pink = b[0] + b[1] + b[2] + b[3] + b[4] + b[5] + b[6] + white * 0.5362;
        b[6] = white * 0.115_926;
        pink * 0.11
    }
}

/// Random clicks and pops: exponentially decaying impulses at random times,
/// highpassed so they tick rather than thump.
#[derive(Clone, Copy, Debug, Default)]
struct Crackle {
    click: f32,
    click_decay: f32,
    pop: f32,
    pop_decay: f32,
    hp: OnePole,
    pop_hp: OnePole,
}

impl Crackle {
    fn reset(&mut self) {
        self.click = 0.0;
        self.pop = 0.0;
        self.hp.reset();
        self.pop_hp.reset();
    }

    /// `clicks` and `pops` are events per sample.
    #[inline(always)]
    fn next(&mut self, rng: &mut Rng, clicks: f32, pops: f32, sr: f32) -> f32 {
        if rng.next_float() < clicks {
            // Heavy-tailed amplitudes: mostly faint ticks, the odd loud one.
            let a = rng.next_float();
            let amp = a * a * a * 0.9 + 0.03;
            self.click += if rng.next_u32() & 1 == 0 { amp } else { -amp };
            self.click_decay = 1.0 - 1.0 / (sr * (0.00006 + 0.00025 * rng.next_float()));
        }
        if rng.next_float() < pops {
            let amp = 0.4 + 0.5 * rng.next_float();
            self.pop += if rng.next_u32() & 1 == 0 { amp } else { -amp };
            self.pop_decay = 1.0 - 1.0 / (sr * (0.0008 + 0.0015 * rng.next_float()));
        }
        let out = self.hp.hp(self.click) + 0.7 * self.pop_hp.hp(self.pop);
        self.click *= self.click_decay;
        self.pop *= self.pop_decay;
        out
    }
}

/// Per-channel generator state.
#[derive(Clone, Copy, Debug, Default)]
struct Channel {
    rng: Rng,
    pink: Pink,
    crackle: Crackle,
    /// Type-specific band limiting: highpass then lowpass.
    hp: Svf,
    lp: Svf,
    rumble: OnePole,
    tone: Tone,
}

pub struct Noise {
    sr: f32,
    kind: NoiseType,
    /// Output trim per type so all types sit at a similar perceived level.
    trim: f32,
    ch: [Channel; 2],
    hum_phase: f32,
    whine_phase: f32,
    /// Static bursts of the radio type, and level / density wander (Flux).
    radio: Drift,
    flux_level: Drift,
    flux_density: Drift,
    level: Ramp,
    follow: f32,
    duck: f32,
    flux: f32,
    follow_env: f32,
    duck_env: f32,
    follow_coeffs: (f32, f32),
    duck_coeffs: (f32, f32),
    /// Fade-out while the transport is stopped and the input silent.
    idle_gain: f32,
    idle_coeff: f32,
    silent_samples: usize,
    activity: f32,
}

impl Default for Noise {
    fn default() -> Self {
        let mut n = Self {
            sr: 48_000.0,
            kind: NoiseType::Vinyl,
            trim: 1.0,
            ch: [Channel::default(); 2],
            hum_phase: 0.0,
            whine_phase: 0.0,
            radio: Drift::new(91),
            flux_level: Drift::new(92),
            flux_density: Drift::new(93),
            level: Ramp::new(0.0),
            follow: 0.0,
            duck: 0.0,
            flux: 0.0,
            follow_env: 0.0,
            duck_env: 0.0,
            follow_coeffs: (0.0, 0.0),
            duck_coeffs: (0.0, 0.0),
            idle_gain: 1.0,
            idle_coeff: 0.0,
            silent_samples: 0,
            activity: 0.0,
        };
        n.prepare(48_000.0);
        n
    }
}

/// Seconds of silence (with the transport stopped) before the noise fades.
const IDLE_AFTER: f32 = 0.4;

impl Noise {
    pub fn prepare(&mut self, sample_rate: f32) {
        self.sr = sample_rate;
        let chunk_rate = sample_rate / CHUNK as f32;
        self.radio.set_rate(3.0, sample_rate);
        self.flux_level.set_rate(0.7, chunk_rate);
        self.flux_density.set_rate(0.4, chunk_rate);
        self.follow_coeffs = (one_pole_coeff(1.0 / (2.0 * std::f32::consts::PI * 0.002), sample_rate), one_pole_coeff(1.3, sample_rate));
        self.duck_coeffs = (one_pole_coeff(6.0, sample_rate), one_pole_coeff(0.5, sample_rate));
        self.idle_coeff = one_pole_coeff(0.6, sample_rate);
        for ch in &mut self.ch {
            ch.crackle.hp.set(900.0, sample_rate);
            ch.crackle.pop_hp.set(150.0, sample_rate);
            ch.rumble.set(25.0, sample_rate);
        }
        self.configure(self.kind);
        self.reset();
    }

    pub fn reset(&mut self) {
        for (i, ch) in self.ch.iter_mut().enumerate() {
            ch.rng.seed(0x5eed_0001 + i as u32 * 7919);
            ch.pink = Pink::default();
            ch.crackle.reset();
            ch.hp.reset();
            ch.lp.reset();
            ch.rumble.reset();
            ch.tone.reset();
        }
        self.hum_phase = 0.0;
        self.whine_phase = 0.0;
        self.radio.reset();
        self.flux_level.reset();
        self.flux_density.reset();
        self.follow_env = 0.0;
        self.duck_env = 0.0;
        self.idle_gain = 1.0;
        self.silent_samples = 0;
        self.activity = 0.0;
    }

    /// Band limits per type: (highpass Hz, lowpass Hz).
    fn configure(&mut self, kind: NoiseType) {
        self.kind = kind;
        let (hp, lp) = match kind {
            NoiseType::Vinyl => (30.0, 7_000.0),
            NoiseType::Dust => (200.0, 12_000.0),
            NoiseType::TapeHiss => (1_200.0, 15_000.0),
            NoiseType::Cassette => (600.0, 9_000.0),
            NoiseType::Vhs => (1_500.0, 11_000.0),
            NoiseType::Hum50 | NoiseType::Hum60 => (30.0, 4_000.0),
            NoiseType::Radio => (400.0, 3_200.0),
            NoiseType::White | NoiseType::Pink => (20.0, 20_000.0),
        };
        // Hiss and hum around -16 dBFS RMS at full volume; crackle types
        // lower in RMS, since their clicks are heard by their peaks.
        self.trim = match kind {
            NoiseType::Vinyl => 1.0,
            NoiseType::Dust => 1.6,
            NoiseType::TapeHiss => 0.4,
            NoiseType::Cassette => 0.5,
            NoiseType::Vhs => 0.55,
            NoiseType::Hum50 | NoiseType::Hum60 => 0.4,
            NoiseType::Radio => 1.8,
            NoiseType::White => 0.6,
            NoiseType::Pink => 0.68,
        };
        let sr = self.sr;
        for ch in &mut self.ch {
            ch.hp.set(hp, BUTTERWORTH_K, sr);
            ch.lp.set(lp, BUTTERWORTH_K, sr);
        }
    }

    /// Once per chunk of `n` samples.
    pub fn set(&mut self, p: &NoiseParams, n: usize) {
        if p.kind != self.kind {
            self.configure(p.kind);
        }
        let flux_db = p.flux * 9.0 * self.flux_level.next();
        let target = if p.on {
            let a = p.amount;
            engage(a, 0.02) * a * a * fast_db_to_gain(flux_db)
        } else {
            0.0
        };
        self.level.set(target, n);
        self.follow = p.follow;
        self.duck = p.duck;
        self.flux = p.flux;
        let sr = self.sr;
        for ch in &mut self.ch {
            ch.tone.set(p.tone, false, sr);
        }
    }

    pub fn is_silent(&self) -> bool {
        self.level.is_silent()
    }

    /// 0 .. 1 for the editor: the noise level, on a 60 dB scale.
    pub fn activity(&self) -> f32 {
        let db = 20.0 * self.activity.max(1.0e-6).log10();
        ((db + 60.0) / 60.0).clamp(0.0, 1.0)
    }

    /// Writes the noise for the input chunk `in_l`/`in_r` into `out_l`/`out_r`
    /// (overwriting them). `playing` is the host transport state.
    pub fn render(&mut self, in_l: &[f32], in_r: &[f32], out_l: &mut [f32], out_r: &mut [f32], playing: bool) {
        let n = in_l.len().min(in_r.len()).min(out_l.len()).min(out_r.len());
        if self.level.is_silent() {
            out_l[..n].fill(0.0);
            out_r[..n].fill(0.0);
            self.activity *= 0.9;
            return;
        }
        let sr = self.sr;
        let density = (1.0 + self.flux * 0.9 * self.flux_density.value()).max(0.05);
        let (clicks, pops) = match self.kind {
            NoiseType::Vinyl => (14.0, 0.6),
            NoiseType::Dust => (45.0, 1.5),
            NoiseType::Radio => (6.0, 0.3),
            _ => (0.0, 0.0),
        };
        let clicks = clicks * density / sr;
        let pops = pops * density / sr;
        let hum_hz = match self.kind {
            NoiseType::Hum60 => 60.0,
            _ => 50.0,
        };
        let hum_inc = hum_hz / sr;
        let whine_hz = 15_734.0;
        let whine_inc = if whine_hz < sr * 0.45 { whine_hz / sr } else { 0.0 };
        let (fa, fr) = self.follow_coeffs;
        let (da, dr) = self.duck_coeffs;
        let idle_limit = (IDLE_AFTER * sr) as usize;
        let mut peak: f32 = 0.0;

        for i in 0..n {
            let input = in_l[i].abs().max(in_r[i].abs());
            let c = if input > self.follow_env { fa } else { fr };
            self.follow_env += c * (input - self.follow_env);
            let c = if input > self.duck_env { da } else { dr };
            self.duck_env += c * (input - self.duck_env);

            if input > 1.0e-4 {
                self.silent_samples = 0;
            } else {
                self.silent_samples = self.silent_samples.saturating_add(1);
            }
            let idle_target = if playing || self.silent_samples < idle_limit { 1.0 } else { 0.0 };
            self.idle_gain += self.idle_coeff * (idle_target - self.idle_gain);

            let follow_gain = 1.0 - self.follow + self.follow * (self.follow_env * 4.0).min(1.0);
            let duck_gain = 1.0 / (1.0 + self.duck * 12.0 * self.duck_env);
            let gain = self.level.next() * follow_gain * duck_gain * self.idle_gain * self.trim;

            // Mono parts shared by both channels.
            let hum = match self.kind {
                NoiseType::Hum50 | NoiseType::Hum60 => {
                    let ph = self.hum_phase;
                    self.hum_phase = (ph + hum_inc).fract();
                    0.30 * fast_sin01(ph)
                        + 0.22 * fast_sin01(2.0 * ph)
                        + 0.16 * fast_sin01(3.0 * ph)
                        + 0.07 * fast_sin01(4.0 * ph)
                        + 0.08 * fast_sin01(5.0 * ph)
                        + 0.04 * fast_sin01(7.0 * ph)
                        + 0.02 * fast_sin01(9.0 * ph)
                }
                NoiseType::Vhs => {
                    let ph = self.whine_phase;
                    self.whine_phase = (ph + whine_inc).fract();
                    let ph2 = self.hum_phase;
                    self.hum_phase = (ph2 + 59.94 / sr).fract();
                    0.02 * fast_sin01(ph) + 0.03 * fast_sin01(ph2) + 0.015 * fast_sin01(3.0 * ph2)
                }
                _ => 0.0,
            };
            let radio_gate = if self.kind == NoiseType::Radio {
                let d = self.radio.next();
                0.25 + 0.75 * (d * d).min(1.0)
            } else {
                1.0
            };

            let mut out = [0.0f32; 2];
            for (k, ch) in self.ch.iter_mut().enumerate() {
                let white = ch.rng.next_bipolar();
                let raw = match self.kind {
                    NoiseType::Vinyl => {
                        let crackle = ch.crackle.next(&mut ch.rng, clicks, pops, sr);
                        let surface = ch.pink.next(white) * 0.5;
                        let rumble = ch.rumble.lp(white) * 3.0;
                        ch.lp.lp(ch.hp.hp(surface + rumble)) + crackle
                    }
                    NoiseType::Dust => ch.lp.lp(ch.crackle.next(&mut ch.rng, clicks, pops, sr)),
                    NoiseType::TapeHiss => ch.lp.lp(ch.hp.hp(white * 0.8 + ch.pink.next(white) * 0.4)),
                    NoiseType::Cassette => ch.lp.lp(ch.hp.hp(white * 0.7 + ch.pink.next(white) * 0.8)),
                    NoiseType::Vhs => ch.lp.lp(ch.hp.hp(white * 0.8)) + hum,
                    NoiseType::Hum50 | NoiseType::Hum60 => ch.lp.lp(hum + white * 0.004),
                    NoiseType::Radio => {
                        let crackle = ch.crackle.next(&mut ch.rng, clicks, pops, sr);
                        ch.lp.lp(ch.hp.hp(ch.pink.next(white) * 1.6 * radio_gate)) + crackle * 0.5
                    }
                    NoiseType::White => white * 0.45,
                    NoiseType::Pink => ch.pink.next(white) * 1.2,
                };
                out[k] = ch.tone.process(raw) * gain;
            }
            out_l[i] = out[0];
            out_r[i] = out[1];
            peak = peak.max(out[0].abs()).max(out[1].abs());
        }
        self.activity = self.activity * 0.8 + peak * 0.2;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn params(kind: NoiseType) -> NoiseParams {
        NoiseParams { on: true, amount: 1.0, kind, tone: 0.0, follow: 0.0, duck: 0.0, flux: 0.0 }
    }

    fn rms_of(noise: &mut Noise, p: NoiseParams, input: f32, playing: bool, chunks: usize) -> (f32, f32) {
        let inp = [input; CHUNK];
        let (mut l, mut r) = ([0.0; CHUNK], [0.0; CHUNK]);
        let mut sq = 0.0;
        let mut peak: f32 = 0.0;
        for _ in 0..chunks {
            noise.set(&p, CHUNK);
            noise.render(&inp, &inp, &mut l, &mut r, playing);
            for v in l.iter().chain(&r) {
                sq += v * v;
                peak = peak.max(v.abs());
            }
        }
        ((sq / (2 * CHUNK * chunks) as f32).sqrt(), peak)
    }

    #[test]
    fn every_type_makes_a_sensible_level() {
        let mut levels = Vec::new();
        for kind in NoiseType::ALL {
            let mut n = Noise::default();
            let (rms, peak) = rms_of(&mut n, params(kind), 0.0, true, 3_000);
            let db = 20.0 * rms.log10();
            assert!(peak < 1.5, "{kind:?} peak {peak}");
            assert!((-45.0..-8.0).contains(&db), "{kind:?} at {db:.1} dB");
            levels.push((kind, db));
        }
        eprintln!("{levels:?}");
    }

    #[test]
    fn amount_zero_and_off_are_silent() {
        let mut n = Noise::default();
        let mut p = params(NoiseType::White);
        p.amount = 0.0;
        assert_eq!(rms_of(&mut n, p, 0.0, true, 50).1, 0.0);
        p.amount = 1.0;
        p.on = false;
        assert_eq!(rms_of(&mut n, p, 0.0, true, 50).1, 0.0);
    }

    #[test]
    fn noise_fades_when_stopped_and_silent_but_not_while_input_plays() {
        let mut n = Noise::default();
        let p = params(NoiseType::White);
        let (playing_rms, _) = rms_of(&mut n, p, 0.0, true, 100);
        // Let the idle timer run out and the fade finish, then measure.
        rms_of(&mut n, p, 0.0, false, 3_000);
        let (stopped_rms, _) = rms_of(&mut n, p, 0.0, false, 500);
        assert!(stopped_rms < playing_rms * 0.05, "{stopped_rms} vs {playing_rms}");
        let (fed_rms, _) = rms_of(&mut n, p, 0.3, false, 2_000);
        assert!(fed_rms > playing_rms * 0.8, "{fed_rms}");
    }

    #[test]
    fn follow_tracks_the_input_and_duck_ducks() {
        let mut p = params(NoiseType::Pink);
        p.follow = 1.0;
        let quiet = rms_of(&mut Noise::default(), p, 0.0, true, 400).0;
        let loud = rms_of(&mut Noise::default(), p, 0.5, true, 400).0;
        assert!(quiet < loud * 0.05, "{quiet} {loud}");

        let mut p = params(NoiseType::Pink);
        p.duck = 1.0;
        let quiet = rms_of(&mut Noise::default(), p, 0.0, true, 400).0;
        let loud = rms_of(&mut Noise::default(), p, 0.5, true, 400).0;
        assert!(loud < quiet * 0.3, "{quiet} {loud}");
    }

    #[test]
    fn renders_repeat_exactly() {
        let a = rms_of(&mut Noise::default(), params(NoiseType::Vinyl), 0.0, true, 200);
        let b = rms_of(&mut Noise::default(), params(NoiseType::Vinyl), 0.0, true, 200);
        assert_eq!(a, b);
    }
}
