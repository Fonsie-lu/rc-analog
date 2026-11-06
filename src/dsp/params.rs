//! Single source of truth for the parameter set. The engine's flat parameter
//! array, the nih-plug parameter layer (`crate::params`), presets and the
//! editor are all generated from this table, so the layers cannot drift apart.
//!
//! Values are in natural units everywhere except at the host boundary. The
//! `id` strings (`"wobble.amount"`) are the stable automation and preset keys:
//! never rename one.

use std::collections::HashMap;
use std::sync::OnceLock;

/// Every parameter, in table order. The order of the builder calls in
/// [`build`] must match; a debug assertion catches drift.
#[repr(usize)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum ParamId {
    // ---- Global -------------------------------------------------------------
    Magnitude,

    // ---- Noise ----------------------------------------------------------------
    NoiseOn,
    NoiseAmount,
    NoiseType,
    NoiseTone,
    NoiseFollow,
    NoiseDuck,
    NoiseRouting,
    NoiseFlux,

    // ---- Wobble ---------------------------------------------------------------
    WobbleOn,
    WobbleAmount,
    WobbleBalance,
    WobbleWowRate,
    WobbleFlutterRate,
    WobbleStereo,
    WobbleMix,
    WobbleFlux,

    // ---- Distort --------------------------------------------------------------
    DistortOn,
    DistortAmount,
    DistortType,
    DistortFocusLo,
    DistortFocusHi,
    DistortMix,
    DistortFlux,

    // ---- Digital --------------------------------------------------------------
    DigitalOn,
    DigitalAmount,
    DigitalBalance,
    DigitalSmooth,
    DigitalFocusLo,
    DigitalFocusHi,
    DigitalCut,
    DigitalMix,
    DigitalFlux,

    // ---- Space ----------------------------------------------------------------
    SpaceOn,
    SpaceAmount,
    SpaceType,
    SpaceDecay,
    SpacePreDelay,
    SpaceFocusLo,
    SpaceFocusHi,
    SpaceStereo,
    SpaceFlux,

    // ---- Loss -----------------------------------------------------------------
    LossOn,
    LossAmount,
    LossBalance,
    LossDropouts,
    LossGap,
    LossStereo,
    LossFlux,

    // ---- Master ---------------------------------------------------------------
    InGain,
    OutGain,
    Width,
    EqOn,
    LowCut,
    LowCutSlope,
    HighCut,
    HighCutSlope,
    ToneMode,
    Tone,
}

pub const NUM_PARAMS: usize = ParamId::Tone as usize + 1;

impl ParamId {
    #[inline(always)]
    pub fn index(self) -> usize {
        self as usize
    }

    /// The parameter at a table index. Panics when out of range.
    pub fn from_index(index: usize) -> ParamId {
        assert!(index < NUM_PARAMS, "parameter index {index} out of range");
        // SAFETY: `ParamId` is `repr(usize)` with contiguous discriminants
        // starting at 0, and `index` was checked against the count.
        unsafe { std::mem::transmute::<usize, ParamId>(index) }
    }

    pub fn desc(self) -> &'static ParamDesc {
        &table().descs[self as usize]
    }

    /// `None` when the ID string is unknown (older or newer preset).
    pub fn from_id(id: &str) -> Option<ParamId> {
        table().by_id.get(id).copied()
    }

    pub fn all() -> impl Iterator<Item = ParamId> {
        (0..NUM_PARAMS).map(ParamId::from_index)
    }
}

#[derive(Clone, Debug)]
pub struct ParamDesc {
    pub id: ParamId,
    /// Stable automation ID, e.g. `"wobble.amount"`.
    pub id_string: &'static str,
    /// Short display name.
    pub name: &'static str,
    /// Group label for the host.
    pub module: &'static str,
    pub min: f32,
    pub max: f32,
    pub default: f32,
    pub unit: &'static str,
    /// 0 = continuous, else the number of discrete values.
    pub steps: usize,
    /// Non-empty for enum-style parameters.
    pub choices: &'static [&'static str],
    /// 0 = linear range, else the value that sits at the middle of the
    /// normalised range.
    pub skew_centre: f32,
    pub bipolar: bool,
}

impl ParamDesc {
    pub fn clamp(&self, v: f32) -> f32 {
        v.clamp(self.min, self.max)
    }

    /// Exponent mapping a linear proportion onto the normalised range, as in
    /// `normalised = proportion ^ skew`. 1.0 for linear parameters.
    pub fn skew_factor(&self) -> f32 {
        if self.skew_centre <= 0.0 || self.steps > 0 {
            return 1.0;
        }
        let p = (self.skew_centre - self.min) / (self.max - self.min);
        if p <= 0.0 || p >= 1.0 {
            return 1.0;
        }
        0.5_f32.ln() / p.ln()
    }

    pub fn to_normalized(&self, v: f32) -> f32 {
        let p = ((self.clamp(v) - self.min) / (self.max - self.min)).clamp(0.0, 1.0);
        p.powf(self.skew_factor())
    }

    pub fn from_normalized(&self, n: f32) -> f32 {
        let p = n.clamp(0.0, 1.0).powf(1.0 / self.skew_factor());
        let v = self.min + p * (self.max - self.min);
        if self.steps > 0 {
            v.round()
        } else {
            v
        }
    }

    /// Display text in natural units, e.g. "350 ms", "Tape", "+3.0 dB".
    pub fn format(&self, v: f32) -> String {
        if !self.choices.is_empty() {
            let i = (v.round() as isize - self.min as isize).clamp(0, self.choices.len() as isize - 1);
            return self.choices[i as usize].to_owned();
        }
        let sign = if self.bipolar && v > 0.0 { "+" } else { "" };
        match self.unit {
            "s" if v < 1.0 => format!("{:.0} ms", v * 1000.0),
            "s" => format!("{v:.2} s"),
            "Hz" if v >= 1000.0 => format!("{:.1} kHz", v / 1000.0),
            "Hz" if v < 10.0 => format!("{v:.2} Hz"),
            "Hz" => format!("{v:.0} Hz"),
            "ms" => format!("{v:.0} ms"),
            "dB" => format!("{sign}{v:.1} dB"),
            // Unitless controls read as percentages.
            _ => format!("{sign}{:.0}%", v * 100.0),
        }
    }
}

pub const ON_OFF: &[&str] = &["Off", "On"];
pub const NOISE_TYPES: &[&str] =
    &["Vinyl", "Dust", "Tape Hiss", "Cassette", "VHS", "Hum 50", "Hum 60", "Radio", "White", "Pink"];
pub const ROUTING: &[&str] = &["Pre", "Post"];
pub const STEREO: &[&str] = &["Mono", "Stereo"];
pub const DISTORT_TYPES: &[&str] = &["Tape", "Tube", "Cassette", "Transistor", "Fuzz"];
pub const SPACE_TYPES: &[&str] = &["Room", "Hall", "Plate", "Spring"];
pub const SLOPES: &[&str] = &["Soft", "Hard"];
pub const TONE_MODES: &[&str] = &["Tilt", "Mid"];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NoiseType {
    Vinyl,
    Dust,
    TapeHiss,
    Cassette,
    Vhs,
    Hum50,
    Hum60,
    Radio,
    White,
    Pink,
}

impl NoiseType {
    pub const ALL: [NoiseType; 10] = [
        NoiseType::Vinyl,
        NoiseType::Dust,
        NoiseType::TapeHiss,
        NoiseType::Cassette,
        NoiseType::Vhs,
        NoiseType::Hum50,
        NoiseType::Hum60,
        NoiseType::Radio,
        NoiseType::White,
        NoiseType::Pink,
    ];
    pub fn from_value(v: f32) -> Self {
        Self::ALL[(v.round().max(0.0) as usize).min(Self::ALL.len() - 1)]
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DistortType {
    Tape,
    Tube,
    Cassette,
    Transistor,
    Fuzz,
}

impl DistortType {
    pub const ALL: [DistortType; 5] =
        [DistortType::Tape, DistortType::Tube, DistortType::Cassette, DistortType::Transistor, DistortType::Fuzz];
    pub fn from_value(v: f32) -> Self {
        Self::ALL[(v.round().max(0.0) as usize).min(Self::ALL.len() - 1)]
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SpaceType {
    Room,
    Hall,
    Plate,
    Spring,
}

impl SpaceType {
    pub const ALL: [SpaceType; 4] = [SpaceType::Room, SpaceType::Hall, SpaceType::Plate, SpaceType::Spring];
    pub fn from_value(v: f32) -> Self {
        Self::ALL[(v.round().max(0.0) as usize).min(Self::ALL.len() - 1)]
    }
}

/// The six effect modules in signal order, as the editor lays them out.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Module {
    Noise = 0,
    Wobble,
    Distort,
    Digital,
    Space,
    Loss,
}

impl Module {
    pub const ALL: [Module; 6] = [Module::Noise, Module::Wobble, Module::Distort, Module::Digital, Module::Space, Module::Loss];

    pub fn name(self) -> &'static str {
        match self {
            Module::Noise => "NOISE",
            Module::Wobble => "WOBBLE",
            Module::Distort => "DISTORT",
            Module::Digital => "DIGITAL",
            Module::Space => "SPACE",
            Module::Loss => "LOSS",
        }
    }

    pub fn on(self) -> ParamId {
        match self {
            Module::Noise => ParamId::NoiseOn,
            Module::Wobble => ParamId::WobbleOn,
            Module::Distort => ParamId::DistortOn,
            Module::Digital => ParamId::DigitalOn,
            Module::Space => ParamId::SpaceOn,
            Module::Loss => ParamId::LossOn,
        }
    }

    /// The module's big knob, which Magnitude scales.
    pub fn amount(self) -> ParamId {
        ParamId::from_index(self.on().index() + 1)
    }

    pub fn flux(self) -> ParamId {
        match self {
            Module::Noise => ParamId::NoiseFlux,
            Module::Wobble => ParamId::WobbleFlux,
            Module::Distort => ParamId::DistortFlux,
            Module::Digital => ParamId::DigitalFlux,
            Module::Space => ParamId::SpaceFlux,
            Module::Loss => ParamId::LossFlux,
        }
    }
}

// ---- Table ---------------------------------------------------------------------

struct Table {
    descs: Vec<ParamDesc>,
    by_id: HashMap<&'static str, ParamId>,
}

fn table() -> &'static Table {
    static TABLE: OnceLock<Table> = OnceLock::new();
    TABLE.get_or_init(|| {
        let descs = build();
        debug_assert_eq!(descs.len(), NUM_PARAMS);
        let by_id = descs.iter().map(|d| (d.id_string, d.id)).collect();
        Table { descs, by_id }
    })
}

/// All descriptors in table order.
pub fn descs() -> &'static [ParamDesc] {
    &table().descs
}

pub fn default_value(id: ParamId) -> f32 {
    id.desc().default
}

/// Default values for every parameter, in table order.
pub fn defaults() -> [f32; NUM_PARAMS] {
    std::array::from_fn(|i| descs()[i].default)
}

struct Builder {
    descs: Vec<ParamDesc>,
    module: &'static str,
}

impl Builder {
    #[allow(clippy::too_many_arguments)]
    fn add(
        &mut self,
        id: ParamId,
        id_string: &'static str,
        name: &'static str,
        min: f32,
        max: f32,
        default: f32,
        unit: &'static str,
        steps: usize,
        choices: &'static [&'static str],
        skew_centre: f32,
        bipolar: bool,
    ) {
        debug_assert_eq!(id as usize, self.descs.len(), "params.rs builder order must match ParamId");
        self.descs.push(ParamDesc {
            id,
            id_string,
            name,
            module: self.module,
            min,
            max,
            default,
            unit,
            steps,
            choices,
            skew_centre,
            bipolar,
        });
    }

    /// Continuous 0 .. 1, shown as a percentage.
    fn unit(&mut self, id: ParamId, s: &'static str, name: &'static str, def: f32) {
        self.add(id, s, name, 0.0, 1.0, def, "", 0, &[], 0.0, false);
    }

    /// Continuous -1 .. 1, centred.
    fn bipolar_unit(&mut self, id: ParamId, s: &'static str, name: &'static str) {
        self.add(id, s, name, -1.0, 1.0, 0.0, "", 0, &[], 0.0, true);
    }

    #[allow(clippy::too_many_arguments)]
    fn skewed(&mut self, id: ParamId, s: &'static str, name: &'static str, lo: f32, hi: f32, def: f32, unit: &'static str, centre: f32) {
        self.add(id, s, name, lo, hi, def, unit, 0, &[], centre, false);
    }

    fn gain(&mut self, id: ParamId, s: &'static str, name: &'static str) {
        self.add(id, s, name, -24.0, 24.0, 0.0, "dB", 0, &[], 0.0, true);
    }

    fn choice(&mut self, id: ParamId, s: &'static str, name: &'static str, def: f32, choices: &'static [&'static str]) {
        let n = choices.len();
        self.add(id, s, name, 0.0, (n - 1) as f32, def, "", n, choices, 0.0, false);
    }

    /// Focus band edges: the frequency range a module works on.
    fn focus(&mut self, lo: ParamId, s_lo: &'static str, hi: ParamId, s_hi: &'static str, def_lo: f32, def_hi: f32) {
        self.skewed(lo, s_lo, "Focus Low", 20.0, 20_000.0, def_lo, "Hz", 640.0);
        self.skewed(hi, s_hi, "Focus High", 20.0, 20_000.0, def_hi, "Hz", 640.0);
    }
}

fn build() -> Vec<ParamDesc> {
    use ParamId::*;
    let mut b = Builder { descs: Vec::with_capacity(NUM_PARAMS), module: "Global" };

    b.unit(Magnitude, "global.magnitude", "Magnitude", 1.0);

    b.module = "Noise";
    b.choice(NoiseOn, "noise.on", "On", 1.0, ON_OFF);
    b.unit(NoiseAmount, "noise.amount", "Volume", 0.2);
    b.choice(NoiseType, "noise.type", "Type", 0.0, NOISE_TYPES);
    b.bipolar_unit(NoiseTone, "noise.tone", "Tone");
    b.unit(NoiseFollow, "noise.follow", "Follow", 0.0);
    b.unit(NoiseDuck, "noise.duck", "Duck", 0.0);
    b.choice(NoiseRouting, "noise.routing", "Routing", 0.0, ROUTING);
    b.unit(NoiseFlux, "noise.flux", "Flux", 0.2);

    b.module = "Wobble";
    b.choice(WobbleOn, "wobble.on", "On", 1.0, ON_OFF);
    b.unit(WobbleAmount, "wobble.amount", "Depth", 0.2);
    b.unit(WobbleBalance, "wobble.balance", "Wow/Flutter", 0.3);
    b.skewed(WobbleWowRate, "wobble.wowRate", "Wow Rate", 0.1, 4.0, 0.55, "Hz", 0.8);
    b.skewed(WobbleFlutterRate, "wobble.flutterRate", "Flutter Rate", 4.0, 25.0, 9.0, "Hz", 10.0);
    b.choice(WobbleStereo, "wobble.stereo", "Stereo", 0.0, STEREO);
    b.unit(WobbleMix, "wobble.mix", "Mix", 1.0);
    b.unit(WobbleFlux, "wobble.flux", "Flux", 0.3);

    b.module = "Distort";
    b.choice(DistortOn, "distort.on", "On", 1.0, ON_OFF);
    b.unit(DistortAmount, "distort.amount", "Drive", 0.25);
    b.choice(DistortType, "distort.type", "Type", 0.0, DISTORT_TYPES);
    b.focus(DistortFocusLo, "distort.focusLo", DistortFocusHi, "distort.focusHi", 20.0, 20_000.0);
    b.unit(DistortMix, "distort.mix", "Mix", 1.0);
    b.unit(DistortFlux, "distort.flux", "Flux", 0.1);

    b.module = "Digital";
    b.choice(DigitalOn, "digital.on", "On", 1.0, ON_OFF);
    b.unit(DigitalAmount, "digital.amount", "Crush", 0.0);
    b.unit(DigitalBalance, "digital.balance", "Rate/Bits", 0.5);
    b.unit(DigitalSmooth, "digital.smooth", "Smooth", 0.3);
    b.focus(DigitalFocusLo, "digital.focusLo", DigitalFocusHi, "digital.focusHi", 20.0, 20_000.0);
    b.choice(DigitalCut, "digital.cut", "Cut", 0.0, ON_OFF);
    b.unit(DigitalMix, "digital.mix", "Mix", 1.0);
    b.unit(DigitalFlux, "digital.flux", "Flux", 0.0);

    b.module = "Space";
    b.choice(SpaceOn, "space.on", "On", 1.0, ON_OFF);
    b.unit(SpaceAmount, "space.amount", "Wet", 0.0);
    b.choice(SpaceType, "space.type", "Type", 0.0, SPACE_TYPES);
    b.skewed(SpaceDecay, "space.decay", "Decay", 0.1, 20.0, 1.2, "s", 2.0);
    b.skewed(SpacePreDelay, "space.preDelay", "Pre Delay", 0.0, 300.0, 10.0, "ms", 60.0);
    b.focus(SpaceFocusLo, "space.focusLo", SpaceFocusHi, "space.focusHi", 120.0, 9_000.0);
    b.choice(SpaceStereo, "space.stereo", "Stereo", 1.0, STEREO);
    b.unit(SpaceFlux, "space.flux", "Flux", 0.2);

    b.module = "Loss";
    b.choice(LossOn, "loss.on", "On", 1.0, ON_OFF);
    b.unit(LossAmount, "loss.amount", "Amount", 0.15);
    b.unit(LossBalance, "loss.balance", "Wear/Flutter", 0.3);
    b.unit(LossDropouts, "loss.dropouts", "Dropouts", 0.2);
    b.unit(LossGap, "loss.gap", "Gap", 0.3);
    b.choice(LossStereo, "loss.stereo", "Stereo", 0.0, STEREO);
    b.unit(LossFlux, "loss.flux", "Flux", 0.2);

    b.module = "Master";
    b.gain(InGain, "master.inGain", "Input");
    b.gain(OutGain, "master.outGain", "Output");
    b.add(Width, "master.width", "Width", 0.0, 2.0, 1.0, "", 0, &[], 0.0, false);
    b.choice(EqOn, "master.eqOn", "EQ", 1.0, ON_OFF);
    b.skewed(LowCut, "master.lowCut", "Low Cut", 20.0, 2_000.0, 20.0, "Hz", 150.0);
    b.choice(LowCutSlope, "master.lowCutSlope", "Low Slope", 0.0, SLOPES);
    b.skewed(HighCut, "master.highCut", "High Cut", 500.0, 20_000.0, 20_000.0, "Hz", 5_000.0);
    b.choice(HighCutSlope, "master.highCutSlope", "High Slope", 0.0, SLOPES);
    b.choice(ToneMode, "master.toneMode", "Tone Mode", 0.0, TONE_MODES);
    b.bipolar_unit(Tone, "master.tone", "Tone");

    b.descs
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn table_matches_enum_and_ids_are_unique() {
        assert_eq!(descs().len(), NUM_PARAMS);
        for (i, d) in descs().iter().enumerate() {
            assert_eq!(d.id as usize, i);
            assert_eq!(ParamId::from_id(d.id_string), Some(d.id));
            assert!(d.default >= d.min && d.default <= d.max, "{} default", d.id_string);
            if !d.choices.is_empty() {
                assert_eq!(d.choices.len(), d.steps, "{}", d.id_string);
            }
        }
        assert_eq!(ParamId::from_id("wobble.amount"), Some(ParamId::WobbleAmount));
        assert_eq!(ParamId::from_id("nope"), None);
    }

    #[test]
    fn module_blocks_start_with_on_then_amount() {
        for m in Module::ALL {
            assert_eq!(m.on().desc().name, "On");
            assert!(m.amount().desc().id_string.ends_with(".amount"), "{:?}", m);
            assert!(m.flux().desc().id_string.ends_with(".flux"), "{:?}", m);
        }
    }

    #[test]
    fn normalised_mapping_round_trips_and_honours_skew_centre() {
        for d in descs() {
            for k in 0..=20 {
                let n = k as f32 / 20.0;
                let v = d.from_normalized(n);
                let back = d.to_normalized(v);
                let again = d.from_normalized(back);
                assert!((again - v).abs() <= 1.0e-3 * (d.max - d.min).max(1.0), "{}", d.id_string);
            }
        }
        let focus = ParamId::DistortFocusLo.desc();
        assert!((focus.from_normalized(0.5) - 640.0).abs() < 1.0);
    }

    #[test]
    fn formats_read_naturally() {
        assert_eq!(ParamId::SpaceDecay.desc().format(0.25), "250 ms");
        assert_eq!(ParamId::SpaceDecay.desc().format(2.5), "2.50 s");
        assert_eq!(ParamId::HighCut.desc().format(8000.0), "8.0 kHz");
        assert_eq!(ParamId::InGain.desc().format(3.0), "+3.0 dB");
        assert_eq!(ParamId::Width.desc().format(1.5), "150%");
        assert_eq!(ParamId::DistortType.desc().format(3.0), "Transistor");
        assert_eq!(ParamId::Tone.desc().format(0.5), "+50%");
    }
}
