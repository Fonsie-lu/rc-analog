//! Headless GUI tests: the editor runs in a real egui context with simulated
//! mouse input against a mock host that applies parameter changes late, like
//! nih-plug's CLAP wrapper does, and checks gesture balance.

use super::*;
use crate::dsp::params::{default_value, Module};
use nih_plug::wrapper::state::PluginState;
use nih_plug_egui::egui::{Event, Modifiers, PointerButton, RawInput, Vec2};

const SCREEN: Vec2 = Vec2::new(BW, BH);

#[derive(Default)]
struct MockHost {
    /// Values the "plugin" has applied (what the GUI reads back).
    store: PreviewStore,
    /// Values sent by the GUI but not applied yet.
    queue: Mutex<Vec<(ParamPtr, f32)>>,
    open_gestures: Mutex<HashMap<ParamPtr, i32>>,
    errors: Mutex<Vec<String>>,
}

impl MockHost {
    fn flush(&self) {
        let mut store = self.store.lock().unwrap();
        for (ptr, v) in self.queue.lock().unwrap().drain(..) {
            store.insert(ptr, v);
        }
    }
}

impl GuiContext for MockHost {
    fn plugin_api(&self) -> PluginApi {
        PluginApi::Clap
    }
    fn request_resize(&self) -> bool {
        false
    }
    unsafe fn raw_begin_set_parameter(&self, param: ParamPtr) {
        let mut g = self.open_gestures.lock().unwrap();
        let n = g.entry(param).or_default();
        if *n != 0 {
            self.errors.lock().unwrap().push(format!("nested gesture: {}", param.name()));
        }
        *n += 1;
    }
    unsafe fn raw_set_parameter_normalized(&self, param: ParamPtr, normalized: f32) {
        if self.open_gestures.lock().unwrap().get(&param).copied().unwrap_or(0) != 1 {
            self.errors.lock().unwrap().push(format!("set outside gesture: {}", param.name()));
        }
        self.queue.lock().unwrap().push((param, normalized));
    }
    unsafe fn raw_end_set_parameter(&self, param: ParamPtr) {
        let mut g = self.open_gestures.lock().unwrap();
        let n = g.entry(param).or_default();
        *n -= 1;
        if *n != 0 {
            self.errors.lock().unwrap().push(format!("unbalanced end: {}", param.name()));
        }
    }
    fn get_state(&self) -> PluginState {
        unimplemented!()
    }
    fn set_state(&self, _state: PluginState) {}
}

struct Harness {
    ctx: egui::Context,
    params: Rc21Params,
    host: MockHost,
    state: EditorState,
    time: f64,
    frame: u64,
    /// Host applies queued values every `latency` frames.
    latency: u64,
    events: Vec<Event>,
    pointer: Pos2,
    dir: PathBuf,
}

fn scratch_dir(tag: &str) -> PathBuf {
    static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("rc21-editor-{}-{tag}-{n}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    dir
}

impl Harness {
    fn new(latency: u64) -> Self {
        let ctx = egui::Context::default();
        setup_style(&ctx);
        let host = MockHost::default();
        let shared = Arc::new(Shared::default());
        let mut state = EditorState::new(shared.clone()).with_preview_store(host.store.clone());
        // Never the real library: tests must not see or touch user presets.
        let dir = scratch_dir("presets");
        state.presets_dir = dir.clone();
        state.user_presets.clear();
        let mut h = Self {
            ctx,
            params: Rc21Params::new(shared),
            host,
            state,
            time: 0.0,
            frame: 0,
            latency,
            events: Vec::new(),
            pointer: pos2(0.0, 0.0),
            dir,
        };
        h.run(2);
        h
    }

    fn run(&mut self, frames: usize) {
        for _ in 0..frames {
            let raw = RawInput {
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, SCREEN)),
                time: Some(self.time),
                events: std::mem::take(&mut self.events),
                ..Default::default()
            };
            let setter = ParamSetter::new(&self.host);
            let (params, state) = (&self.params, &mut self.state);
            let _ = self.ctx.run(raw, |ctx| root(ctx, params, &setter, state));
            self.time += 1.0 / 60.0;
            self.frame += 1;
            if self.latency > 0 && self.frame.is_multiple_of(self.latency) {
                self.host.flush();
            }
        }
    }

    fn settle(&mut self) {
        self.run(3);
        self.host.flush();
        self.run(2);
    }

    fn move_to(&mut self, p: Pos2) {
        self.pointer = p;
        self.events.push(Event::PointerMoved(p));
    }

    fn press(&mut self, pressed: bool) {
        self.events.push(Event::PointerButton {
            pos: self.pointer,
            button: PointerButton::Primary,
            pressed,
            modifiers: Modifiers::default(),
        });
    }

    fn click(&mut self, p: Pos2) {
        self.move_to(p);
        self.run(1);
        self.press(true);
        self.run(1);
        self.press(false);
        self.run(2);
    }

    fn double_click(&mut self, p: Pos2) {
        self.move_to(p);
        self.run(1);
        for _ in 0..2 {
            self.press(true);
            self.run(1);
            self.press(false);
            self.run(1);
        }
        self.run(2);
    }

    /// Press at `from`, move to `to` in steps, release.
    fn drag(&mut self, from: Pos2, to: Pos2) {
        self.move_to(from);
        self.run(1);
        self.press(true);
        self.run(1);
        for k in 1..=10 {
            self.move_to(from + (to - from) * (k as f32 / 10.0));
            self.run(1);
        }
        self.press(false);
        self.run(2);
    }

    fn control(&self, name: &str) -> ControlInfo {
        self.state
            .controls
            .iter()
            .find(|c| c.name == name)
            .cloned()
            .unwrap_or_else(|| panic!("no control {name}"))
    }

    /// Value the host holds, in natural units.
    fn value(&self, id: ParamId) -> f32 {
        let hp = self.params.get(id);
        let n = self.host.store.lock().unwrap().get(&hp.ptr()).copied().unwrap_or_else(|| hp.normalized());
        id.desc().from_normalized(n)
    }

    fn assert_clean(&self) {
        let errors = self.host.errors.lock().unwrap();
        assert!(errors.is_empty(), "{errors:?}");
        let open = self.host.open_gestures.lock().unwrap();
        assert!(open.values().all(|n| *n == 0), "open gestures: {open:?}");
    }
}

impl Drop for Harness {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

#[test]
fn every_parameter_has_a_control() {
    let h = Harness::new(1);
    for id in ParamId::all() {
        let name = id.desc().id_string;
        assert!(h.state.controls.iter().any(|c| c.name == name), "{name} has no control");
    }
    // Everything lies on the canvas.
    for c in &h.state.controls {
        assert!(c.rect.min.x >= 0.0 && c.rect.min.y >= 0.0 && c.rect.max.x <= BW + 0.5 && c.rect.max.y <= BH + 0.5, "{} off canvas", c.name);
    }
}

#[test]
fn knob_drags_and_double_click_resets() {
    let mut h = Harness::new(3);
    let knob = h.control("wobble.amount");
    let c = knob.rect.center();
    h.drag(c, c - vec2(0.0, 60.0));
    h.settle();
    let up = h.value(ParamId::WobbleAmount);
    assert!(up > default_value(ParamId::WobbleAmount) + 0.2, "{up}");
    h.double_click(c);
    h.settle();
    assert!((h.value(ParamId::WobbleAmount) - default_value(ParamId::WobbleAmount)).abs() < 1.0e-4);
    h.assert_clean();
}

#[test]
fn horizontal_sliders_follow_the_pointer() {
    let mut h = Harness::new(2);
    let slider = h.control("global.magnitude");
    let c = slider.rect.center();
    h.drag(c, c - vec2(slider.rect.width() * 0.3, 0.0));
    h.settle();
    let v = h.value(ParamId::Magnitude);
    assert!(v < 0.75 && v > 0.5, "{v}");
    h.assert_clean();
}

#[test]
fn power_square_toggles_and_the_hatch_switches_back_on() {
    let mut h = Harness::new(1);
    let power = h.control("space.on");
    h.click(power.rect.left_center() + vec2(10.0, 0.0));
    h.settle();
    assert_eq!(h.value(ParamId::SpaceOn), 0.0);
    let hatch = h.control("hatch:SPACE");
    h.click(hatch.rect.center());
    h.settle();
    assert_eq!(h.value(ParamId::SpaceOn), 1.0);
    h.assert_clean();
}

#[test]
fn focus_handles_move_and_never_cross() {
    let mut h = Harness::new(2);
    let lo = h.control("distort.focusLo");
    let hi = h.control("distort.focusHi");
    // Low handle right, past the high one.
    h.drag(lo.rect.center(), hi.rect.center() + vec2(50.0, 0.0));
    h.settle();
    let (l, u) = (h.value(ParamId::DistortFocusLo), h.value(ParamId::DistortFocusHi));
    assert!(l > 1_000.0, "{l}");
    assert!(l <= u + 1.0, "{l} > {u}");
    // Double-click resets both; then the high handle moves left on its own.
    h.double_click(h.control("distort.focusHi").rect.center() - vec2(40.0, 0.0));
    h.settle();
    assert_eq!(h.value(ParamId::DistortFocusLo), default_value(ParamId::DistortFocusLo));
    let hi = h.control("distort.focusHi");
    h.drag(hi.rect.center(), hi.rect.center() - vec2(60.0, 0.0));
    h.settle();
    let (l2, u2) = (h.value(ParamId::DistortFocusLo), h.value(ParamId::DistortFocusHi));
    assert!(u2 < 5_000.0, "{u2} vs {u}");
    assert!(l2 <= u2 + 1.0);
    h.assert_clean();
}

#[test]
fn choice_list_and_segments_select() {
    let mut h = Harness::new(1);
    let sel = h.control("distort.type");
    h.click(sel.rect.center());
    let item = h.control("choice:distort.type:4");
    h.click(item.rect.center());
    h.settle();
    assert_eq!(h.value(ParamId::DistortType), 4.0);
    assert!(h.state.popup.is_none());

    let cell = h.control("noise.routing#1");
    h.click(cell.rect.center());
    h.settle();
    assert_eq!(h.value(ParamId::NoiseRouting), 1.0);

    // Selector arrows step.
    let sel = h.control("space.type");
    h.click(sel.rect.right_center() - vec2(6.0, 0.0));
    h.settle();
    assert_eq!(h.value(ParamId::SpaceType), 1.0);
    h.assert_clean();
}

#[test]
fn presets_load_from_the_list_and_step() {
    let mut h = Harness::new(2);
    h.click(h.control("preset").rect.center());
    h.click(h.control("preset:4").rect.center());
    h.settle();
    assert_eq!(h.state.preset, Some(4));
    assert_eq!(h.state.shared.preset_name(), FACTORY[4].name);
    let want = factory_values(4).unwrap();
    for id in ParamId::all() {
        assert!((h.value(id) - want[id.index()]).abs() < 1.0e-2 * (id.desc().max - id.desc().min).max(1.0), "{}", id.desc().id_string);
    }
    h.click(h.control("preset-next").rect.center());
    h.settle();
    assert_eq!(h.state.preset, Some(5));
    h.click(h.control("preset-prev").rect.center());
    h.click(h.control("preset-prev").rect.center());
    h.settle();
    assert_eq!(h.state.preset, Some(3));
    h.assert_clean();
}

#[test]
fn saving_adds_a_user_preset() {
    let mut h = Harness::new(1);
    let knob = h.control("loss.amount");
    h.drag(knob.rect.center(), knob.rect.center() - vec2(0.0, 80.0));
    h.settle();
    let amount = h.value(ParamId::LossAmount);
    // Answer the save dialog as zenity would.
    let path = h.dir.join("My Tape");
    let job: FileJob = Arc::new(Mutex::new(Some(Ok(path.display().to_string()))));
    h.state.save_job = Some(job);
    h.run(2);
    assert_eq!(h.state.user_presets.len(), 1);
    assert_eq!(h.state.shared.preset_name(), "My Tape");
    let index = num_factory_presets();
    assert_eq!(h.state.preset, Some(index));

    // Change something, then load the saved preset back.
    h.click(h.control("preset").rect.center());
    h.click(h.control("preset:0").rect.center());
    h.settle();
    assert!((h.value(ParamId::LossAmount) - default_value(ParamId::LossAmount)).abs() < 1.0e-4);
    h.click(h.control("preset").rect.center());
    h.click(h.control(&format!("preset:{index}")).rect.center());
    h.settle();
    assert!((h.value(ParamId::LossAmount) - amount).abs() < 1.0e-3);
    h.assert_clean();
}

#[test]
fn theme_chips_switch_the_theme() {
    let mut h = Harness::new(1);
    h.click(h.control("theme:GRUVBOX").rect.center());
    assert_eq!(h.params.theme.load(Relaxed), 3);
    // Every theme draws without trouble.
    for i in 0..THEMES.len() as u8 {
        h.params.theme.store(i, Relaxed);
        h.run(2);
    }
}

#[test]
fn big_knobs_are_the_module_amounts() {
    let h = Harness::new(1);
    for m in Module::ALL {
        let k = h.control(m.amount().desc().id_string);
        assert_eq!(k.kind, ControlKind::Knob);
        assert!(k.rect.width() > 80.0, "{:?} big knob too small", m);
    }
}
