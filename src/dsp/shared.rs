//! State shared between the audio thread and the editor: meters and module
//! activity as relaxed atomics (the audio thread writes, the editor reads),
//! and the name of the loaded preset (main thread only).

use atomic_float::AtomicF32;
use std::sync::atomic::Ordering::Relaxed;
use std::sync::Mutex;

use super::params::Module;

#[derive(Default)]
pub struct Feedback {
    /// Peak since the editor last looked, per channel. The editor swaps them to 0.
    pub in_peak: [AtomicF32; 2],
    pub out_peak: [AtomicF32; 2],
    /// Per module, 0 .. 1, in [`Module`] order: what the module is doing now.
    pub activity: [AtomicF32; 6],
    /// The digital module's effective sample rate (Hz) and bit depth.
    pub digital_rate: AtomicF32,
    pub digital_bits: AtomicF32,
    /// Fraction of the block's real-time budget spent processing, smoothed.
    pub cpu_load: AtomicF32,
}

impl Feedback {
    /// Raises a "peak since last read" meter (audio thread, single writer).
    pub fn raise(meter: &AtomicF32, peak: f32) {
        if peak > meter.load(Relaxed) {
            meter.store(peak, Relaxed);
        }
    }

    /// Reads and clears a peak meter (editor).
    pub fn take(meter: &AtomicF32) -> f32 {
        meter.swap(0.0, Relaxed)
    }

    pub fn activity(&self, m: Module) -> f32 {
        self.activity[m as usize].load(Relaxed)
    }
}

pub struct Shared {
    pub feedback: Feedback,
    preset_name: Mutex<String>,
}

impl Default for Shared {
    fn default() -> Self {
        Self { feedback: Feedback::default(), preset_name: Mutex::new("Init".to_owned()) }
    }
}

impl Shared {
    pub fn preset_name(&self) -> String {
        self.preset_name.lock().map(|n| n.clone()).unwrap_or_default()
    }

    /// Main thread / editor only, never the audio thread.
    pub fn set_preset_name(&self, name: &str) {
        if let Ok(mut n) = self.preset_name.lock() {
            name.clone_into(&mut n);
        }
    }
}
