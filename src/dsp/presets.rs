//! Presets: the factory list (built in code as overrides on the defaults),
//! the preset file format and the user preset folder.
//!
//! A preset file is JSON: `{"product": "rc-21", "schema": 1, "name": ...,
//! "params": {"<id_string>": value, ...}}` with values in natural units.
//! Unknown IDs are ignored and missing ones take their defaults, so presets
//! survive parameters being added. User presets are the `.json` files in
//! `$XDG_DATA_HOME/rc-21/presets`; a preset's name is its file stem.

use std::path::{Path, PathBuf};

use serde_json::{json, Map, Value};

use super::params::{defaults, ParamId, NUM_PARAMS};

pub const PRODUCT: &str = "rc-21";
pub const SCHEMA: u64 = 1;

pub struct FactoryPreset {
    pub name: &'static str,
    /// Overrides on top of the parameter defaults, by ID string.
    pub values: &'static [(&'static str, f32)],
}

/// Every module's big knob at 0: a dry starting point.
const ZERO: [(&str, f32); 6] = [
    ("noise.amount", 0.0),
    ("wobble.amount", 0.0),
    ("distort.amount", 0.0),
    ("digital.amount", 0.0),
    ("space.amount", 0.0),
    ("loss.amount", 0.0),
];

pub static FACTORY: &[FactoryPreset] = &[
    FactoryPreset { name: "Init", values: &[] },
    FactoryPreset { name: "Clean Slate", values: &ZERO },
    FactoryPreset {
        name: "Subtle Warmth",
        values: &[
            ("noise.type", 2.0),
            ("noise.amount", 0.1),
            ("wobble.amount", 0.12),
            ("distort.amount", 0.3),
            ("loss.amount", 0.1),
            ("loss.gap", 0.15),
            ("master.tone", -0.1),
        ],
    },
    FactoryPreset {
        name: "Dusty Vinyl",
        values: &[
            ("noise.type", 0.0),
            ("noise.amount", 0.45),
            ("noise.tone", -0.3),
            ("wobble.amount", 0.25),
            ("wobble.balance", 0.1),
            ("wobble.wowRate", 0.55),
            ("distort.type", 1.0),
            ("distort.amount", 0.2),
            ("loss.amount", 0.0),
            ("master.lowCut", 40.0),
            ("master.highCut", 9_000.0),
        ],
    },
    FactoryPreset {
        name: "Worn Cassette",
        values: &[
            ("noise.type", 3.0),
            ("noise.amount", 0.3),
            ("wobble.amount", 0.4),
            ("wobble.balance", 0.45),
            ("wobble.flux", 0.5),
            ("distort.type", 2.0),
            ("distort.amount", 0.45),
            ("loss.amount", 0.45),
            ("loss.dropouts", 0.3),
            ("loss.gap", 0.45),
            ("master.highCut", 11_000.0),
        ],
    },
    FactoryPreset {
        name: "VHS Memories",
        values: &[
            ("noise.type", 4.0),
            ("noise.amount", 0.3),
            ("wobble.amount", 0.5),
            ("wobble.balance", 0.2),
            ("wobble.wowRate", 0.3),
            ("wobble.stereo", 1.0),
            ("distort.amount", 0.3),
            ("loss.amount", 0.35),
            ("loss.gap", 0.5),
            ("loss.stereo", 1.0),
            ("master.width", 0.8),
        ],
    },
    FactoryPreset {
        name: "Lo-Fi Beats",
        values: &[
            ("noise.type", 0.0),
            ("noise.amount", 0.3),
            ("noise.duck", 0.6),
            ("wobble.amount", 0.3),
            ("distort.amount", 0.35),
            ("digital.amount", 0.35),
            ("digital.balance", 0.6),
            ("digital.smooth", 0.4),
            ("master.lowCut", 60.0),
            ("master.highCut", 8_000.0),
            ("master.tone", -0.2),
        ],
    },
    FactoryPreset {
        name: "Sampler 12-Bit",
        values: &[
            ("noise.amount", 0.0),
            ("wobble.amount", 0.0),
            ("distort.type", 3.0),
            ("distort.amount", 0.2),
            ("digital.amount", 0.4),
            ("digital.balance", 0.55),
            ("digital.smooth", 0.6),
            ("loss.amount", 0.0),
        ],
    },
    FactoryPreset {
        name: "Crushed Drums",
        values: &[
            ("noise.amount", 0.0),
            ("wobble.amount", 0.0),
            ("distort.type", 4.0),
            ("distort.amount", 0.4),
            ("distort.focusLo", 200.0),
            ("distort.focusHi", 8_000.0),
            ("distort.mix", 0.5),
            ("digital.amount", 0.75),
            ("digital.balance", 0.4),
            ("digital.focusLo", 1_000.0),
            ("digital.mix", 0.6),
            ("loss.amount", 0.0),
        ],
    },
    FactoryPreset {
        name: "Dreamy Chorus",
        values: &[
            ("noise.amount", 0.0),
            ("wobble.amount", 0.55),
            ("wobble.balance", 0.1),
            ("wobble.wowRate", 0.8),
            ("wobble.stereo", 1.0),
            ("wobble.mix", 0.5),
            ("distort.amount", 0.1),
            ("space.type", 1.0),
            ("space.amount", 0.3),
            ("space.decay", 3.5),
            ("loss.amount", 0.0),
        ],
    },
    FactoryPreset {
        name: "Spring Tank",
        values: &[
            ("noise.amount", 0.05),
            ("wobble.amount", 0.05),
            ("distort.type", 1.0),
            ("distort.amount", 0.25),
            ("space.type", 3.0),
            ("space.amount", 0.4),
            ("space.decay", 2.2),
            ("space.preDelay", 0.0),
            ("loss.amount", 0.0),
        ],
    },
    FactoryPreset {
        name: "Haunted Hall",
        values: &[
            ("noise.type", 2.0),
            ("noise.amount", 0.15),
            ("noise.routing", 1.0),
            ("wobble.amount", 0.4),
            ("wobble.balance", 0.15),
            ("space.type", 1.0),
            ("space.amount", 0.55),
            ("space.decay", 8.0),
            ("space.preDelay", 40.0),
            ("space.flux", 0.6),
            ("master.tone", -0.4),
        ],
    },
    FactoryPreset {
        name: "Drunk Tape",
        values: &[
            ("wobble.amount", 0.9),
            ("wobble.balance", 0.2),
            ("wobble.wowRate", 0.4),
            ("wobble.flux", 0.9),
            ("loss.amount", 0.4),
            ("loss.dropouts", 0.2),
        ],
    },
    FactoryPreset {
        name: "Tube Glow",
        values: &[
            ("noise.amount", 0.05),
            ("wobble.amount", 0.05),
            ("distort.type", 1.0),
            ("distort.amount", 0.45),
            ("distort.mix", 0.8),
            ("loss.amount", 0.05),
            ("master.tone", 0.1),
        ],
    },
    FactoryPreset {
        name: "Mains Hum",
        values: &[
            ("noise.type", 5.0),
            ("noise.amount", 0.35),
            ("noise.follow", 0.3),
            ("distort.type", 3.0),
            ("distort.amount", 0.2),
        ],
    },
    FactoryPreset {
        name: "AM Radio",
        values: &[
            ("noise.type", 7.0),
            ("noise.amount", 0.35),
            ("distort.type", 3.0),
            ("distort.amount", 0.5),
            ("distort.focusLo", 300.0),
            ("distort.focusHi", 4_000.0),
            ("digital.amount", 0.2),
            ("digital.smooth", 0.7),
            ("master.lowCut", 300.0),
            ("master.lowCutSlope", 1.0),
            ("master.highCut", 3_500.0),
            ("master.highCutSlope", 1.0),
            ("master.toneMode", 1.0),
            ("master.tone", 0.6),
            ("master.width", 0.0),
        ],
    },
    FactoryPreset {
        name: "Telephone",
        values: &[
            ("noise.type", 8.0),
            ("noise.amount", 0.1),
            ("wobble.amount", 0.0),
            ("distort.type", 3.0),
            ("distort.amount", 0.4),
            ("digital.amount", 0.3),
            ("loss.amount", 0.0),
            ("master.lowCut", 400.0),
            ("master.lowCutSlope", 1.0),
            ("master.highCut", 3_000.0),
            ("master.highCutSlope", 1.0),
            ("master.width", 0.0),
        ],
    },
    FactoryPreset {
        name: "Broken Speaker",
        values: &[
            ("noise.amount", 0.1),
            ("distort.type", 4.0),
            ("distort.amount", 0.7),
            ("distort.focusLo", 500.0),
            ("distort.focusHi", 6_000.0),
            ("loss.amount", 0.6),
            ("loss.dropouts", 0.8),
            ("loss.stereo", 1.0),
            ("master.toneMode", 1.0),
            ("master.tone", 0.7),
        ],
    },
    FactoryPreset {
        name: "Wide Air",
        values: &[
            ("noise.amount", 0.0),
            ("wobble.amount", 0.2),
            ("wobble.stereo", 1.0),
            ("wobble.mix", 0.5),
            ("distort.amount", 0.1),
            ("space.type", 2.0),
            ("space.amount", 0.25),
            ("space.decay", 1.5),
            ("loss.amount", 0.0),
            ("master.width", 1.4),
            ("master.tone", 0.3),
        ],
    },
    FactoryPreset {
        name: "Old Film",
        values: &[
            ("noise.type", 1.0),
            ("noise.amount", 0.4),
            ("noise.routing", 1.0),
            ("wobble.amount", 0.3),
            ("wobble.balance", 0.6),
            ("loss.amount", 0.5),
            ("loss.balance", 0.6),
            ("loss.gap", 0.6),
            ("master.highCut", 6_000.0),
            ("master.width", 0.0),
        ],
    },
];

pub fn num_factory_presets() -> usize {
    FACTORY.len()
}

/// Parameter values of factory preset `index`.
pub fn factory_values(index: usize) -> Option<[f32; NUM_PARAMS]> {
    let preset = FACTORY.get(index)?;
    let mut v = defaults();
    for (id, value) in preset.values {
        if let Some(p) = ParamId::from_id(id) {
            v[p.index()] = p.desc().clamp(*value);
        }
    }
    Some(v)
}

pub fn to_json(name: &str, values: &[f32; NUM_PARAMS]) -> String {
    let mut params = Map::new();
    for id in ParamId::all() {
        params.insert(id.desc().id_string.to_owned(), json!(values[id.index()]));
    }
    let doc = json!({ "product": PRODUCT, "schema": SCHEMA, "name": name, "params": Value::Object(params) });
    serde_json::to_string_pretty(&doc).unwrap_or_default()
}

/// Name and values of a preset file, or `None` if it is not one.
pub fn from_json(text: &str) -> Option<(String, [f32; NUM_PARAMS])> {
    let doc: Value = serde_json::from_str(text).ok()?;
    if doc.get("product").and_then(Value::as_str) != Some(PRODUCT) {
        return None;
    }
    let params = doc.get("params")?.as_object()?;
    let mut v = defaults();
    for (key, value) in params {
        if let (Some(id), Some(x)) = (ParamId::from_id(key), value.as_f64()) {
            v[id.index()] = id.desc().clamp(x as f32);
        }
    }
    let name = doc.get("name").and_then(Value::as_str).unwrap_or("Preset").to_owned();
    Some((name, v))
}

// ---- User presets -----------------------------------------------------------

#[derive(Clone, Debug, PartialEq)]
pub struct UserPreset {
    pub name: String,
    pub path: PathBuf,
}

/// `$XDG_DATA_HOME/rc-21/presets`, or `~/.local/share/rc-21/presets`.
pub fn user_dir() -> PathBuf {
    let base = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))
        .unwrap_or_else(|| PathBuf::from("."));
    base.join(PRODUCT).join("presets")
}

/// The `.json` files in `dir`, sorted by name. A missing folder is empty.
pub fn list_user(dir: &Path) -> Vec<UserPreset> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut out: Vec<UserPreset> = entries
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.is_file() && p.extension().is_some_and(|x| x.eq_ignore_ascii_case("json")))
        .filter_map(|path| Some(UserPreset { name: path.file_stem()?.to_string_lossy().into_owned(), path }))
        .collect();
    out.sort_by_key(|p| p.name.to_lowercase());
    out
}

/// Values of a preset file; its name is the file stem.
pub fn load_user(path: &Path) -> Option<(String, [f32; NUM_PARAMS])> {
    let text = std::fs::read_to_string(path).ok()?;
    let (_, values) = from_json(&text)?;
    let name = path.file_stem()?.to_string_lossy().into_owned();
    Some((name, values))
}

/// Writes a preset to `path` (".json" added when missing) through a
/// temporary file, so a failed write never leaves a truncated preset.
/// Returns the path written and the preset name (the file stem).
pub fn save_user(path: &Path, values: &[f32; NUM_PARAMS]) -> std::io::Result<(PathBuf, String)> {
    let path = if path.extension().is_some_and(|x| x.eq_ignore_ascii_case("json")) {
        path.to_path_buf()
    } else {
        path.with_extension("json")
    };
    let name = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "Preset".to_owned());
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = path.with_extension("json.tmp");
    std::fs::write(&tmp, to_json(&name, values))?;
    std::fs::rename(&tmp, &path)?;
    Ok((path, name))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn factory_presets_use_known_ids_and_unique_names() {
        let mut names = std::collections::HashSet::new();
        for p in FACTORY {
            assert!(names.insert(p.name), "duplicate {}", p.name);
            for (id, v) in p.values {
                let pid = ParamId::from_id(id).unwrap_or_else(|| panic!("{}: unknown id {id}", p.name));
                let d = pid.desc();
                assert!(*v >= d.min && *v <= d.max, "{}: {id} = {v} out of range", p.name);
            }
        }
        assert_eq!(factory_values(0), Some(defaults()));
        assert!(factory_values(FACTORY.len()).is_none());
    }

    #[test]
    fn json_round_trips_and_tolerates_unknown_and_missing_ids() {
        let v = factory_values(4).unwrap();
        let (name, back) = from_json(&to_json("Worn", &v)).unwrap();
        assert_eq!(name, "Worn");
        assert_eq!(back, v);

        let text = r#"{"product":"rc-21","schema":1,"name":"x","params":{"wobble.amount":0.7,"future.knob":3}}"#;
        let (_, v) = from_json(text).unwrap();
        assert_eq!(v[ParamId::WobbleAmount.index()], 0.7);
        assert_eq!(v[ParamId::NoiseAmount.index()], defaults()[ParamId::NoiseAmount.index()]);
        assert!(from_json(r#"{"product":"other","params":{}}"#).is_none());
        assert!(from_json("not json").is_none());
    }

    #[test]
    fn user_presets_save_list_and_load() {
        let dir = std::env::temp_dir().join(format!("rc21-presets-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        assert!(list_user(&dir).is_empty());
        let v = factory_values(7).unwrap();
        let (path, name) = save_user(&dir.join("b side"), &v).unwrap();
        assert_eq!(name, "b side");
        save_user(&dir.join("A Side.json"), &defaults()).unwrap();
        let list = list_user(&dir);
        assert_eq!(list.iter().map(|p| p.name.as_str()).collect::<Vec<_>>(), ["A Side", "b side"]);
        assert_eq!(load_user(&path), Some(("b side".to_owned(), v)));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
