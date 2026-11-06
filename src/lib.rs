//! RC-21 - an analog colour effect (CLAP and VST3): noise, wobble,
//! distortion, digital crunch, space and tape loss in one chain.
//!
//! Two layers with a hard boundary: [`dsp`] holds all DSP and state and knows
//! nothing about nih-plug; this file, [`params`] and [`editor`] adapt it to
//! the host. Host parameters are copied into the engine at the start of every
//! block, so the engine's parameter array belongs to the audio thread alone.

use nih_plug::prelude::*;
use std::num::NonZeroU32;
use std::sync::atomic::Ordering::Relaxed;
use std::sync::Arc;
use std::time::Instant;

pub mod dsp;
pub mod editor;
pub mod params;

use dsp::engine::{Engine, LATENCY};
use dsp::shared::Shared;
use params::Rc21Params;

pub struct Rc21 {
    params: Arc<Rc21Params>,
    shared: Arc<Shared>,
    engine: Engine,
    sample_rate: f32,
    /// Right channel stand-in for mono layouts; sized in `initialize`.
    mono_scratch: Vec<f32>,
}

impl Default for Rc21 {
    fn default() -> Self {
        let shared = Arc::new(Shared::default());
        Self {
            params: Arc::new(Rc21Params::new(shared.clone())),
            engine: Engine::new(shared.clone()),
            shared,
            sample_rate: 48_000.0,
            mono_scratch: Vec::new(),
        }
    }
}

impl Plugin for Rc21 {
    const NAME: &'static str = "RC-21";
    const VENDOR: &'static str = "Fonsie Audio";
    const URL: &'static str = "https://github.com/Fonsie-lu/rc-20";
    const EMAIL: &'static str = "beat.weber@bithawk.ch";
    const VERSION: &'static str = env!("CARGO_PKG_VERSION");

    const AUDIO_IO_LAYOUTS: &'static [AudioIOLayout] = &[
        AudioIOLayout {
            main_input_channels: NonZeroU32::new(2),
            main_output_channels: NonZeroU32::new(2),
            ..AudioIOLayout::const_default()
        },
        AudioIOLayout {
            main_input_channels: NonZeroU32::new(1),
            main_output_channels: NonZeroU32::new(1),
            ..AudioIOLayout::const_default()
        },
    ];

    const SAMPLE_ACCURATE_AUTOMATION: bool = false;

    type SysExMessage = ();
    type BackgroundTask = ();

    fn params(&self) -> Arc<dyn Params> {
        self.params.clone()
    }

    fn editor(&mut self, _async_executor: AsyncExecutor<Self>) -> Option<Box<dyn Editor>> {
        editor::create(self.params.clone(), self.shared.clone())
    }

    fn initialize(
        &mut self,
        _audio_io_layout: &AudioIOLayout,
        buffer_config: &BufferConfig,
        context: &mut impl InitContext<Self>,
    ) -> bool {
        self.sample_rate = buffer_config.sample_rate;
        self.engine.prepare(buffer_config.sample_rate);
        self.engine.set_params(&self.params.snapshot());
        self.mono_scratch = vec![0.0; buffer_config.max_buffer_size as usize];
        context.set_latency_samples(LATENCY as u32);
        true
    }

    fn reset(&mut self) {
        self.engine.reset();
    }

    fn process(
        &mut self,
        buffer: &mut Buffer,
        _aux: &mut AuxiliaryBuffers,
        context: &mut impl ProcessContext<Self>,
    ) -> ProcessStatus {
        let start = Instant::now();
        let num_samples = buffer.samples();
        self.engine.set_playing(context.transport().playing);
        self.engine.set_params(&self.params.snapshot());

        let channels = buffer.as_slice();
        match channels.len() {
            0 => {}
            1 => {
                // Mono: run the stereo chain on a copy and fold it back down.
                let left = &mut *channels[0];
                let size = self.mono_scratch.len().max(1);
                for piece in left.chunks_mut(size) {
                    let right = &mut self.mono_scratch[..piece.len()];
                    right.copy_from_slice(piece);
                    self.engine.process(piece, right);
                    for (l, r) in piece.iter_mut().zip(right.iter()) {
                        *l = 0.5 * (*l + *r);
                    }
                }
            }
            _ => {
                let (left, rest) = channels.split_at_mut(1);
                self.engine.process(left[0], rest[0]);
            }
        }

        let budget = num_samples as f32 / self.sample_rate;
        if budget > 0.0 {
            let fb = &self.shared.feedback;
            let load = start.elapsed().as_secs_f32() / budget;
            fb.cpu_load.store(fb.cpu_load.load(Relaxed) * 0.9 + load * 0.1, Relaxed);
        }
        // Reverb tails and noise keep sounding after the input stops.
        ProcessStatus::KeepAlive
    }
}

impl ClapPlugin for Rc21 {
    const CLAP_ID: &'static str = "ch.fonsie.rc-21";
    const CLAP_DESCRIPTION: Option<&'static str> =
        Some("Analog colour: noise, wobble, distortion, digital crunch, space and tape loss");
    const CLAP_MANUAL_URL: Option<&'static str> = None;
    const CLAP_SUPPORT_URL: Option<&'static str> = None;
    const CLAP_FEATURES: &'static [ClapFeature] =
        &[ClapFeature::AudioEffect, ClapFeature::Stereo, ClapFeature::Mono, ClapFeature::Distortion];
}

impl Vst3Plugin for Rc21 {
    const VST3_CLASS_ID: [u8; 16] = *b"FonsieRC21Effect";
    const VST3_SUBCATEGORIES: &'static [Vst3SubCategory] =
        &[Vst3SubCategory::Fx, Vst3SubCategory::Distortion, Vst3SubCategory::Stereo];
}

nih_export_clap!(Rc21);
nih_export_vst3!(Rc21);
