//! The six module columns and the master strip.
//!
//! Every module column has the same rhythm: header (power square, title,
//! activity strip), the big knob, one row for the type or balance, a row of
//! small knobs, a row for the focus range or a switch, an optional extra row
//! and the Flux slider at the bottom. A switched-off module is covered by a
//! hatch; clicking the hatch switches it back on.

use nih_plug_egui::egui::{Align2, Id, Sense};
use std::sync::atomic::Ordering::Relaxed;

use super::theme::Section;
use super::widgets::G;
use super::{ControlKind, BW};
use crate::dsp::params::{Module, ParamId as P};
use crate::dsp::shared::Feedback;

/// Module columns span this range, six equal columns.
const MOD_X0: f32 = 16.0;
const MOD_X1: f32 = BW - 16.0;
const BAR_Y: f32 = 84.0;
const BODY_Y0: f32 = 112.0;
const BODY_Y1: f32 = 500.0;
const BIG_Y: f32 = 168.0;
const BIG_R: f32 = 36.0;
/// Type selector or balance slider.
const ROW_A: f32 = 246.0;
/// Small knobs (centre).
const KNOB_Y: f32 = 306.0;
const KNOB_R: f32 = 17.0;
/// Focus range or a switch.
const ROW_C: f32 = 384.0;
const ROW_D: f32 = 424.0;
const FLUX_Y: f32 = 468.0;

const MASTER_Y: f32 = 520.0;

/// Centres of `n` equal cells across `x0 .. x1`.
fn spread(x0: f32, x1: f32, n: usize) -> Vec<f32> {
    let step = (x1 - x0) / n as f32;
    (0..n).map(|i| x0 + (i as f32 + 0.5) * step).collect()
}

fn section(m: Module) -> Section {
    match m {
        Module::Noise => Section::Noise,
        Module::Wobble => Section::Wobble,
        Module::Distort => Section::Distort,
        Module::Digital => Section::Digital,
        Module::Space => Section::Space,
        Module::Loss => Section::Loss,
    }
}

impl G<'_, '_> {
    pub(crate) fn modules(&mut self) {
        let w = (MOD_X1 - MOD_X0) / 6.0;
        for (i, m) in Module::ALL.into_iter().enumerate() {
            let x0 = MOD_X0 + i as f32 * w;
            self.module(m, x0, x0 + w);
        }
    }

    fn module(&mut self, m: Module, x0: f32, x1: f32) {
        self.section(section(m));
        let activity = self.st.shared.feedback.activity(m);
        self.module_header(Some(m.on()), x0, x1, BAR_Y, m.name(), section(m), Some(activity));
        self.backdrop(x0 + 3.0, BODY_Y0, x1 - 3.0, BODY_Y1);
        let cx = (x0 + x1) * 0.5;
        let (l, r) = (x0 + 14.0, x1 - 14.0);
        let label = m.amount().desc().name.to_uppercase();
        self.knob(m.amount(), cx, BIG_Y, BIG_R, &label);

        match m {
            Module::Noise => {
                self.selector(P::NoiseType, l, ROW_A, r - l);
                let xs = spread(l, r, 3);
                self.knob(P::NoiseTone, xs[0], KNOB_Y, KNOB_R, "TONE");
                self.knob(P::NoiseFollow, xs[1], KNOB_Y, KNOB_R, "FOLLOW");
                self.knob(P::NoiseDuck, xs[2], KNOB_Y, KNOB_R, "DUCK");
                self.legend(l, ROW_C, "ROUTING");
                self.segments(P::NoiseRouting, l, ROW_C, r - l, &["PRE", "POST"]);
            }
            Module::Wobble => {
                self.hslider(P::WobbleBalance, l, ROW_A + 2.0, r - l, "", Some(("WOW", "FLUTTER")));
                let xs = spread(l, r, 3);
                self.knob(P::WobbleWowRate, xs[0], KNOB_Y, KNOB_R, "WOW HZ");
                self.knob(P::WobbleFlutterRate, xs[1], KNOB_Y, KNOB_R, "FLUT HZ");
                self.knob(P::WobbleMix, xs[2], KNOB_Y, KNOB_R, "MIX");
                self.segments(P::WobbleStereo, l, ROW_C, r - l, &["MONO", "STEREO"]);
            }
            Module::Distort => {
                self.selector(P::DistortType, l, ROW_A, r - l);
                self.knob(P::DistortMix, cx, KNOB_Y, KNOB_R, "MIX");
                self.range(P::DistortFocusLo, P::DistortFocusHi, l, ROW_C, r - l);
            }
            Module::Digital => {
                self.hslider(P::DigitalBalance, l, ROW_A + 2.0, r - l, "", Some(("RATE", "BITS")));
                let xs = spread(l, r, 2);
                self.knob(P::DigitalSmooth, xs[0], KNOB_Y, KNOB_R, "SMOOTH");
                self.knob(P::DigitalMix, xs[1], KNOB_Y, KNOB_R, "MIX");
                self.range(P::DigitalFocusLo, P::DigitalFocusHi, l, ROW_C, r - l);
                let half = (r - l) * 0.5;
                self.segments(P::DigitalCut, l, ROW_D, half, &["FULL", "CUT"]);
                self.digital_status(r, ROW_D + 10.0);
            }
            Module::Space => {
                self.selector(P::SpaceType, l, ROW_A, r - l);
                let xs = spread(l, r, 2);
                self.knob(P::SpaceDecay, xs[0], KNOB_Y, KNOB_R, "DECAY");
                self.knob(P::SpacePreDelay, xs[1], KNOB_Y, KNOB_R, "PRE DLY");
                self.range(P::SpaceFocusLo, P::SpaceFocusHi, l, ROW_C, r - l);
                self.segments(P::SpaceStereo, l, ROW_D, r - l, &["MONO", "STEREO"]);
            }
            Module::Loss => {
                self.hslider(P::LossBalance, l, ROW_A + 2.0, r - l, "", Some(("WEAR", "FLUTTER")));
                let xs = spread(l, r, 2);
                self.knob(P::LossDropouts, xs[0], KNOB_Y, KNOB_R, "DROPOUTS");
                self.knob(P::LossGap, xs[1], KNOB_Y, KNOB_R, "GAP");
                self.segments(P::LossStereo, l, ROW_C, r - l, &["MONO", "STEREO"]);
            }
        }
        // Flux: caption and value above, so the thumb never covers them.
        self.legend(l, FLUX_Y, "FLUX");
        let flux = self.value_text(m.flux());
        self.text(self.cv.p(r, FLUX_Y - 3.0), Align2::RIGHT_BOTTOM, &flux, 11.0, self.th.legend);
        self.hslider(m.flux(), l, FLUX_Y, r - l, "", None);

        if !self.is_on(m.on()) {
            self.hatch(x0 + 3.0, BODY_Y0, x1 - 3.0, BODY_Y1);
            let rect = self.cv.r(x0 + 3.0, BODY_Y0, x1 - x0 - 6.0, BODY_Y1 - BODY_Y0);
            let resp = self.ui.interact(rect, Id::new(("hatch", m as usize)), Sense::click());
            self.register(&format!("hatch:{}", m.name()), ControlKind::Button, rect);
            if resp.clicked() {
                self.change_plain(m.on(), 1.0);
            }
            if resp.hovered() {
                self.set_focus(&format!("{} is off", m.name()), "CLICK TO SWITCH ON".to_owned());
            }
            self.text(self.cv.p((x0 + x1) * 0.5, (BODY_Y0 + BODY_Y1) * 0.5), Align2::CENTER_CENTER, "OFF", 15.0, self.th.legend);
        }
    }

    /// Small caption above a row.
    fn legend(&self, x: f32, y: f32, text: &str) {
        self.text(self.cv.p(x, y - 3.0), Align2::LEFT_BOTTOM, text, 11.0, self.th.legend);
    }

    /// The digital module's effective rate and depth.
    fn digital_status(&self, right: f32, y: f32) {
        let fb = &self.st.shared.feedback;
        let rate = fb.digital_rate.load(Relaxed);
        let bits = fb.digital_bits.load(Relaxed);
        let text = if rate > 0.0 { format!("{:.1}K {:.0}B", rate / 1_000.0, bits.max(1.0)) } else { String::new() };
        self.text(self.cv.p(right, y), Align2::RIGHT_CENTER, &text, 11.0, self.th.legend);
    }

    // ------------------------------------------------------------ master

    pub(crate) fn master(&mut self) {
        let (in_x1, out_x0) = (216.0, 884.0);
        let y = MASTER_Y;
        let body = (y + 28.0, y + 226.0);

        // Input.
        self.section(Section::Master);
        self.module_header(None, MOD_X0, in_x1, y, "INPUT", Section::Master, None);
        self.backdrop(MOD_X0 + 3.0, body.0, in_x1 - 3.0, body.1);
        self.knob(P::InGain, 80.0, y + 100.0, 28.0, "GAIN");
        self.meters(150.0, y + 44.0, 0);

        // EQ.
        self.module_header(Some(P::EqOn), in_x1, out_x0, y, "EQ", Section::Master, None);
        self.backdrop(in_x1 + 3.0, body.0, out_x0 - 3.0, body.1);
        let xs = spread(in_x1, out_x0, 3);
        self.knob(P::LowCut, xs[0], y + 96.0, 26.0, "LOW CUT");
        self.segments(P::LowCutSlope, xs[0] - 60.0, y + 160.0, 120.0, &["SOFT", "HARD"]);
        self.knob(P::HighCut, xs[1], y + 96.0, 26.0, "HIGH CUT");
        self.segments(P::HighCutSlope, xs[1] - 60.0, y + 160.0, 120.0, &["SOFT", "HARD"]);
        self.knob(P::Tone, xs[2], y + 96.0, 26.0, "TONE");
        self.segments(P::ToneMode, xs[2] - 60.0, y + 160.0, 120.0, &["TILT", "MID"]);
        if !self.is_on(P::EqOn) {
            self.hatch(in_x1 + 3.0, body.0, out_x0 - 3.0, body.1);
        }

        // Output.
        self.module_header(None, out_x0, MOD_X1, y, "OUTPUT", Section::Master, None);
        self.backdrop(out_x0 + 3.0, body.0, MOD_X1 - 3.0, body.1);
        self.knob(P::OutGain, out_x0 + 70.0, y + 100.0, 28.0, "GAIN");
        self.knob(P::Width, out_x0 + 170.0, y + 100.0, 28.0, "WIDTH");
        self.meters(out_x0 + 262.0, y + 44.0, 2);
    }

    /// A pair of peak meters (L, R) with a legend; `first` indexes
    /// `EditorState::meters` (0 = input, 2 = output).
    fn meters(&mut self, x: f32, y: f32, first: usize) {
        let fb = &self.st.shared.feedback;
        let peaks = if first == 0 { &fb.in_peak } else { &fb.out_peak };
        let fresh = [Feedback::take(&peaks[0]), Feedback::take(&peaks[1])];
        for (shown, f) in self.st.meters[first..first + 2].iter_mut().zip(fresh) {
            // Instant rise, ~20 dB per second fall at 60 frames per second.
            *shown = f.max(*shown * 0.94);
        }
        let h = 150.0;
        for k in 0..2 {
            self.meter(x + k as f32 * 16.0, y, 12.0, h, self.st.meters[first + k]);
        }
        let label = if first == 0 { "IN" } else { "OUT" };
        self.text(self.cv.p(x + 14.0, y + h + 14.0), Align2::CENTER_CENTER, label, 11.0, self.th.legend);
    }
}
