//! The effect chain:
//!
//! ```text
//! in gain -> [noise pre] -> wobble -> distort -> digital -> space -> loss
//!         -> EQ (cuts, tone) -> [noise post] -> width -> out gain
//! ```
//!
//! The host's parameter values are copied in once per block (`set_params`)
//! and smoothed once per chunk of [`CHUNK`] samples; each module then gets its
//! settings for the chunk and ramps its gains across it. Chunks are counted
//! continuously, independent of the host's block boundaries. Magnitude scales the
//! six big knobs, the input and output gain and the EQ, so it takes the whole
//! plugin from dry to fully processed. Nothing here allocates after
//! `prepare`.

use std::sync::atomic::Ordering::Relaxed;
use std::sync::Arc;

use super::common::{fast_db_to_gain, one_pole_coeff, DenormalGuard, Ramp, CHUNK};
use super::digital::{Digital, DigitalParams};
use super::distort::{Distort, DistortParams};
use super::filters::{Cut, Tone};
use super::loss::{Loss, LossParams};
use super::noise::{Noise, NoiseParams};
use super::params::{defaults, DistortType, Module, NoiseType, ParamId, SpaceType, NUM_PARAMS};
use super::shared::{Feedback, Shared};
use super::space::{Space, SpaceParams};
use super::wobble::{Wobble, WobbleParams};

/// Latency the plugin reports: the distortion's oversampler, always in path.
pub const LATENCY: usize = super::distort::LATENCY;

/// Smoothing time of continuous parameters.
const SMOOTH_SECONDS: f32 = 0.03;

#[derive(Clone, Copy, Debug, Default)]
struct Eq {
    low: [Cut; 2],
    high: [Cut; 2],
    tone: [Tone; 2],
    wet: Ramp,
}

pub struct Engine {
    shared: Arc<Shared>,
    sr: f32,
    target: [f32; NUM_PARAMS],
    smoothed: [f32; NUM_PARAMS],
    smooth_coeff: f32,
    /// Snap the smoothed values to the targets on the next chunk.
    snap: bool,
    playing: bool,
    noise: Noise,
    wobble: Wobble,
    distort: Distort,
    digital: Digital,
    space: Space,
    loss: Loss,
    eq: Eq,
    in_gain: Ramp,
    out_gain: Ramp,
    width: Ramp,
    noise_post: bool,
    noise_l: [f32; CHUNK],
    noise_r: [f32; CHUNK],
    /// Samples left in the current chunk. Chunks run on across host blocks,
    /// so renders do not depend on the block size.
    chunk_left: usize,
}

impl Engine {
    pub fn new(shared: Arc<Shared>) -> Self {
        let values = defaults();
        let mut e = Self {
            shared,
            sr: 48_000.0,
            target: values,
            smoothed: values,
            smooth_coeff: 1.0,
            snap: true,
            playing: true,
            noise: Noise::default(),
            wobble: Wobble::default(),
            distort: Distort::default(),
            digital: Digital::default(),
            space: Space::default(),
            loss: Loss::default(),
            eq: Eq { low: [Cut::new(true); 2], high: [Cut::new(false); 2], ..Eq::default() },
            in_gain: Ramp::new(1.0),
            out_gain: Ramp::new(1.0),
            width: Ramp::new(1.0),
            noise_post: false,
            noise_l: [0.0; CHUNK],
            noise_r: [0.0; CHUNK],
            chunk_left: 0,
        };
        e.prepare(48_000.0);
        e
    }

    /// Allocates the delay lines for `sample_rate`. Not real-time safe.
    pub fn prepare(&mut self, sample_rate: f32) {
        self.sr = sample_rate;
        self.smooth_coeff = one_pole_coeff(1.0 / (std::f32::consts::TAU * SMOOTH_SECONDS), sample_rate / CHUNK as f32);
        self.noise.prepare(sample_rate);
        self.wobble.prepare(sample_rate);
        self.distort.prepare(sample_rate);
        self.digital.prepare(sample_rate);
        self.space.prepare(sample_rate);
        self.loss.prepare(sample_rate);
        self.reset();
    }

    /// Clears all audio state (tails, delay lines, generators).
    pub fn reset(&mut self) {
        self.noise.reset();
        self.wobble.reset();
        self.distort.reset();
        self.digital.reset();
        self.space.reset();
        self.loss.reset();
        for c in self.eq.low.iter_mut().chain(self.eq.high.iter_mut()) {
            c.reset();
        }
        for t in &mut self.eq.tone {
            t.reset();
        }
        self.snap = true;
        self.chunk_left = 0;
    }

    /// Natural-unit values for every parameter, in table order. Once per block.
    pub fn set_params(&mut self, values: &[f32; NUM_PARAMS]) {
        self.target = *values;
    }

    /// Host transport state; the noise fades out while stopped and silent.
    pub fn set_playing(&mut self, playing: bool) {
        self.playing = playing;
    }

    pub fn sample_rate(&self) -> f32 {
        self.sr
    }

    #[inline]
    fn v(&self, id: ParamId) -> f32 {
        self.smoothed[id.index()]
    }

    #[inline]
    fn on(&self, id: ParamId) -> bool {
        self.smoothed[id.index()] > 0.5
    }

    /// Smooths the parameters by one chunk and hands every module its
    /// settings for the next `n` samples.
    fn update(&mut self, n: usize) {
        let c = if self.snap { 1.0 } else { self.smooth_coeff };
        let ramp = if self.snap { 0 } else { n };
        self.snap = false;
        for id in ParamId::all() {
            let i = id.index();
            if id.desc().steps > 0 {
                self.smoothed[i] = self.target[i];
            } else {
                self.smoothed[i] += c * (self.target[i] - self.smoothed[i]);
            }
        }
        use ParamId as P;
        let mag = self.v(P::Magnitude);
        let amount = |e: &Self, m: Module| e.v(m.amount()) * mag;

        self.noise.set(
            &NoiseParams {
                on: self.on(P::NoiseOn),
                amount: amount(self, Module::Noise),
                kind: NoiseType::from_value(self.v(P::NoiseType)),
                tone: self.v(P::NoiseTone),
                follow: self.v(P::NoiseFollow),
                duck: self.v(P::NoiseDuck),
                flux: self.v(P::NoiseFlux),
            },
            n,
        );
        self.noise_post = self.on(P::NoiseRouting);
        self.wobble.set(
            &WobbleParams {
                on: self.on(P::WobbleOn),
                amount: amount(self, Module::Wobble),
                balance: self.v(P::WobbleBalance),
                wow_rate: self.v(P::WobbleWowRate),
                flutter_rate: self.v(P::WobbleFlutterRate),
                stereo: self.on(P::WobbleStereo),
                mix: self.v(P::WobbleMix),
                flux: self.v(P::WobbleFlux),
            },
            n,
        );
        self.distort.set(
            &DistortParams {
                on: self.on(P::DistortOn),
                amount: amount(self, Module::Distort),
                kind: DistortType::from_value(self.v(P::DistortType)),
                focus_lo: self.v(P::DistortFocusLo),
                focus_hi: self.v(P::DistortFocusHi),
                mix: self.v(P::DistortMix),
                flux: self.v(P::DistortFlux),
            },
            n,
        );
        self.digital.set(
            &DigitalParams {
                on: self.on(P::DigitalOn),
                amount: amount(self, Module::Digital),
                balance: self.v(P::DigitalBalance),
                smooth: self.v(P::DigitalSmooth),
                focus_lo: self.v(P::DigitalFocusLo),
                focus_hi: self.v(P::DigitalFocusHi),
                cut: self.on(P::DigitalCut),
                mix: self.v(P::DigitalMix),
                flux: self.v(P::DigitalFlux),
            },
            n,
        );
        self.space.set(
            &SpaceParams {
                on: self.on(P::SpaceOn),
                amount: amount(self, Module::Space),
                kind: SpaceType::from_value(self.v(P::SpaceType)),
                decay: self.v(P::SpaceDecay),
                pre_delay_ms: self.v(P::SpacePreDelay),
                focus_lo: self.v(P::SpaceFocusLo),
                focus_hi: self.v(P::SpaceFocusHi),
                stereo: self.on(P::SpaceStereo),
                flux: self.v(P::SpaceFlux),
            },
            n,
        );
        self.loss.set(
            &LossParams {
                on: self.on(P::LossOn),
                amount: amount(self, Module::Loss),
                balance: self.v(P::LossBalance),
                dropouts: self.v(P::LossDropouts),
                gap: self.v(P::LossGap),
                stereo: self.on(P::LossStereo),
                flux: self.v(P::LossFlux),
            },
            n,
        );

        let sr = self.sr;
        let (lc, lc_hard) = (self.v(P::LowCut), self.on(P::LowCutSlope));
        let (hc, hc_hard) = (self.v(P::HighCut), self.on(P::HighCutSlope));
        let (tone, mid) = (self.v(P::Tone), self.on(P::ToneMode));
        for k in 0..2 {
            self.eq.low[k].set(lc, lc_hard, sr);
            self.eq.high[k].set(hc, hc_hard, sr);
            self.eq.tone[k].set(tone, mid, sr);
        }
        let eq_wet = if self.on(P::EqOn) { mag } else { 0.0 };
        self.eq.wet.set(eq_wet, ramp);
        self.in_gain.set(fast_db_to_gain(self.v(P::InGain) * mag), ramp);
        self.out_gain.set(fast_db_to_gain(self.v(P::OutGain) * mag), ramp);
        self.width.set(self.v(P::Width), ramp);
    }

    /// Processes a stereo block in place. Any length.
    pub fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        let _denormals = DenormalGuard::new();
        let total = left.len().min(right.len());
        let mut in_peak = [0.0f32; 2];
        let mut out_peak = [0.0f32; 2];
        let mut start = 0;
        while start < total {
            if self.chunk_left == 0 {
                self.update(CHUNK);
                self.chunk_left = CHUNK;
            }
            let n = (total - start).min(self.chunk_left);
            self.chunk_left -= n;
            let l = &mut left[start..start + n];
            let r = &mut right[start..start + n];

            for i in 0..n {
                let g = self.in_gain.next();
                l[i] *= g;
                r[i] *= g;
                in_peak[0] = in_peak[0].max(l[i].abs());
                in_peak[1] = in_peak[1].max(r[i].abs());
            }

            let (nl, nr) = (&mut self.noise_l[..n], &mut self.noise_r[..n]);
            self.noise.render(l, r, nl, nr, self.playing);
            if !self.noise_post {
                add(l, nl);
                add(r, nr);
            }

            self.wobble.process(l, r);
            self.distort.process(l, r);
            self.digital.process(l, r);
            self.space.process(l, r);
            self.loss.process(l, r);

            if !self.eq.wet.is_silent() {
                for i in 0..n {
                    let wet = self.eq.wet.next();
                    for (k, x) in [&mut l[i], &mut r[i]].into_iter().enumerate() {
                        let y = self.eq.tone[k].process(self.eq.high[k].process(self.eq.low[k].process(*x)));
                        *x += (y - *x) * wet;
                    }
                }
            } else {
                // Keep the filters' states moving so switching on is smooth.
                for i in 0..n {
                    for (k, x) in [l[i], r[i]].into_iter().enumerate() {
                        self.eq.tone[k].process(self.eq.high[k].process(self.eq.low[k].process(x)));
                    }
                }
            }

            if self.noise_post {
                add(l, &self.noise_l[..n]);
                add(r, &self.noise_r[..n]);
            }

            for i in 0..n {
                let w = self.width.next();
                let g = self.out_gain.next();
                let mid = 0.5 * (l[i] + r[i]);
                let side = 0.5 * (l[i] - r[i]) * w;
                l[i] = (mid + side) * g;
                r[i] = (mid - side) * g;
                out_peak[0] = out_peak[0].max(l[i].abs());
                out_peak[1] = out_peak[1].max(r[i].abs());
            }
            start += n;
        }

        let fb = &self.shared.feedback;
        for k in 0..2 {
            Feedback::raise(&fb.in_peak[k], in_peak[k]);
            Feedback::raise(&fb.out_peak[k], out_peak[k]);
        }
        let activity = [
            self.noise.activity(),
            self.wobble.activity(),
            self.distort.activity(),
            self.digital.activity(),
            self.space.activity(),
            self.loss.activity(),
        ];
        for (a, v) in fb.activity.iter().zip(activity) {
            a.store(v, Relaxed);
        }
        let (rate, bits) = self.digital.status();
        fb.digital_rate.store(rate, Relaxed);
        fb.digital_bits.store(bits, Relaxed);
    }
}

#[inline(always)]
fn add(dst: &mut [f32], src: &[f32]) {
    for (d, s) in dst.iter_mut().zip(src) {
        *d += s;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::dsp::common::TWO_PI;
    use crate::dsp::params::default_value;

    fn engine(values: &[f32; NUM_PARAMS]) -> Engine {
        let mut e = Engine::new(Arc::new(Shared::default()));
        e.prepare(48_000.0);
        e.set_params(values);
        e
    }

    fn sine(n: usize) -> Vec<f32> {
        (0..n).map(|i| (TWO_PI * 220.0 * i as f32 / 48_000.0).sin() * 0.5).collect()
    }

    fn render(e: &mut Engine, input: &[f32], block: usize) -> (Vec<f32>, Vec<f32>) {
        let mut l = input.to_vec();
        let mut r = input.to_vec();
        for (cl, cr) in l.chunks_mut(block).zip(r.chunks_mut(block)) {
            e.process(cl, cr);
        }
        (l, r)
    }

    #[test]
    fn magnitude_zero_is_the_dry_signal_delayed_by_the_latency() {
        let mut v = defaults();
        v[ParamId::Magnitude.index()] = 0.0;
        // Width is not scaled by Magnitude; at 100 % it is neutral anyway.
        assert_eq!(default_value(ParamId::Width), 1.0);
        let input = sine(10_000);
        let (l, _) = render(&mut engine(&v), &input, 333);
        for i in LATENCY..input.len() {
            assert!((l[i] - input[i - LATENCY]).abs() < 1.0e-6, "{i}: {} vs {}", l[i], input[i - LATENCY]);
        }
    }

    #[test]
    fn defaults_colour_the_sound_but_stay_tame() {
        let input = sine(96_000);
        let (l, r) = render(&mut engine(&defaults()), &input, 512);
        let peak = l.iter().chain(&r).fold(0.0f32, |a, b| a.max(b.abs()));
        assert!(peak.is_finite() && peak < 1.0, "{peak}");
        let diff: f32 = l[LATENCY..].iter().zip(&input).map(|(a, b)| (a - b).abs()).sum::<f32>() / input.len() as f32;
        assert!(diff > 1.0e-3, "defaults should do something: {diff}");
    }

    #[test]
    fn everything_at_full_stays_finite_and_bounded() {
        let mut v = defaults();
        for m in Module::ALL {
            v[m.on().index()] = 1.0;
            v[m.amount().index()] = 1.0;
            v[m.flux().index()] = 1.0;
        }
        v[ParamId::InGain.index()] = 24.0;
        v[ParamId::SpaceDecay.index()] = 20.0;
        let input = sine(96_000);
        let (l, r) = render(&mut engine(&v), &input, 128);
        for x in l.iter().chain(&r) {
            assert!(x.is_finite() && x.abs() < 8.0, "{x}");
        }
    }

    #[test]
    fn renders_repeat_exactly_whatever_the_block_size() {
        let input = sine(20_000);
        let a = render(&mut engine(&defaults()), &input, 512);
        let b = render(&mut engine(&defaults()), &input, 512);
        assert_eq!(a, b);
        for block in [1, 7, 100, 4_096] {
            let c = render(&mut engine(&defaults()), &input, block);
            let first = c.0.iter().zip(&a.0).position(|(x, y)| x != y);
            assert_eq!(first, None, "block {block}");
        }
    }

    #[test]
    fn post_noise_is_not_crushed() {
        // Post routing adds noise after the digital module: with silence in,
        // fully crushed digital and post noise, the output is noise with many
        // distinct values.
        let mut v = defaults();
        v[ParamId::NoiseType.index()] = 8.0; // white
        v[ParamId::NoiseAmount.index()] = 0.5;
        v[ParamId::NoiseRouting.index()] = 1.0;
        v[ParamId::DigitalAmount.index()] = 1.0;
        v[ParamId::DigitalBalance.index()] = 1.0;
        let input = vec![0.0; 20_000];
        let (l, _) = render(&mut engine(&v), &input, 256);
        let mut vals: Vec<i32> = l[5_000..].iter().map(|x| (x * 1.0e5) as i32).collect();
        vals.sort();
        vals.dedup();
        assert!(vals.len() > 1_000, "{}", vals.len());
    }

    #[test]
    fn meters_and_activity_are_published() {
        let shared = Arc::new(Shared::default());
        let mut e = Engine::new(shared.clone());
        e.set_params(&defaults());
        let input = sine(4_800);
        let mut l = input.clone();
        let mut r = input;
        e.process(&mut l, &mut r);
        assert!(Feedback::take(&shared.feedback.in_peak[0]) > 0.4);
        assert!(Feedback::take(&shared.feedback.out_peak[0]) > 0.1);
        assert_eq!(Feedback::take(&shared.feedback.in_peak[0]), 0.0);
        assert!(shared.feedback.activity(Module::Noise) > 0.0);
    }
}
