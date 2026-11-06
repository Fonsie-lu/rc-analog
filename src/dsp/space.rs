//! SPACE: reverb. An 8-line feedback delay network with a Hadamard mixing
//! matrix, input diffusion, per-line damping and slowly modulated line
//! lengths. The four types set line lengths and diffusion; Spring adds a
//! dispersive allpass chain for the chirp of a spring tank. The focus range
//! sets what the reverb hears and how it damps (low edge: highpass on the
//! input and in the loop, high edge: lowpass damping). Stereo spreads the
//! tank across the field; Mono folds it to the centre. Flux moves the pre
//! delay and deepens the line modulation. The big knob is dry/wet.

use super::common::{fast_exp2, fast_sin01, one_pole_coeff, DelayLine, Drift, Ramp, CHUNK};
use super::filters::{OnePole, Svf, BUTTERWORTH_K};
use super::params::SpaceType;

#[derive(Clone, Copy, Debug)]
pub struct SpaceParams {
    pub on: bool,
    pub amount: f32,
    pub kind: SpaceType,
    /// RT60, seconds.
    pub decay: f32,
    pub pre_delay_ms: f32,
    pub focus_lo: f32,
    pub focus_hi: f32,
    pub stereo: bool,
    pub flux: f32,
}

const LINES: usize = 8;
const DIFFUSERS: usize = 4;
const SPRING_STAGES: usize = 12;
const MAX_PRE_DELAY_MS: f32 = 340.0;

struct TypeShape {
    lines_ms: [f32; LINES],
    diffusers_ms: [f32; DIFFUSERS],
    diffusion: f32,
    /// Line modulation depth at Flux 0, ms.
    mod_ms: f32,
}

fn shape(kind: SpaceType) -> TypeShape {
    match kind {
        SpaceType::Room => TypeShape {
            lines_ms: [11.3, 13.7, 17.1, 19.9, 23.3, 27.1, 29.9, 33.7],
            diffusers_ms: [1.3, 2.1, 3.4, 4.6],
            diffusion: 0.6,
            mod_ms: 0.08,
        },
        SpaceType::Hall => TypeShape {
            lines_ms: [37.1, 43.3, 51.7, 59.3, 67.1, 73.9, 83.3, 97.1],
            diffusers_ms: [3.1, 4.7, 7.9, 11.3],
            diffusion: 0.62,
            mod_ms: 0.25,
        },
        SpaceType::Plate => TypeShape {
            lines_ms: [17.3, 22.9, 28.7, 33.1, 39.7, 45.1, 51.3, 57.7],
            diffusers_ms: [0.9, 1.7, 2.9, 4.1],
            diffusion: 0.72,
            mod_ms: 0.12,
        },
        SpaceType::Spring => TypeShape {
            lines_ms: [24.1, 26.3, 29.7, 31.9, 35.3, 37.1, 41.9, 43.7],
            diffusers_ms: [0.7, 1.1, 1.9, 2.3],
            diffusion: 0.5,
            mod_ms: 0.05,
        },
    }
}

const MAX_LINE_MS: f32 = 100.0;
const MAX_DIFFUSER_MS: f32 = 13.0;

/// Schroeder allpass on a delay line.
#[derive(Clone, Debug, Default)]
struct Allpass {
    line: DelayLine,
    delay: usize,
}

impl Allpass {
    #[inline(always)]
    fn process(&mut self, x: f32, g: f32) -> f32 {
        let d = self.line.tap(self.delay.saturating_sub(1));
        let v = x + g * d;
        self.line.push(v);
        d - g * v
    }
}

/// First-order allpass for the spring dispersion.
#[derive(Clone, Copy, Debug, Default)]
struct Ap1 {
    x1: f32,
    y1: f32,
}

impl Ap1 {
    #[inline(always)]
    fn process(&mut self, x: f32, a: f32) -> f32 {
        let y = -a * x + self.x1 + a * self.y1;
        self.x1 = x;
        self.y1 = y;
        y
    }
}

#[derive(Clone, Debug, Default)]
struct Input {
    pre: DelayLine,
    hp: Svf,
    lp: Svf,
    diffusers: [Allpass; DIFFUSERS],
    spring: [Ap1; SPRING_STAGES],
}

pub struct Space {
    sr: f32,
    kind: SpaceType,
    inputs: [Input; 2],
    lines: [DelayLine; LINES],
    line_len: [f32; LINES],
    gains: [f32; LINES],
    damp: [OnePole; LINES],
    loop_hp: [OnePole; LINES],
    lfo_phase: [f32; LINES],
    lfo_inc: [f32; LINES],
    mod_depth: f32,
    diffusion: f32,
    spring_tone: [Svf; 2],
    pre_delay: Ramp,
    pre_drift: Drift,
    mix: Ramp,
    stereo: bool,
    activity: f32,
}

impl Default for Space {
    fn default() -> Self {
        let mut s = Self {
            sr: 48_000.0,
            kind: SpaceType::Room,
            inputs: [Input::default(), Input::default()],
            lines: Default::default(),
            line_len: [1.0; LINES],
            gains: [0.0; LINES],
            damp: [OnePole::default(); LINES],
            loop_hp: [OnePole::default(); LINES],
            lfo_phase: [0.0; LINES],
            lfo_inc: [0.0; LINES],
            mod_depth: 0.0,
            diffusion: 0.6,
            spring_tone: [Svf::default(); 2],
            pre_delay: Ramp::new(0.0),
            pre_drift: Drift::new(51),
            mix: Ramp::new(0.0),
            stereo: true,
            activity: 0.0,
        };
        s.prepare(48_000.0);
        s
    }
}

/// In-place 8-point Hadamard transform scaled to be orthonormal.
#[inline(always)]
fn hadamard8(v: &mut [f32; LINES]) {
    let mut h = 1;
    while h < LINES {
        let mut i = 0;
        while i < LINES {
            for j in i..i + h {
                let (a, b) = (v[j], v[j + h]);
                v[j] = a + b;
                v[j + h] = a - b;
            }
            i += 2 * h;
        }
        h *= 2;
    }
    let norm = 1.0 / (LINES as f32).sqrt();
    for x in v.iter_mut() {
        *x *= norm;
    }
}

impl Space {
    pub fn prepare(&mut self, sample_rate: f32) {
        self.sr = sample_rate;
        let ms = sample_rate / 1000.0;
        for input in &mut self.inputs {
            input.pre.prepare((MAX_PRE_DELAY_MS * ms) as usize + 8);
            for d in &mut input.diffusers {
                d.line.prepare((MAX_DIFFUSER_MS * 1.1 * ms) as usize + 4);
            }
        }
        for line in &mut self.lines {
            line.prepare((MAX_LINE_MS * ms) as usize + 64);
        }
        for (i, inc) in self.lfo_inc.iter_mut().enumerate() {
            *inc = (0.31 + 0.09 * i as f32) / sample_rate;
        }
        self.pre_drift.set_rate(0.6, sample_rate / CHUNK as f32);
        self.configure(self.kind);
        self.reset();
    }

    pub fn reset(&mut self) {
        for input in &mut self.inputs {
            input.pre.reset();
            input.hp.reset();
            input.lp.reset();
            for d in &mut input.diffusers {
                d.line.reset();
            }
            input.spring = [Ap1::default(); SPRING_STAGES];
        }
        for line in &mut self.lines {
            line.reset();
        }
        for f in self.damp.iter_mut().chain(self.loop_hp.iter_mut()) {
            f.reset();
        }
        for (i, p) in self.lfo_phase.iter_mut().enumerate() {
            *p = i as f32 * 0.137;
        }
        for t in &mut self.spring_tone {
            t.reset();
        }
        self.pre_drift.reset();
        self.activity = 0.0;
    }

    fn configure(&mut self, kind: SpaceType) {
        self.kind = kind;
        let s = shape(kind);
        let ms = self.sr / 1000.0;
        for (len, l) in self.line_len.iter_mut().zip(s.lines_ms) {
            *len = l * ms;
        }
        for (k, input) in self.inputs.iter_mut().enumerate() {
            // The right channel's diffusers are a little longer: decorrelation.
            let stretch = if k == 0 { 1.0 } else { 1.071 };
            for (d, l) in input.diffusers.iter_mut().zip(s.diffusers_ms) {
                d.delay = ((l * stretch * ms) as usize).max(1);
            }
        }
        self.diffusion = s.diffusion;
        self.mod_depth = s.mod_ms * ms;
        for t in &mut self.spring_tone {
            t.set(1_400.0, 1.2, self.sr);
        }
    }

    pub fn set(&mut self, p: &SpaceParams, n: usize) {
        if p.kind != self.kind {
            self.configure(p.kind);
        }
        let sr = self.sr;
        let ms = sr / 1000.0;
        let wander = p.flux * 15.0 * self.pre_drift.next();
        let pre = (p.pre_delay_ms + wander).clamp(0.0, MAX_PRE_DELAY_MS - 5.0) * ms;
        self.pre_delay.set(pre, n);
        let rt60 = p.decay.max(0.05);
        for (g, len) in self.gains.iter_mut().zip(&self.line_len) {
            // 60 dB of decay over rt60: g = 10^(-3 len / (rt60 sr)).
            *g = fast_exp2(-9.965_784 * len / (rt60 * sr));
        }
        let damp_hz = p.focus_hi.clamp(500.0, 0.45 * sr);
        let damp = one_pole_coeff(damp_hz, sr);
        let low = one_pole_coeff((p.focus_lo * 0.5).max(10.0), sr);
        for (d, h) in self.damp.iter_mut().zip(self.loop_hp.iter_mut()) {
            d.set_coeff(damp);
            h.set_coeff(low);
        }
        for input in &mut self.inputs {
            input.hp.set(p.focus_lo, BUTTERWORTH_K, sr);
            input.lp.set(p.focus_hi, BUTTERWORTH_K, sr);
        }
        let mod_scale = 1.0 + 8.0 * p.flux;
        self.mod_depth = shape(self.kind).mod_ms * ms * mod_scale;
        self.mix.set(if p.on { p.amount } else { 0.0 }, n);
        self.stereo = p.stereo;
    }

    /// 0 .. 1 for the editor: the wet level on a 60 dB scale.
    pub fn activity(&self) -> f32 {
        let db = 20.0 * self.activity.max(1.0e-6).log10();
        ((db + 60.0) / 60.0).clamp(0.0, 1.0)
    }

    pub fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        let n = left.len().min(right.len());
        if self.mix.is_silent() {
            self.activity *= 0.9;
            return;
        }
        let spring = self.kind == SpaceType::Spring;
        let g_diff = self.diffusion;
        let mut peak: f32 = 0.0;
        for i in 0..n {
            let mix = self.mix.next();
            let pre = self.pre_delay.next();
            // Equal-power dry/wet.
            let wet_gain = fast_sin01(mix * 0.25);
            let dry_gain = fast_sin01(mix * 0.25 + 0.25);

            let mut fed = [0.0f32; 2];
            for (k, (input, x)) in self.inputs.iter_mut().zip([left[i], right[i]]).enumerate() {
                input.pre.push(x);
                let mut v = input.pre.read_linear(pre);
                v = input.lp.lp(input.hp.hp(v));
                if spring {
                    for ap in &mut input.spring {
                        v = ap.process(v, 0.62);
                    }
                }
                for d in &mut input.diffusers {
                    v = d.process(v, g_diff);
                }
                fed[k] = v;
            }

            // Tank.
            let mut y = [0.0f32; LINES];
            for (j, yj) in y.iter_mut().enumerate() {
                let ph = self.lfo_phase[j];
                self.lfo_phase[j] = (ph + self.lfo_inc[j]).fract();
                let d = self.line_len[j] + self.mod_depth * (1.0 + fast_sin01(ph));
                let raw = self.lines[j].read_linear(d);
                let damped = self.damp[j].lp(raw);
                *yj = damped - self.loop_hp[j].lp(damped);
            }
            let mut fb = y;
            hadamard8(&mut fb);
            for (j, ((line, f), g)) in self.lines.iter_mut().zip(fb).zip(self.gains).enumerate() {
                let inject = if j % 2 == 0 { fed[0] } else { fed[1] };
                let sign = if j & 2 == 0 { 1.0 } else { -1.0 };
                line.push(f * g + inject * sign * 0.5);
            }

            let mut wl = 0.5 * (y[0] - y[2] + y[4] - y[6] + 0.7 * (y[1] - y[5]));
            let mut wr = 0.5 * (y[1] - y[3] + y[5] - y[7] + 0.7 * (y[2] - y[6]));
            if spring {
                wl = self.spring_tone[0].process(wl).bp * 2.0 + wl * 0.3;
                wr = self.spring_tone[1].process(wr).bp * 2.0 + wr * 0.3;
            }
            if !self.stereo {
                let m = 0.5 * (wl + wr);
                wl = m;
                wr = m;
            }
            peak = peak.max(wl.abs()).max(wr.abs());
            left[i] = left[i] * dry_gain + wl * wet_gain;
            right[i] = right[i] * dry_gain + wr * wet_gain;
        }
        self.activity = self.activity * 0.8 + peak * 0.2;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn params(kind: SpaceType, decay: f32) -> SpaceParams {
        SpaceParams { on: true, amount: 1.0, kind, decay, pre_delay_ms: 0.0, focus_lo: 20.0, focus_hi: 20_000.0, stereo: true, flux: 0.0 }
    }

    /// Wet impulse response energy in consecutive 100 ms windows.
    fn energy_windows(p: SpaceParams, seconds: f32) -> Vec<f32> {
        let mut s = Space::default();
        let n = (48_000.0 * seconds) as usize;
        let mut l = vec![0.0f32; n];
        let mut r = vec![0.0f32; n];
        l[0] = 1.0;
        r[0] = 1.0;
        for (cl, cr) in l.chunks_mut(CHUNK).zip(r.chunks_mut(CHUNK)) {
            s.set(&p, cl.len());
            s.process(cl, cr);
        }
        l.chunks(4_800).map(|w| w.iter().map(|v| v * v).sum::<f32>()).collect()
    }

    #[test]
    fn decay_time_is_roughly_honoured() {
        for kind in SpaceType::ALL {
            let e = energy_windows(params(kind, 1.0), 2.0);
            assert!(e.iter().all(|v| v.is_finite()));
            // 1 s RT60: 60 dB per second, so 100 ms windows lose ~6 dB each.
            // Compare 0.3-0.4 s against 0.8-0.9 s: about 30 dB apart.
            let drop = 10.0 * (e[3] / e[8]).log10();
            assert!((18.0..45.0).contains(&drop), "{kind:?} dropped {drop:.1} dB");
        }
    }

    #[test]
    fn longer_decay_rings_longer() {
        let short = energy_windows(params(SpaceType::Hall, 0.5), 2.0);
        let long = energy_windows(params(SpaceType::Hall, 4.0), 2.0);
        assert!(long[15] > short[15] * 1.0e3);
    }

    #[test]
    fn zero_amount_is_a_bypass() {
        let mut s = Space::default();
        let mut p = params(SpaceType::Plate, 2.0);
        p.amount = 0.0;
        let mut l: Vec<f32> = (0..2_000).map(|i| (i as f32 * 0.02).sin()).collect();
        let orig = l.clone();
        let mut r = l.clone();
        for (cl, cr) in l.chunks_mut(CHUNK).zip(r.chunks_mut(CHUNK)) {
            s.set(&p, cl.len());
            s.process(cl, cr);
        }
        assert_eq!(l, orig);
    }

    #[test]
    fn mono_folds_the_tail_to_the_centre() {
        let mut s = Space::default();
        let mut p = params(SpaceType::Hall, 2.0);
        p.stereo = false;
        let n = 20_000;
        let mut l = vec![0.0f32; n];
        let mut r = vec![0.0f32; n];
        l[0] = 1.0;
        for (cl, cr) in l.chunks_mut(CHUNK).zip(r.chunks_mut(CHUNK)) {
            s.set(&p, cl.len());
            s.process(cl, cr);
        }
        for i in 1..n {
            assert!((l[i] - r[i]).abs() < 1.0e-6);
        }
    }
}
