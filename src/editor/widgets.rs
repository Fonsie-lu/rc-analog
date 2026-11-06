//! Canvas mapping, parameter I/O and the controls: knobs, horizontal sliders,
//! the focus range, cell lists, selectors, buttons, module headers and meters.

use nih_plug::prelude::*;
use nih_plug_egui::egui::{
    pos2, vec2, Align2, Color32, CornerRadius, FontFamily, FontId, Id, Painter, Pos2, Rect, Response, Sense, Shape,
    Stroke, StrokeKind, Ui,
};
use std::f32::consts::PI;

use super::theme::{Section, Theme};
use super::{ControlInfo, ControlKind, EditorState, Pending, Popup, PENDING_TIMEOUT};
use crate::dsp::params::{ParamId, NUM_PARAMS};
use crate::params::RcAnalogParams;

/// Smallest text size in canvas pixels; smaller requests are raised to it.
pub const MIN_FONT: f32 = 11.0;

/// Maps canvas coordinates to screen coordinates.
#[derive(Clone, Copy)]
pub struct Cv {
    pub o: Pos2,
    pub s: f32,
}

impl Cv {
    pub fn p(&self, x: f32, y: f32) -> Pos2 {
        pos2(self.o.x + x * self.s, self.o.y + y * self.s)
    }
    pub fn r(&self, x: f32, y: f32, w: f32, h: f32) -> Rect {
        Rect::from_min_size(self.p(x, y), vec2(w * self.s, h * self.s))
    }
    pub fn rc(&self, cx: f32, cy: f32, w: f32, h: f32) -> Rect {
        self.r(cx - w * 0.5, cy - h * 0.5, w, h)
    }
    pub fn font(&self, size: f32) -> FontId {
        FontId::new(size.max(MIN_FONT) * self.s, FontFamily::Name("panel".into()))
    }
    pub fn mono(&self, size: f32) -> FontId {
        FontId::new(size.max(MIN_FONT) * self.s, FontFamily::Name("oled".into()))
    }
    pub fn stroke(&self, w: f32, c: Color32) -> Stroke {
        Stroke::new(w * self.s, c)
    }
    pub fn round(&self, r: f32) -> CornerRadius {
        CornerRadius::same((r * self.s) as u8)
    }
    /// Screen position back to canvas coordinates.
    pub fn to_canvas(&self, p: Pos2) -> Pos2 {
        pos2((p.x - self.o.x) / self.s, (p.y - self.o.y) / self.s)
    }
}

/// Vertical drag across a knob's full range, in canvas pixels.
pub const KNOB_TRAVEL: f32 = 160.0;

/// Lowest and highest frequency of the focus range sliders.
const RANGE_LO: f32 = 20.0;
const RANGE_HI: f32 = 20_000.0;

pub struct G<'a, 'b> {
    pub ui: &'a Ui,
    pub pt: Painter,
    pub cv: Cv,
    pub set: &'a ParamSetter<'b>,
    pub p: &'a RcAnalogParams,
    pub st: &'a mut EditorState,
    pub th: &'static Theme,
    /// Accent of the section currently being drawn.
    pub accent: Color32,
}

impl G<'_, '_> {
    // ------------------------------------------------------------ text

    pub fn text(&self, pos: Pos2, align: Align2, text: &str, size: f32, color: Color32) {
        self.pt.text(pos, align, text, self.cv.font(size), color);
    }

    /// Width of `text` in the panel font, in canvas pixels.
    pub fn text_w(&self, text: &str, size: f32) -> f32 {
        let font = self.cv.font(size);
        self.ui.fonts(|f| f.layout_no_wrap(text.to_owned(), font, Color32::WHITE).size().x) / self.cv.s
    }

    pub fn section(&mut self, s: Section) {
        self.accent = self.th.accents[s as usize];
    }

    // ------------------------------------------------------- parameter I/O

    pub fn register(&mut self, name: &str, kind: ControlKind, rect: Rect) {
        self.st.controls.push(ControlInfo { name: name.to_owned(), kind, rect });
    }

    pub fn now(&self) -> f64 {
        self.ui.input(|i| i.time)
    }

    /// The parameter's actual normalised value (from the preview store in the
    /// standalone preview and the tests).
    pub fn actual(&self, id: ParamId) -> f32 {
        let hp = self.p.get(id);
        if let Some(store) = &self.st.preview_store {
            if let Some(v) = store.lock().unwrap().get(&hp.ptr()) {
                return *v;
            }
        }
        hp.normalized()
    }

    /// The value to display: a value the GUI just sent wins until the host
    /// has applied it. nih-plug applies GUI changes asynchronously, so reading
    /// the parameter right after setting it returns the old value.
    pub fn shown(&mut self, id: ParamId) -> f32 {
        let actual = self.actual(id);
        let now = self.now();
        match self.st.pending.get(&id) {
            Some(p) if (p.value - actual).abs() > 1e-5 && now - p.time < PENDING_TIMEOUT => p.value,
            Some(_) => {
                self.st.pending.remove(&id);
                actual
            }
            None => actual,
        }
    }

    /// Displayed value in natural units.
    pub fn plain(&mut self, id: ParamId) -> f32 {
        let n = self.shown(id);
        id.desc().from_normalized(n)
    }

    pub fn is_on(&mut self, id: ParamId) -> bool {
        self.plain(id) > 0.5
    }

    /// Send a normalised value (snapped to the parameter's steps) and remember
    /// it as pending. Must sit between `begin` and `end`.
    pub fn send(&mut self, id: ParamId, normalized: f32) {
        let d = id.desc();
        let v = d.to_normalized(d.from_normalized(normalized.clamp(0.0, 1.0)));
        if (v - self.shown(id)).abs() < 1e-6 {
            return;
        }
        self.p.get(id).set(self.set, v);
        let time = self.now();
        self.st.pending.insert(id, Pending { value: v, time });
    }

    pub fn begin(&self, id: ParamId) {
        self.p.get(id).begin(self.set);
    }

    pub fn end(&self, id: ParamId) {
        self.p.get(id).end(self.set);
    }

    /// A complete single-step change (begin, set, end).
    pub fn change(&mut self, id: ParamId, normalized: f32) {
        self.begin(id);
        self.send(id, normalized);
        self.end(id);
    }

    /// Single-step change in natural units.
    pub fn change_plain(&mut self, id: ParamId, value: f32) {
        let n = id.desc().to_normalized(value);
        self.change(id, n);
    }

    /// Every parameter at once (a preset), each as its own gesture.
    pub fn send_all(&mut self, values: &[f32; NUM_PARAMS]) {
        for id in ParamId::all() {
            self.change_plain(id, values[id.index()]);
        }
    }

    pub fn value_text(&mut self, id: ParamId) -> String {
        let v = self.plain(id);
        id.desc().format(v)
    }

    pub fn set_focus(&mut self, name: &str, value: String) {
        self.st.focus = Some(super::Focus { name: name.to_uppercase(), value: value.to_uppercase() });
    }

    fn focus_param(&mut self, id: ParamId) {
        let d = id.desc();
        let text = self.value_text(id);
        self.set_focus(&format!("{} {}", d.module, d.name), text);
    }

    // ----------------------------------------------------------- inputs

    /// Drag (Shift for fine), wheel, double-click to reset. The drag position
    /// lives in egui temp memory (start value plus movement) because the
    /// parameter itself only updates asynchronously.
    fn continuous_input(&mut self, id: ParamId, rect: Rect, travel: f32, horizontal: bool) -> Response {
        let eid = Id::new(("drag", id));
        let resp = self.ui.interact(rect, eid, Sense::click_and_drag());
        if resp.drag_started() {
            let start = self.shown(id);
            self.ui.data_mut(|d| d.insert_temp(eid, start));
            self.begin(id);
        }
        if resp.dragged() {
            let fine = self.ui.input(|i| i.modifiers.shift);
            let moved = if horizontal { resp.drag_delta().x } else { -resp.drag_delta().y };
            let delta = moved / (travel * self.cv.s) * if fine { 0.15 } else { 1.0 };
            let start = self.shown(id);
            let pos: f32 = self.ui.data(|d| d.get_temp(eid)).unwrap_or(start);
            let pos = (pos + delta).clamp(0.0, 1.0);
            self.ui.data_mut(|d| d.insert_temp(eid, pos));
            self.send(id, pos);
        }
        if resp.drag_stopped() {
            self.end(id);
        }
        if resp.double_clicked() {
            let def = self.p.get(id).default_normalized();
            self.change(id, def);
        }
        if resp.hovered() && !resp.dragged() {
            let scroll = self.ui.input(|i| i.raw_scroll_delta.y);
            if scroll != 0.0 {
                let steps = id.desc().steps;
                let step = if steps > 1 { 1.0 / (steps - 1) as f32 } else { 0.0 };
                let step = step.max(0.0015 * scroll.abs());
                let v = self.shown(id) + step * scroll.signum();
                self.change(id, v);
            }
        }
        if resp.hovered() || resp.dragged() {
            self.focus_param(id);
        }
        resp
    }

    // ----------------------------------------------------------- knobs

    /// Rotary knob: a 270° track with the value arc from the bottom left (from
    /// the top for bipolar parameters). The label turns into the value while
    /// the knob is hovered.
    pub fn knob(&mut self, id: ParamId, cx: f32, cy: f32, r: f32, label: &str) -> Response {
        let cv = self.cv;
        let s = cv.s;
        let label_size = if r > 30.0 { 12.5 } else { 11.0 };
        let rect = cv.rc(cx, cy + 8.0, 2.0 * r + 24.0, 2.0 * r + 34.0);
        self.register(id.desc().id_string, ControlKind::Knob, rect);
        let resp = self.continuous_input(id, rect, KNOB_TRAVEL, false);
        let active = resp.hovered() || resp.dragged();
        let v = self.shown(id);
        let text = if active { self.value_text(id) } else { label.to_owned() };

        let c = cv.p(cx, cy);
        let at = |t: f32, radius: f32| {
            let a = (0.75 + 1.5 * t) * PI;
            c + vec2(a.cos(), a.sin()) * radius * s
        };
        let arc = |a: f32, b: f32, radius: f32, width: f32, color: Color32| {
            let (a, b) = (a.min(b), a.max(b));
            let n = ((b - a) * 48.0).ceil().max(1.0) as usize;
            let pts: Vec<Pos2> = (0..=n).map(|k| at(a + (b - a) * k as f32 / n as f32, radius)).collect();
            self.pt.add(Shape::line(pts, Stroke::new(width * s, color)));
        };
        let track = if r > 30.0 { 4.0 } else { 2.5 };
        let origin = if id.desc().bipolar { 0.5 } else { 0.0 };
        arc(0.0, 1.0, r + track + 1.5, track, self.th.slot);
        if (v - origin).abs() > 1e-4 {
            arc(origin, v, r + track + 1.5, track, self.accent);
        }
        self.pt.circle_filled(c + vec2(1.5, 3.0) * s, r * s, Color32::from_black_alpha(140));
        self.pt.circle_filled(c, r * s, self.th.cap_bottom);
        self.pt.circle_filled(c - vec2(0.0, 1.0) * s, (r - 2.0) * s, self.th.cap);
        let ink = if resp.dragged() { self.th.drag_line } else { self.th.text };
        self.pt.line_segment([at(v, r * 0.3), at(v, r * 0.85)], Stroke::new(if r > 30.0 { 3.0 } else { 2.2 } * s, ink));
        let color = if active { self.accent } else { self.th.text };
        self.text(cv.p(cx, cy + r + track + 14.0), Align2::CENTER_CENTER, &text, label_size, color);
        resp
    }

    // ---------------------------------------------------------- sliders

    /// Horizontal slider, 18 px high: drag sideways (Shift for fine), wheel,
    /// double-click resets. `ends` names the two extremes of a balance
    /// slider; otherwise `label` sits on the left and the value on the right.
    pub fn hslider(&mut self, id: ParamId, x: f32, y: f32, w: f32, label: &str, ends: Option<(&str, &str)>) {
        let cv = self.cv;
        let rect = cv.r(x, y, w, 18.0);
        self.register(id.desc().id_string, ControlKind::Slider, rect);
        let resp = self.continuous_input(id, rect, w, true);
        let active = resp.hovered() || resp.dragged();
        let v = self.shown(id);
        self.pt.rect_filled(rect, cv.round(3.0), self.th.slot);
        let fill_w = (w - 4.0) * v;
        if fill_w > 0.5 {
            self.pt
                .rect_filled(cv.r(x + 2.0, y + 2.0, fill_w, 14.0), cv.round(2.0), self.accent.gamma_multiply(0.35));
        }
        let tx = x + 2.0 + fill_w;
        self.pt.rect_filled(cv.r(tx - 1.5, y + 1.0, 3.0, 16.0), cv.round(1.0), self.accent);
        self.pt.rect_stroke(
            rect,
            cv.round(3.0),
            Stroke::new(1.0 * cv.s, if active { self.accent } else { self.th.scale }),
            StrokeKind::Inside,
        );
        let ink = self.th.text;
        match ends {
            Some((a, b)) => {
                if active {
                    let value = self.value_text(id);
                    self.text(cv.p(x + w * 0.5, y + 9.5), Align2::CENTER_CENTER, &value, 11.0, ink);
                } else {
                    self.text(cv.p(x + 7.0, y + 9.5), Align2::LEFT_CENTER, a, 11.0, ink);
                    self.text(cv.p(x + w - 7.0, y + 9.5), Align2::RIGHT_CENTER, b, 11.0, ink);
                }
            }
            None => {
                self.text(cv.p(x + 7.0, y + 9.5), Align2::LEFT_CENTER, label, 11.0, ink);
                if active {
                    let value = self.value_text(id);
                    self.text(cv.p(x + w - 7.0, y + 9.5), Align2::RIGHT_CENTER, &value, 11.0, ink);
                }
            }
        }
    }

    /// Focus range: two handles on a 20 Hz - 20 kHz log axis. A press takes
    /// the nearer handle, which then follows the pointer and never crosses
    /// the other one. Double-click resets both.
    pub fn range(&mut self, lo: ParamId, hi: ParamId, x: f32, y: f32, w: f32) {
        let cv = self.cv;
        let track = cv.r(x, y, w, 18.0);
        let eid = Id::new(("range", lo));
        let resp = self.ui.interact(track, eid, Sense::click_and_drag());
        let span = (RANGE_HI / RANGE_LO).ln();
        let t_of = |f: f32| ((f / RANGE_LO).ln() / span).clamp(0.0, 1.0);
        let f_of = |t: f32| RANGE_LO * (t.clamp(0.0, 1.0) * span).exp();
        let x_of = |t: f32| x + 4.0 + t * (w - 8.0);
        let (f_lo, f_hi) = (self.plain(lo), self.plain(hi));
        let (t_lo, t_hi) = (t_of(f_lo), t_of(f_hi));
        self.register(lo.desc().id_string, ControlKind::Handle, cv.rc(x_of(t_lo), y + 9.0, 10.0, 18.0));
        self.register(hi.desc().id_string, ControlKind::Handle, cv.rc(x_of(t_hi), y + 9.0, 10.0, 18.0));

        let pointer_t = |g: &Self| {
            g.ui.input(|i| i.pointer.interact_pos())
                .map(|p| (g.cv.to_canvas(p).x - x - 4.0) / (w - 8.0))
                .unwrap_or(0.0)
        };
        if resp.drag_started() {
            let origin = self.ui.input(|i| i.pointer.press_origin()).map(|p| self.cv.to_canvas(p).x).unwrap_or(0.0);
            let take_hi = (origin - x_of(t_hi)).abs() < (origin - x_of(t_lo)).abs()
                || (t_lo == t_hi && origin > x_of(t_hi));
            self.ui.data_mut(|d| d.insert_temp(eid, take_hi));
            self.begin(if take_hi { hi } else { lo });
        }
        let dragging_hi: Option<bool> = if resp.dragged() || resp.drag_stopped() { self.ui.data(|d| d.get_temp(eid)) } else { None };
        if resp.dragged() {
            let t = pointer_t(self);
            match dragging_hi {
                Some(true) => {
                    let f = f_of(t.max(t_lo));
                    self.send(hi, hi.desc().to_normalized(f));
                }
                Some(false) => {
                    let f = f_of(t.min(t_hi));
                    self.send(lo, lo.desc().to_normalized(f));
                }
                None => {}
            }
        }
        if resp.drag_stopped() {
            self.end(if dragging_hi == Some(true) { hi } else { lo });
        }
        if resp.double_clicked() {
            let (dl, dh) = (self.p.get(lo).default_normalized(), self.p.get(hi).default_normalized());
            self.change(lo, dl);
            self.change(hi, dh);
        }
        let active = resp.hovered() || resp.dragged();
        if active {
            let text = format!("{} - {}", lo.desc().format(f_lo), hi.desc().format(f_hi));
            let module = lo.desc().module;
            self.set_focus(&format!("{module} focus"), text);
        }

        self.pt.rect_filled(track, cv.round(3.0), self.th.slot);
        let (bx0, bx1) = (x_of(t_lo), x_of(t_hi));
        self.pt.rect_filled(
            Rect::from_two_pos(cv.p(bx0, y + 2.0), cv.p(bx1, y + 16.0)),
            cv.round(2.0),
            self.accent.gamma_multiply(0.35),
        );
        // Decade ticks: 100 Hz, 1 kHz, 10 kHz.
        for f in [100.0, 1_000.0, 10_000.0] {
            let tx = x_of(t_of(f));
            self.pt.line_segment([cv.p(tx, y + 13.0), cv.p(tx, y + 17.0)], cv.stroke(1.0, self.th.scale));
        }
        for bx in [bx0, bx1] {
            self.pt.rect_filled(cv.r(bx - 2.0, y, 4.0, 18.0), cv.round(1.5), self.accent);
        }
        self.pt.rect_stroke(
            track,
            cv.round(3.0),
            Stroke::new(1.0 * cv.s, if active { self.accent } else { self.th.scale }),
            StrokeKind::Inside,
        );
        let short = |f: f32| if f >= 1_000.0 { format!("{:.1}k", f / 1_000.0) } else { format!("{f:.0}") };
        self.text(cv.p(x, y - 3.0), Align2::LEFT_BOTTOM, "FOCUS", 11.0, self.th.legend);
        self.text(cv.p(x + w, y - 3.0), Align2::RIGHT_BOTTOM, &format!("{} - {} HZ", short(f_lo), short(f_hi)), 11.0, self.th.legend);
    }

    // ----------------------------------------------------------- choices

    /// Choice parameter as a row of cells; clicking a cell selects it.
    pub fn segments(&mut self, id: ParamId, x: f32, y: f32, w: f32, labels: &[&str]) {
        let cv = self.cv;
        let d = id.desc();
        let count = d.steps.max(2);
        let cell_w = w / count as f32;
        self.register(d.id_string, ControlKind::List, cv.r(x, y, w, 20.0));
        let current = (self.plain(id) - d.min).round() as usize;
        for i in 0..count {
            let rect = cv.r(x + i as f32 * cell_w, y, cell_w, 20.0).shrink(1.0 * cv.s);
            let key = format!("{}#{i}", d.id_string);
            let resp = self.ui.interact(rect, Id::new(&key), Sense::click());
            self.register(&key, ControlKind::List, rect);
            if resp.clicked() {
                self.change_plain(id, d.min + i as f32);
            }
            if resp.hovered() {
                self.set_focus(&format!("{} {}", d.module, d.name), d.format(d.min + i as f32));
            }
            let selected = i == current;
            let fill = if selected { self.th.slot } else { self.th.slot.gamma_multiply(0.45) };
            self.pt.rect_filled(rect, cv.round(2.0), fill);
            if selected || resp.hovered() {
                let edge = if selected { self.accent } else { self.th.scale };
                self.pt.rect_stroke(rect, cv.round(2.0), Stroke::new(1.0 * cv.s, edge), StrokeKind::Inside);
            }
            let ink = if selected { self.accent } else { self.th.step_off };
            let label = labels.get(i).copied().unwrap_or("");
            self.text(rect.center(), Align2::CENTER_CENTER, label, 11.0, ink);
        }
    }

    /// Box showing a choice's value with arrows either side. Arrows and the
    /// wheel step; clicking the name opens the full list.
    pub fn selector(&mut self, id: ParamId, x: f32, y: f32, w: f32) {
        let cv = self.cv;
        let d = id.desc();
        let rect = cv.r(x, y, w, 22.0);
        let eid = Id::new(("sel", id));
        let resp = self.ui.interact(rect.shrink2(vec2(16.0 * cv.s, 0.0)), eid, Sense::click());
        self.register(d.id_string, ControlKind::Selector, rect);
        let current = self.plain(id).round();
        let left = self.ui.interact(Rect::from_min_size(rect.min, vec2(16.0 * cv.s, rect.height())), eid.with("l"), Sense::click());
        let right = self.ui.interact(
            Rect::from_min_size(rect.right_top() - vec2(16.0 * cv.s, 0.0), vec2(16.0 * cv.s, rect.height())),
            eid.with("r"),
            Sense::click(),
        );
        let mut next = None;
        if left.clicked() {
            next = Some(current - 1.0);
        }
        if right.clicked() {
            next = Some(current + 1.0);
        }
        if resp.clicked() {
            self.st.popup = Some(Popup::Choice { param: id, at: cv.to_canvas(rect.left_bottom()) });
        }
        let hovered = resp.hovered() || left.hovered() || right.hovered();
        if hovered {
            let scroll = self.ui.input(|i| i.raw_scroll_delta.y);
            if scroll != 0.0 {
                next = Some(current - scroll.signum());
            }
            self.focus_param(id);
        }
        if let Some(v) = next {
            self.change_plain(id, v.clamp(d.min, d.max));
        }
        self.pt.rect_filled(rect, cv.round(3.0), self.th.slot);
        self.pt.rect_stroke(
            rect,
            cv.round(3.0),
            Stroke::new(1.0 * cv.s, if hovered { self.accent } else { self.th.scale }),
            StrokeKind::Inside,
        );
        let text = self.value_text(id).to_uppercase();
        self.text(rect.center(), Align2::CENTER_CENTER, &text, 12.0, self.accent);
        for (cx, dir) in [(x + 8.0, -1.0), (x + w - 8.0, 1.0)] {
            let c = cv.p(cx, y + 11.0);
            self.pt.add(Shape::convex_polygon(
                vec![c + vec2(3.0 * dir, 0.0) * cv.s, c + vec2(-3.0 * dir, -3.5) * cv.s, c + vec2(-3.0 * dir, 3.5) * cv.s],
                self.th.legend,
                Stroke::NONE,
            ));
        }
    }

    // ----------------------------------------------------------- buttons

    #[allow(clippy::too_many_arguments)]
    pub fn button(&mut self, key: &str, x: f32, y: f32, w: f32, text: &str, active: bool, help: &str) -> Response {
        let cv = self.cv;
        let rect = cv.r(x, y, w, 20.0);
        let resp = self.ui.interact(rect, Id::new(("btn", key)), Sense::click());
        self.register(key, ControlKind::Button, rect);
        let down = resp.is_pointer_button_down_on();
        let face = if active {
            self.th.slot
        } else if down {
            self.th.button_down
        } else if resp.hovered() {
            self.th.button_hover
        } else {
            self.th.button
        };
        let rect = if down { rect.translate(vec2(0.0, cv.s)) } else { rect };
        self.pt.rect_filled(rect.translate(vec2(0.0, 2.0 * cv.s)), cv.round(2.0), Color32::from_black_alpha(140));
        self.pt.rect_filled(rect, cv.round(2.0), face);
        if active {
            self.pt.rect_stroke(rect, cv.round(2.0), Stroke::new(1.0 * cv.s, self.accent), StrokeKind::Inside);
        }
        let ink = if active { self.accent } else { self.th.text };
        self.text(rect.center(), Align2::CENTER_CENTER, text, 11.0, ink);
        if resp.hovered() && !help.is_empty() {
            self.set_focus(help, String::new());
        }
        resp
    }

    /// Section bar with an optional power square (the module's On switch)
    /// and an activity strip on the right.
    #[allow(clippy::too_many_arguments)]
    pub fn module_header(&mut self, on: Option<ParamId>, x0: f32, x1: f32, y: f32, title: &str, section: Section, activity: Option<f32>) {
        let cv = self.cv;
        let th = self.th;
        let bar = th.bars[section as usize];
        self.pt.rect_filled(cv.r(x0 + 1.5, y, x1 - x0 - 3.0, 24.0), cv.round(3.0), bar);
        let title_w = self.text_w(title, 15.0);
        let tx = if let Some(id) = on {
            let hit = cv.r(x0 + 4.0, y, 32.0 + title_w, 24.0);
            let resp = self.ui.interact(hit, Id::new(("power", id)), Sense::click());
            self.register(id.desc().id_string, ControlKind::Switch, hit);
            let lit = self.is_on(id);
            if resp.clicked() {
                self.change_plain(id, if lit { 0.0 } else { 1.0 });
            }
            if resp.hovered() {
                let v = self.value_text(id);
                self.set_focus(&format!("{} on", id.desc().module), v);
            }
            let square = cv.r(x0 + 9.0, y + 5.0, 14.0, 14.0);
            self.pt.rect_filled(square, cv.round(2.0), th.bar_text);
            if lit {
                self.pt.rect_filled(square.shrink(3.5 * cv.s), cv.round(1.0), th.led);
            } else {
                self.pt.rect_filled(square.shrink(2.0 * cv.s), cv.round(1.0), bar);
            }
            x0 + 32.0
        } else {
            x0 + 10.0
        };
        self.text(cv.p(tx, y + 12.5), Align2::LEFT_CENTER, title, 15.0, th.bar_text);
        if let Some(level) = activity {
            // Five-segment strip: what the module is doing right now.
            let n = 5;
            let lit = (level.clamp(0.0, 1.0) * n as f32).ceil() as usize;
            for k in 0..n {
                let r = cv.r(x1 - 12.0 - (n - k) as f32 * 7.0, y + 8.0, 5.0, 8.0);
                let fill = if k < lit { th.bar_text } else { th.bar_text.gamma_multiply(0.25) };
                self.pt.rect_filled(r, cv.round(1.0), fill);
            }
        }
    }

    /// Recessed area behind a group of controls.
    pub fn backdrop(&self, x0: f32, y0: f32, x1: f32, y1: f32) {
        let cv = self.cv;
        self.pt.rect_filled(cv.r(x0, y0, x1 - x0, y1 - y0), cv.round(4.0), self.th.slot.gamma_multiply(0.35));
    }

    /// Diagonal hatch over a switched-off module.
    pub fn hatch(&self, x0: f32, y0: f32, x1: f32, y1: f32) {
        let cv = self.cv;
        let clip = cv.r(x0, y0, x1 - x0, y1 - y0);
        let pt = self.pt.with_clip_rect(clip);
        pt.rect_filled(clip, cv.round(4.0), self.th.panel.gamma_multiply(0.55));
        let h = y1 - y0;
        let mut x = x0 - h;
        while x < x1 {
            pt.line_segment([cv.p(x, y1), cv.p(x + h, y0)], cv.stroke(5.0, self.th.hatch.gamma_multiply(0.8)));
            x += 18.0;
        }
    }

    /// Vertical peak meter, -60 .. +6 dBFS, with the 0 dB mark.
    pub fn meter(&self, x: f32, y: f32, w: f32, h: f32, level: f32) {
        let cv = self.cv;
        self.pt.rect_filled(cv.r(x, y, w, h), cv.round(2.0), self.th.slot);
        let db = 20.0 * level.max(1.0e-6).log10();
        let t_of = |db: f32| ((db + 60.0) / 66.0).clamp(0.0, 1.0);
        let t = t_of(db);
        if t > 0.0 {
            let zero = t_of(0.0);
            let top = y + h * (1.0 - t);
            let split = y + h * (1.0 - zero);
            self.pt.rect_filled(Rect::from_two_pos(cv.p(x + 1.0, top.max(split)), cv.p(x + w - 1.0, y + h - 1.0)), cv.round(1.5), self.th.led);
            if top < split {
                self.pt.rect_filled(Rect::from_two_pos(cv.p(x + 1.0, top), cv.p(x + w - 1.0, split)), cv.round(1.5), self.th.clip);
            }
        }
        let zero_y = y + h * (1.0 - t_of(0.0));
        self.pt.line_segment([cv.p(x - 2.0, zero_y), cv.p(x + w + 2.0, zero_y)], cv.stroke(1.0, self.th.scale));
    }
}
