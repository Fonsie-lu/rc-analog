//! Hardware-style editor in the look of the sibling plugins: coloured section
//! bars with power squares, knobs with value arcs, cell lists, and four
//! colour themes.
//!
//! Layout: a header (preset browser, CPU, themes), the Magnitude slider and
//! the value readout, the six modules side by side in signal order, and the
//! master section (input, EQ, output) along the bottom.
//!
//! Everything is painted on a fixed `BW` x `BH` canvas through [`Cv`] and
//! scaled uniformly to the window; drag the bottom right corner to resize.

mod panel;
pub mod theme;
mod widgets;

use nih_plug::prelude::*;
use nih_plug_egui::create_egui_editor;
use nih_plug_egui::egui::{self, pos2, vec2, Align2, Color32, FontFamily, Id, Pos2, Rect, Sense, Stroke, StrokeKind, TextureHandle, Ui};
use nih_plug_egui::resizable_window::ResizableWindow;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering::Relaxed;
use std::sync::{Arc, Mutex};

use crate::dsp::params::{ParamId, NUM_PARAMS};
use crate::dsp::presets::{self, factory_values, num_factory_presets, UserPreset, FACTORY};
use crate::dsp::shared::Shared;
use crate::params::RcAnalogParams;
use theme::{Theme, THEMES};
pub use widgets::{Cv, G};

/// Canvas size in logical pixels.
pub(crate) const BW: f32 = 1240.0;
pub(crate) const BH: f32 = 760.0;
/// Default window size: the canvas at 100 %.
pub const WIDTH: u32 = BW as u32;
pub const HEIGHT: u32 = BH as u32;

/// How long a value set by the GUI is shown before trusting the parameter again.
pub(crate) const PENDING_TIMEOUT: f64 = 1.0;

/// Preset list geometry: rows per column, column width, columns at most.
const PRESET_ROWS: usize = 20;
const PRESET_COL_W: f32 = 250.0;
const PRESET_MAX_COLS: usize = 4;
const POPUP_ROW: f32 = 18.0;

pub struct Focus {
    pub name: String,
    pub value: String,
}

pub(crate) struct Pending {
    pub value: f32,
    pub time: f64,
}

/// Overlay lists. Drawn last so they sit on top and take the clicks.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Popup {
    Presets { at: Pos2 },
    Choice { param: ParamId, at: Pos2 },
}

/// Shared parameter store used by the standalone preview and the tests,
/// which have no plugin wrapper that could apply parameter changes.
pub type PreviewStore = Arc<Mutex<HashMap<ParamPtr, f32>>>;

/// Result of a file dialog running on a helper thread.
pub(crate) type FileJob = Arc<Mutex<Option<Result<String, String>>>>;

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ControlKind {
    Knob,
    Slider,
    /// One handle of a focus range, registered under its parameter.
    Handle,
    Selector,
    /// Choice as a row of cells; registered whole under the parameter ID and
    /// per cell as `"<id>#<index>"`.
    List,
    Switch,
    Button,
}

/// Screen rectangle of an interactive control, recorded every frame (tests).
#[derive(Clone, Debug)]
pub struct ControlInfo {
    /// Parameter ID string for parameter controls, a fixed key otherwise.
    pub name: String,
    pub kind: ControlKind,
    pub rect: Rect,
}

pub struct EditorState {
    pub(crate) shared: Arc<Shared>,
    pub(crate) focus: Option<Focus>,
    texture: Option<(u8, TextureHandle)>,
    pub(crate) pending: HashMap<ParamId, Pending>,
    pub(crate) preview_store: Option<PreviewStore>,
    pub controls: Vec<ControlInfo>,
    pub(crate) popup: Option<Popup>,
    /// Last preset loaded from the header: factory presets first, then
    /// `user_presets`.
    pub preset: Option<usize>,
    pub(crate) user_presets: Vec<UserPreset>,
    pub(crate) presets_dir: PathBuf,
    pub(crate) save_job: Option<FileJob>,
    pub(crate) message: String,
    /// Displayed meter levels (in L, in R, out L, out R), falling smoothly.
    pub(crate) meters: [f32; 4],
}

impl EditorState {
    pub fn new(shared: Arc<Shared>) -> Self {
        let dir = presets::user_dir();
        Self {
            shared,
            focus: None,
            texture: None,
            pending: HashMap::new(),
            preview_store: None,
            controls: Vec::new(),
            popup: None,
            preset: None,
            user_presets: presets::list_user(&dir),
            presets_dir: dir,
            save_job: None,
            message: String::new(),
            meters: [0.0; 4],
        }
    }

    /// Read parameter values from `store` instead of the parameters themselves.
    pub fn with_preview_store(mut self, store: PreviewStore) -> Self {
        self.preview_store = Some(store);
        self
    }
}

pub fn create(params: Arc<RcAnalogParams>, shared: Arc<Shared>) -> Option<Box<dyn Editor>> {
    create_egui_editor(
        params.editor_state.clone(),
        EditorState::new(shared),
        |ctx, _| setup_style(ctx),
        move |ctx, setter, state| root(ctx, &params, setter, state),
    )
}

pub fn setup_style(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    fonts.font_data.insert(
        "panel".to_owned(),
        Arc::new(egui::FontData::from_static(include_bytes!("../../assets/fonts/DejaVuSansCondensed-Bold.ttf"))),
    );
    fonts.font_data.insert(
        "oled".to_owned(),
        Arc::new(egui::FontData::from_static(include_bytes!("../../assets/fonts/DejaVuSansMono-Bold.ttf"))),
    );
    fonts.families.insert(FontFamily::Name("panel".into()), vec!["panel".to_owned()]);
    fonts.families.insert(FontFamily::Name("oled".into()), vec!["oled".to_owned()]);
    ctx.set_fonts(fonts);
}

pub fn root(ctx: &egui::Context, params: &RcAnalogParams, setter: &ParamSetter, state: &mut EditorState) {
    // Meters and activity strips animate without any input.
    ctx.request_repaint();
    ResizableWindow::new("rc-analog")
        .min_size(vec2(BW * 0.5, BH * 0.5))
        .show(ctx, params.editor_state.as_ref(), |ui| draw(ui, params, setter, state));
}

fn draw(ui: &mut Ui, p: &RcAnalogParams, set: &ParamSetter, st: &mut EditorState) {
    let full = ui.max_rect();
    let s = (full.width() / BW).min(full.height() / BH);
    let o = pos2(full.center().x - BW * s * 0.5, full.center().y - BH * s * 0.5);
    let cv = Cv { o, s };

    let theme_index = p.theme.load(Relaxed);
    let th = theme::get(theme_index);
    if st.texture.as_ref().map(|(i, _)| *i) != Some(theme_index) {
        let tex = ui.ctx().load_texture("panel-texture", panel_texture(th), egui::TextureOptions::LINEAR_REPEAT);
        st.texture = Some((theme_index, tex));
    }
    if ui.style().visuals.panel_fill != th.outer {
        ui.ctx().style_mut(|s| s.visuals.panel_fill = th.outer);
    }
    st.focus = None;
    st.controls.clear();

    let pt = ui.painter().clone();
    let mut g = G { ui, pt, cv, set, p, st, th, accent: th.accents[0] };
    g.panel();
    g.header();
    // A popup takes the clicks: controls underneath must not react to the
    // click that is meant for a list entry.
    let popup_rect = g.st.popup.map(|pop| g.popup_rect(pop));
    if let Some(r) = popup_rect {
        g.ui.interact(r, Id::new("popup-shield"), Sense::click_and_drag());
    }
    g.magnitude_row();
    g.modules();
    g.master();
    g.readout();
    g.popup();
}

/// Procedural powder coat: fine grain plus sparse lighter flecks.
fn panel_texture(th: &Theme) -> egui::ColorImage {
    let n = 256;
    let mut rng = crate::dsp::common::Rng::new(77);
    let mut px = Vec::with_capacity(n * n);
    for _ in 0..n * n {
        let grain = rng.next_bipolar() * th.grain;
        let fleck = if rng.next_float() > 0.985 { th.fleck } else { 0.0 };
        let v = |c: u8| (c as f32 + grain + fleck).clamp(0.0, 255.0) as u8;
        px.push(Color32::from_rgb(v(th.panel.r()), v(th.panel.g()), v(th.panel.b())));
    }
    egui::ColorImage { size: [n, n], pixels: px }
}

/// Runs `zenity` for a file name to save to, without blocking the GUI.
fn save_dialog(suggested: PathBuf) -> FileJob {
    let job: FileJob = Arc::new(Mutex::new(None));
    let out = job.clone();
    std::thread::spawn(move || {
        let result = std::process::Command::new("zenity")
            .arg("--file-selection")
            .arg("--save")
            .arg("--confirm-overwrite")
            .arg(format!("--filename={}", suggested.display()))
            .arg("--file-filter=RC-Analog presets | *.json")
            .output();
        let result = match result {
            Ok(o) if o.status.success() => Ok(String::from_utf8_lossy(&o.stdout).trim().to_owned()),
            Ok(_) => Err(String::new()),
            Err(e) => Err(format!("NO FILE DIALOG ({e})")),
        };
        if let Ok(mut j) = out.lock() {
            *j = Some(result);
        }
    });
    job
}

impl G<'_, '_> {
    fn panel(&self) {
        let cv = self.cv;
        let rect = cv.r(0.0, 0.0, BW, BH);
        let tex = &self.st.texture.as_ref().unwrap().1;
        let uv = Rect::from_min_max(pos2(0.0, 0.0), pos2(rect.width() / (256.0 * cv.s), rect.height() / (256.0 * cv.s)));
        self.pt.image(tex.id(), rect, uv, Color32::WHITE);
        self.pt.line_segment([cv.p(2.0, 1.0), cv.p(BW - 2.0, 1.0)], cv.stroke(1.5, self.th.bevel));
    }

    fn header(&mut self) {
        let cv = self.cv;
        self.text(cv.p(18.0, 22.0), Align2::LEFT_CENTER, "RC-ANALOG", 20.0, self.th.text);
        let w = self.text_w("RC-ANALOG", 20.0);
        self.text(cv.p(28.0 + w, 23.0), Align2::LEFT_CENTER, "ANALOG COLOUR", 11.0, self.th.header);
        self.poll_save_job();
        self.preset_browser();
        let cpu = self.st.shared.feedback.cpu_load.load(Relaxed);
        self.text(cv.p(694.0, 22.0), Align2::LEFT_CENTER, &format!("CPU {:.0}%", cpu * 100.0), 11.0, self.th.header);
        self.theme_selector();
    }

    /// Preset name with previous / next arrows and SAVE; clicking the name
    /// lists the factory and user presets.
    fn preset_browser(&mut self) {
        let cv = self.cv;
        let count = self.num_presets();
        let name = self.st.shared.preset_name().to_uppercase();
        let name_rect = cv.r(330.0, 11.0, 250.0, 22.0);
        let resp = self.ui.interact(name_rect, Id::new("preset-name"), Sense::click());
        self.register("preset", ControlKind::Button, name_rect);
        self.pt.rect_filled(name_rect, cv.round(3.0), self.th.oled_bg);
        self.pt.rect_stroke(
            name_rect,
            cv.round(3.0),
            Stroke::new(1.0 * cv.s, if resp.hovered() { self.th.oled_ink } else { self.th.oled_frame }),
            StrokeKind::Inside,
        );
        let head = match self.st.preset {
            Some(i) => format!("{:02}", i + 1),
            None => "--".to_owned(),
        };
        self.pt.text(cv.p(338.0, 22.0), Align2::LEFT_CENTER, &head, cv.mono(11.0), self.th.oled_dim);
        self.pt.text(cv.p(364.0, 22.0), Align2::LEFT_CENTER, &name, cv.mono(11.0), self.th.oled_ink);
        if resp.clicked() {
            self.refresh_user_presets();
            self.st.popup = Some(Popup::Presets { at: pos2(330.0, 36.0) });
        }
        if resp.hovered() {
            self.set_focus("PRESETS", "CLICK FOR THE LIST".to_owned());
        }
        self.accent = self.th.sublabel;
        if self.button("preset-prev", 302.0, 12.0, 24.0, "<", false, "PREVIOUS PRESET").clicked() {
            let i = self.st.preset.map(|i| (i + count - 1) % count).unwrap_or(count - 1);
            self.load_preset(i);
        }
        if self.button("preset-next", 584.0, 12.0, 24.0, ">", false, "NEXT PRESET").clicked() {
            let i = self.st.preset.map(|i| (i + 1) % count).unwrap_or(0);
            self.load_preset(i);
        }
        if self.button("preset-save", 616.0, 12.0, 56.0, "SAVE", false, "SAVE AS A USER PRESET").clicked() {
            let name = file_stem_for(&self.st.shared.preset_name());
            let _ = std::fs::create_dir_all(&self.st.presets_dir);
            self.st.save_job = Some(save_dialog(self.st.presets_dir.join(format!("{name}.json"))));
        }
    }

    /// Factory presets plus the user folder.
    pub(crate) fn num_presets(&self) -> usize {
        num_factory_presets() + self.st.user_presets.len()
    }

    fn preset_name(&self, index: usize) -> &str {
        match index.checked_sub(num_factory_presets()) {
            None => FACTORY[index].name,
            Some(u) => self.st.user_presets.get(u).map_or("", |p| p.name.as_str()),
        }
    }

    pub(crate) fn refresh_user_presets(&mut self) {
        self.st.user_presets = presets::list_user(&self.st.presets_dir);
    }

    /// Sends a preset's values to the host and shows its name.
    pub(crate) fn load_preset(&mut self, index: usize) {
        let loaded = match index.checked_sub(num_factory_presets()) {
            None => factory_values(index).map(|v| (FACTORY[index].name.to_owned(), v)),
            Some(u) => self.st.user_presets.get(u).and_then(|p| presets::load_user(&p.path)),
        };
        let Some((name, values)) = loaded else {
            self.st.message = format!("CANNOT LOAD {}", self.preset_name(index).to_uppercase());
            self.refresh_user_presets();
            return;
        };
        self.send_all(&values);
        self.st.shared.set_preset_name(&name);
        self.st.preset = Some(index);
        self.st.message.clear();
    }

    /// The values as the editor shows them, so pending edits are included.
    fn current_values(&mut self) -> [f32; NUM_PARAMS] {
        let mut v = [0.0; NUM_PARAMS];
        for id in ParamId::all() {
            v[id.index()] = self.plain(id);
        }
        v
    }

    /// Writes the current settings to `path` and selects the new preset.
    pub(crate) fn save_preset(&mut self, path: &Path) {
        let values = self.current_values();
        match presets::save_user(path, &values) {
            Ok((written, name)) => {
                self.st.shared.set_preset_name(&name);
                self.refresh_user_presets();
                self.st.preset =
                    self.st.user_presets.iter().position(|p| p.path == written).map(|u| u + num_factory_presets());
                self.st.message = format!("SAVED {}", name.to_uppercase());
            }
            Err(e) => self.st.message = format!("CANNOT SAVE: {e}").to_uppercase(),
        }
    }

    fn poll_save_job(&mut self) {
        let Some(job) = self.st.save_job.clone() else {
            return;
        };
        let Some(result) = job.lock().ok().and_then(|mut j| j.take()) else {
            return;
        };
        self.st.save_job = None;
        match result {
            Ok(path) if !path.is_empty() => self.save_preset(Path::new(&path)),
            Ok(_) => {}
            Err(e) => self.st.message = e,
        }
    }

    fn theme_selector(&mut self) {
        let cv = self.cv;
        let current = self.p.theme.load(Relaxed) as usize;
        let widths: Vec<f32> = THEMES.iter().map(|t| 14.0 + self.text_w(t.name, 11.0)).collect();
        let total: f32 = widths.iter().map(|w| w + 5.0).sum::<f32>() - 5.0;
        let mut x = BW - 14.0 - total;
        for (i, theme) in THEMES.iter().enumerate() {
            let w = widths[i];
            let rect = cv.r(x, 13.0, w, 18.0);
            let key = format!("theme:{}", theme.name);
            let resp = self.ui.interact(rect, Id::new(&key), Sense::click());
            self.register(&key, ControlKind::Button, rect);
            let (fill, ink) = if i == current {
                (self.th.sublabel, self.th.bar_text)
            } else if resp.hovered() {
                (self.th.button_hover, self.th.text)
            } else {
                (self.th.button, self.th.text)
            };
            self.pt.rect_filled(rect, cv.round(3.0), fill);
            self.text(rect.center(), Align2::CENTER_CENTER, theme.name, 11.0, ink);
            if resp.clicked() {
                self.p.theme.store(i as u8, Relaxed);
            }
            x += w + 5.0;
        }
    }

    /// Magnitude: one slider from dry to fully processed.
    fn magnitude_row(&mut self) {
        self.accent = self.th.sublabel;
        self.hslider(ParamId::Magnitude, 16.0, 49.0, 560.0, "MAGNITUDE", None);
    }

    /// Focused control's name and value, else the preset name or the last message.
    fn readout(&mut self) {
        let cv = self.cv;
        let rect = cv.r(592.0, 46.0, 632.0, 24.0);
        self.pt.rect_filled(rect, cv.round(3.0), self.th.oled_bg);
        self.pt.rect_stroke(rect, cv.round(3.0), Stroke::new(1.0 * cv.s, self.th.oled_frame), StrokeKind::Inside);
        let (name, value) = match &self.st.focus {
            Some(f) => (f.name.clone(), f.value.clone()),
            None if !self.st.message.is_empty() => ("PRESET".to_owned(), self.st.message.clone()),
            None => ("PRESET".to_owned(), self.st.shared.preset_name().to_uppercase()),
        };
        self.pt.text(cv.p(602.0, 58.0), Align2::LEFT_CENTER, &name, cv.mono(11.0), self.th.oled_dim);
        self.pt.text(cv.p(1214.0, 58.0), Align2::RIGHT_CENTER, &value, cv.mono(12.0), self.th.oled_ink);
    }

    // ------------------------------------------------------------ popups

    fn preset_columns(&self) -> usize {
        let user_cols = self.st.user_presets.len().div_ceil(PRESET_ROWS).max(1);
        (num_factory_presets().div_ceil(PRESET_ROWS) + user_cols).min(PRESET_MAX_COLS)
    }

    fn popup_rect(&self, pop: Popup) -> Rect {
        let (at, w, h) = match pop {
            Popup::Presets { at } => {
                let cols = self.preset_columns();
                (at, cols as f32 * PRESET_COL_W + 8.0, (PRESET_ROWS + 1) as f32 * POPUP_ROW + 10.0)
            }
            Popup::Choice { param, at } => (at, 170.0, param.desc().choices.len().max(1) as f32 * POPUP_ROW + 8.0),
        };
        let x = at.x.min(BW - w - 4.0);
        let y = at.y.min(BH - h - 4.0);
        self.cv.r(x, y, w, h)
    }

    fn popup(&mut self) {
        let Some(pop) = self.st.popup else {
            return;
        };
        let rect = self.popup_rect(pop);
        let cv = self.cv;
        self.pt.rect_filled(rect.translate(vec2(2.0, 3.0) * cv.s), cv.round(4.0), Color32::from_black_alpha(140));
        self.pt.rect_filled(rect, cv.round(4.0), self.th.oled_bg);
        self.pt.rect_stroke(rect, cv.round(4.0), Stroke::new(1.0 * cv.s, self.th.oled_frame), StrokeKind::Inside);
        let origin = cv.to_canvas(rect.min) + vec2(4.0, 4.0);
        let mut close = false;

        match pop {
            Popup::Presets { .. } => {
                let factory = num_factory_presets();
                let factory_cols = factory.div_ceil(PRESET_ROWS);
                for col in 0..self.preset_columns() {
                    let x = origin.x + col as f32 * PRESET_COL_W;
                    let (first, last, title) = if col < factory_cols {
                        (col * PRESET_ROWS, factory, (col == 0).then_some("FACTORY"))
                    } else {
                        let c = col - factory_cols;
                        (factory + c * PRESET_ROWS, self.num_presets(), (c == 0).then_some("USER"))
                    };
                    if let Some(title) = title {
                        self.pt.text(cv.p(x + 5.0, origin.y + 8.0), Align2::LEFT_CENTER, title, cv.mono(11.0), self.th.oled_dim);
                    }
                    if title == Some("USER") && first == last {
                        let hint = "SAVE TO ADD ONE";
                        self.pt.text(cv.p(x + 5.0, origin.y + 8.0 + POPUP_ROW), Align2::LEFT_CENTER, hint, cv.mono(11.0), self.th.oled_dim);
                    }
                    for i in first..last.min(first + PRESET_ROWS) {
                        let label = format!("{:02}  {}", i + 1, self.preset_name(i).to_uppercase());
                        let at = pos2(x, origin.y + POPUP_ROW + (i - first) as f32 * POPUP_ROW);
                        if self.popup_item(&format!("preset:{i}"), at, PRESET_COL_W - 8.0, &label, self.st.preset == Some(i)) {
                            self.load_preset(i);
                            close = true;
                        }
                    }
                }
            }
            Popup::Choice { param, .. } => {
                let d = param.desc();
                let current = self.plain(param).round() as usize;
                for (i, name) in d.choices.iter().enumerate() {
                    let key = format!("choice:{}:{i}", d.id_string);
                    let at = origin + vec2(0.0, i as f32 * POPUP_ROW);
                    if self.popup_item(&key, at, 162.0, &name.to_uppercase(), current == i) {
                        self.change_plain(param, i as f32);
                        close = true;
                    }
                }
            }
        }

        // A click anywhere outside closes the popup.
        let outside = self
            .ui
            .input(|i| i.pointer.any_pressed() && i.pointer.interact_pos().is_some_and(|p| !rect.contains(p)));
        if close || outside {
            self.st.popup = None;
        }
    }

    fn popup_item(&mut self, key: &str, at: Pos2, w: f32, text: &str, selected: bool) -> bool {
        let cv = self.cv;
        let rect = cv.r(at.x, at.y, w, POPUP_ROW - 1.0);
        let resp = self.ui.interact(rect, Id::new(("popup", key)), Sense::click());
        self.register(key, ControlKind::Button, rect);
        if resp.hovered() || selected {
            let fill = if resp.hovered() { self.th.button_hover } else { self.th.button };
            self.pt.rect_filled(rect, cv.round(2.0), fill);
        }
        let ink = if selected { self.th.oled_ink } else { self.th.text };
        self.pt.text(rect.left_center() + vec2(5.0 * cv.s, 0.0), Align2::LEFT_CENTER, text, cv.mono(11.0), ink);
        resp.clicked()
    }
}

/// A preset name that is safe as a file stem.
fn file_stem_for(name: &str) -> String {
    let cleaned: String =
        name.chars().map(|c| if c == '/' || c == '\\' || c.is_control() { '-' } else { c }).take(64).collect();
    let cleaned = cleaned.trim().trim_start_matches('.').trim();
    if cleaned.is_empty() {
        "Preset".to_owned()
    } else {
        cleaned.to_owned()
    }
}

#[cfg(test)]
#[path = "../editor_tests.rs"]
mod tests;
