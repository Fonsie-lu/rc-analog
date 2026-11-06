//! Rough CPU cost of the effect chain, as a fraction of one core in real time.
//!
//!   cargo run --release --example bench
use rc_analog::dsp::engine::Engine;
use rc_analog::dsp::params::{defaults, Module, ParamId, NUM_PARAMS};
use rc_analog::dsp::shared::Shared;
use std::sync::Arc;
use std::time::Instant;

fn run(name: &str, values: &[f32; NUM_PARAMS], sample_rate: f32, block: usize) {
    let mut engine = Engine::new(Arc::new(Shared::default()));
    engine.prepare(sample_rate);
    engine.set_params(values);
    let seconds = 10.0;
    let total = (seconds * sample_rate) as usize;
    let mut l: Vec<f32> = (0..block).map(|i| (i as f32 * 0.03).sin() * 0.5).collect();
    let mut r = l.clone();
    // Warm up, then measure.
    for _ in 0..(sample_rate as usize / block) {
        engine.process(&mut l, &mut r);
    }
    let start = Instant::now();
    let mut done = 0;
    while done < total {
        engine.process(&mut l, &mut r);
        done += block;
    }
    let load = start.elapsed().as_secs_f32() / (done as f32 / sample_rate);
    println!("{name:<44} {:>6.2} %", load * 100.0);
}

fn main() {
    let mut full = defaults();
    for m in Module::ALL {
        full[m.on().index()] = 1.0;
        full[m.amount().index()] = 0.8;
        full[m.flux().index()] = 0.5;
    }
    full[ParamId::SpaceType.index()] = 1.0;
    run("defaults, 48 k / 128", &defaults(), 48_000.0, 128);
    run("all modules on, 48 k / 128", &full, 48_000.0, 128);
    run("all modules on, 48 k / 32", &full, 48_000.0, 32);
    run("all modules on, 96 k / 256", &full, 96_000.0, 256);
    run("all modules on, 192 k / 512", &full, 192_000.0, 512);
}
