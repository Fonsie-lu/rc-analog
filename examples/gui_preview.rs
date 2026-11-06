//! Opens the editor in a standalone window without a plugin host. Parameter
//! changes go into a local store so every control can be operated and checked.
//!
//!   cargo run --release --example gui_preview
//!   RC21_THEME=0..3                    starts with a theme
//!   RC21_PRESET=n                      starts with factory preset n loaded
//!
//! With `RC21_DEMO=1` the engine processes a test signal (a drum-like
//! pulse and a chord) in a background thread at real-time pace (no audio
//! output), following every edit made in the window, so the meters and the
//! module activity strips can be checked.
use baseview::{Size, WindowOpenOptions, WindowScalePolicy};
use egui_baseview::{EguiWindow, GraphicsConfig};
use rc_21::dsp::engine::Engine;
use rc_21::dsp::params::{ParamId, NUM_PARAMS};
use rc_21::dsp::presets::{factory_values, FACTORY};
use rc_21::dsp::shared::Shared;
use rc_21::editor::{self, EditorState, PreviewStore};
use rc_21::params::Rc21Params;
use nih_plug::prelude::*;
use nih_plug::wrapper::state::PluginState;
use std::sync::Arc;
use std::time::{Duration, Instant};

struct PreviewContext {
    store: PreviewStore,
}

impl GuiContext for PreviewContext {
    fn plugin_api(&self) -> PluginApi {
        PluginApi::Clap
    }
    fn request_resize(&self) -> bool {
        false
    }
    unsafe fn raw_begin_set_parameter(&self, _param: ParamPtr) {}
    unsafe fn raw_set_parameter_normalized(&self, param: ParamPtr, normalized: f32) {
        self.store.lock().unwrap().insert(param, normalized);
    }
    unsafe fn raw_end_set_parameter(&self, _param: ParamPtr) {}
    fn get_state(&self) -> PluginState {
        unimplemented!()
    }
    fn set_state(&self, _state: PluginState) {}
}

fn main() {
    let shared = Arc::new(Shared::default());
    let params = Arc::new(Rc21Params::new(shared.clone()));
    if let Some(i) = std::env::var("RC21_THEME").ok().and_then(|v| v.parse().ok()) {
        params.theme.store(i, std::sync::atomic::Ordering::Relaxed);
    }
    let store = PreviewStore::default();
    let preset: Option<usize> = std::env::var("RC21_PRESET").ok().and_then(|v| v.parse().ok());
    if let Some((i, values)) = preset.and_then(|i| factory_values(i).map(|v| (i, v))) {
        let mut store = store.lock().unwrap();
        for id in ParamId::all() {
            store.insert(params.get(id).ptr(), id.desc().to_normalized(values[id.index()]));
        }
        shared.set_preset_name(FACTORY[i].name);
    }
    if std::env::var("RC21_DEMO").is_ok() {
        let (shared, params, store) = (shared.clone(), params.clone(), store.clone());
        std::thread::spawn(move || demo(shared, params, store));
    }
    let settings = WindowOpenOptions {
        title: "RC-21 Preview".into(),
        size: Size::new(editor::WIDTH as f64, editor::HEIGHT as f64),
        scale: WindowScalePolicy::ScaleFactor(1.0),
        gl_config: Some(Default::default()),
    };
    let context = PreviewContext { store: store.clone() };
    EguiWindow::open_blocking(
        settings,
        GraphicsConfig::default(),
        EditorState::new(shared).with_preview_store(store),
        |ctx, _queue, _state| editor::setup_style(ctx),
        move |ctx, _queue, state| {
            let setter = ParamSetter::new(&context);
            editor::root(ctx, &params, &setter, state);
        },
    );
}

/// Processes a test signal with the window's current values at real-time
/// pace and publishes what the plugin would publish.
fn demo(shared: Arc<Shared>, params: Arc<Rc21Params>, store: PreviewStore) {
    const SR: f32 = 48_000.0;
    const BLOCK: usize = 256;
    let mut engine = Engine::new(shared);
    engine.prepare(SR);
    let (mut l, mut r) = (vec![0.0; BLOCK], vec![0.0; BLOCK]);
    let start = Instant::now();
    let mut n = 0usize;
    loop {
        let values: [f32; NUM_PARAMS] = {
            let store = store.lock().unwrap();
            std::array::from_fn(|i| {
                let id = ParamId::from_index(i);
                let hp = params.get(id);
                let v = store.get(&hp.ptr()).copied().unwrap_or_else(|| hp.normalized());
                id.desc().from_normalized(v)
            })
        };
        engine.set_params(&values);
        for k in 0..BLOCK {
            let t = (n + k) as f32 / SR;
            // A thump every half second over a soft chord.
            let beat = t % 0.5;
            let kick = (-beat * 18.0).exp() * (std::f32::consts::TAU * (55.0 + 80.0 * (-beat * 30.0).exp()) * beat).sin();
            let chord: f32 = [220.0, 277.18, 329.63].iter().map(|f| (std::f32::consts::TAU * f * t).sin()).sum();
            l[k] = 0.5 * kick + 0.08 * chord;
            r[k] = l[k];
        }
        engine.process(&mut l, &mut r);
        n += BLOCK;
        let due = Duration::from_secs_f32(n as f32 / SR);
        if let Some(wait) = due.checked_sub(start.elapsed()) {
            std::thread::sleep(wait);
        }
    }
}
