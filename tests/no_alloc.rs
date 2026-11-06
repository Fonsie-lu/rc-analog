//! The audio path never allocates. Every factory preset renders with the
//! allocator armed to abort, at block sizes from 1 sample up, with parameter
//! changes, transport stops and a reset in between.

use assert_no_alloc::assert_no_alloc;
use rc_21::dsp::engine::Engine;
use rc_21::dsp::params::{Module, ParamId};
use rc_21::dsp::presets::{factory_values, num_factory_presets};
use rc_21::dsp::shared::Shared;
use std::sync::Arc;

// nih-plug's `assert_process_allocs` feature already installs the checking
// global allocator, so it is active in this test binary too.

#[test]
fn rendering_every_preset_does_not_allocate() {
    for i in 0..num_factory_presets() {
        let mut values = factory_values(i).unwrap();
        let mut engine = Engine::new(Arc::new(Shared::default()));
        engine.prepare(44_100.0);
        engine.set_params(&values);
        let mut l: Vec<f32> = (0..2_000).map(|k| (k as f32 * 0.05).sin() * 0.4).collect();
        let mut r = l.clone();
        assert_no_alloc(|| {
            for (k, block) in [1usize, 32, 128, 511, 2_000, 64].into_iter().cycle().take(60).enumerate() {
                match k {
                    10 => {
                        // Every module on and at full, every type switch.
                        for m in Module::ALL {
                            values[m.on().index()] = 1.0;
                            values[m.amount().index()] = 1.0;
                            values[m.flux().index()] = 1.0;
                        }
                        values[ParamId::NoiseType.index()] = 7.0;
                        values[ParamId::DistortType.index()] = 4.0;
                        values[ParamId::SpaceType.index()] = 3.0;
                    }
                    20 => engine.set_playing(false),
                    30 => engine.reset(),
                    40 => engine.set_playing(true),
                    _ => {}
                }
                engine.set_params(&values);
                engine.process(&mut l[..block], &mut r[..block]);
            }
        });
        assert!(l.iter().chain(&r).all(|x| x.is_finite()), "preset {i}");
    }
}
