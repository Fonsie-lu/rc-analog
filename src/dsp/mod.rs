//! The effect engine: all DSP and state, independent of nih-plug and the
//! editor so it can be tested on its own. Nothing in here may depend on
//! `nih_plug`; the plugin layer in `crate::params` and `crate::lib` adapts it.

pub mod common;
pub mod digital;
pub mod distort;
pub mod engine;
pub mod filters;
pub mod loss;
pub mod noise;
pub mod params;
pub mod presets;
pub mod shared;
pub mod space;
pub mod wobble;
