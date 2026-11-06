# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

RC-Analog (vendor Fonsie Audio) is an analog colour effect plugin modelled on the concept of XLN Audio's RC-20 Retro Color: six modules (Noise, Wobble, Distort, Digital, Space, Loss), each with a big knob and a Flux control, a global Magnitude, and a master section. It exports CLAP and VST3 and is built on nih-plug with a custom-painted nih_plug_egui editor (egui 0.31). The structure, conventions and look follow the sibling synth `~/tools/double-slit`. `README.md` describes the modules and the signal flow. The repository is `Fonsie-lu/rc-20` (directory `rc-20-clone`). The product carries no XLN logo or branding, and its DSP, noise generators and presets are original.

## Commands

```sh
cargo test                                   # DSP + headless editor + tests/no_alloc.rs
cargo test --lib dsp::space                  # one module's tests
cargo test --lib editor::tests::focus        # editor tests by name filter
cargo clippy --all-targets                   # must stay warning-free
cargo run --release --example bench          # CPU cost per case, % of one core

cargo xtask bundle rc_analog --release     # -> target/bundled/RC-Analog.{clap,vst3}
./install.sh                                 # build, symlink into ~/.clap and ~/.vst3 (--help)

cargo run --release --example gui_preview    # editor without a host
RC_ANALOG_DEMO=1 RC_ANALOG_PRESET=4 RC_ANALOG_THEME=0..3 cargo run --release --example gui_preview
```

`cargo xtask` is an alias in `.cargo/config.toml`. `vendor/vst3-com` patches nih-plug's `vst3-com` (one macro semicolon) through `[patch]` in `Cargo.toml`. `[profile.test]` uses opt-level 2 because the tests render seconds of audio. There is no nih-plug standalone (no ALSA/JACK headers). Use `gui_preview` for the GUI: with `RC_ANALOG_DEMO=1` the engine processes a kick + chord test signal in a thread with no audio output, so the meters and activity strips move. Use the tests and the bench for audio.

Screenshot workflow: `(RC_ANALOG_DEMO=1 timeout 9 ./target/release/examples/gui_preview &)`, wait ~5 s, `import -window "RC-Analog Preview" out.png`, then Read the PNG. The window comes out at about 2x scale, so crop or resize with `magick`.

## Architecture

There are two layers with a hard boundary:

- **`src/dsp/`**: all DSP and state. **No nih-plug or egui imports**, so the engine can be tested on its own.
- **`src/lib.rs`, `src/params.rs`, `src/editor/`**: the plugin, the host parameters and the GUI.

### Parameters: `dsp/params.rs` is the single source of truth

`ParamId` (`repr(usize)`) plus the table built in `build()` drive the engine's flat `[f32; NUM_PARAMS]`, the host parameters (`src/params.rs` generates a `FloatParam` or `IntParam` per entry and implements `Params` by hand), the preset JSON and the editor. **The order of the builder calls must match the enum** (a debug assertion checks it). Each module block starts with `On` and then the big-knob amount, which is how `Module::on()`/`amount()` find them. Keep that order.

To add a parameter: enum entry → builder call in the same position → wire it in `Engine::update` → a control in `editor/panel.rs` (`every_parameter_has_a_control` fails otherwise). The `id_string`s (`"wobble.amount"`) are the stable automation and preset keys: never rename one.

### Engine (`dsp/engine.rs`)

Chain: in gain → noise (pre) → wobble → distort → digital → space → loss → EQ → noise (post) → width → out gain. `set_params` copies the host values once per block. `update` runs once per **chunk** (`common::CHUNK` = 32 samples): it smooths the continuous parameters, applies Magnitude (scales the six amounts, in/out gain dB and EQ wet) and hands each module a `*Params` struct. Chunks continue across host block boundaries (`chunk_left`), so renders are bit-identical for any block size (tested). Anything stateful that runs per chunk must live in a module's `set()`, never at the end of `process()`.

Every module has the same shape: `prepare(sr)` (allocates), `reset()`, `set(&params, n)` once per chunk (targets `Ramp`s over `n` samples), `process(l, r)` in place, `activity()` 0..1 for the editor. Rules every module keeps:

- `on == false` or amount 0 ⇒ an exact bypass, via `engage()` and a wet `Ramp` that becomes `is_silent()`. Tests check bit-exact passthrough.
- `FocusSplit` (`filters.rs`): band = 12 dB/oct bandpass, `rest = x - band`, so `band + rest == x` exactly. Process the band and add the rest back.
- Distort runs 2x oversampled (`Oversampler2x`, halfband FIR) and **always** delays by `LATENCY` (23 samples), even when off, so the latency reported in `initialize` stays constant. Any new latency source must keep the total fixed.
- Flux fluctuations come from `common::Drift` (seeded smoothed noise). Some drifts are stepped per sample and others per chunk; their `set_rate` must use the matching rate.

`dsp/shared.rs`: `Shared` holds the GUI feedback (peak meters as "max since the editor last read", which the editor clears with `Feedback::take`; per-module activity; the digital module's rate/bits; CPU) plus the preset name. The preset name is behind a `Mutex` that only the main thread and GUI touch. Host state saves the parameters, the editor size, the theme and `"preset-name"`.

Presets (`dsp/presets.rs`): factory presets are overrides on the defaults, by `id_string`. The file format is `{"product":"rc-analog","schema":1,"name","params":{id: value}}`; unknown IDs are ignored and missing ones take their defaults. User presets are files in `$XDG_DATA_HOME/rc-analog/presets`, named by file stem. The editor's SAVE runs `zenity` on a helper thread (`save_dialog`, polled each frame). Editor tests never run zenity: they set `presets_dir` to a temp folder and fill `save_job` themselves.

### Editor (`src/editor/`)

The editor is painted on a fixed 1240×760 canvas (`BW`/`BH`) through `Cv` and scaled to the window. `mod.rs` holds state, the header, the Magnitude row, the readout and the popups (preset list, choice list). `widgets.rs` holds `G`: param I/O, `knob`, `hslider`, `range` (dual-handle focus range on a log axis, registered per handle under each parameter), `segments`, `selector`, `module_header`, `hatch`, `meter`. `panel.rs` holds the six module columns (shared row constants `ROW_A`, `KNOB_Y`, `ROW_C`, `ROW_D`, `FLUX_Y`) and the master strip. A switched-off module gets a hatch whose click switches it back on.

- Parameter values reach the plugin asynchronously. Read through `G::shown`/`plain` (pending values win until the host applies them). Write only through `send` inside `begin`/`end`, or through `change`/`change_plain`.
- Text is never smaller than `MIN_FONT` (11 px). Check new labels for collisions in a screenshot; `G::text_w` measures.
- Never hard-code colours. Add a `Theme` field (`theme.rs`) and fill it for all four themes. `Section` indexes the six module colours plus Master.
- `src/editor_tests.rs` (compiled as `editor::tests` via `#[path]`) runs the real editor headless against a mock host that applies values late and checks gesture balance (`assert_clean`). Every control registers its rect in `EditorState::controls` under its parameter ID or a fixed key (`preset:3`, `choice:distort.type:4`, `hatch:SPACE`, `noise.routing#1`).

## Conventions

- No allocation, lock or libm `exp2`/`sin`/`tanh`/`powf` on the audio path. Use `fast_exp2`, `fast_sin01`, `fast_tanh`, `fast_pow`, `fast_db_to_gain`, `fast_tan_pi`, `one_pole_coeff` from `dsp/common.rs`. `tests/no_alloc.rs` renders every factory preset under nih-plug's aborting allocator.
- Randomness is seeded (`Rng`, `Drift`), never taken from the clock, and `reset()` re-seeds, so repeat renders are bit-identical.
- No vendor name or logo, and no XLN logo or branding, on the panel or in presets.
- Compatibility keys never change: `CLAP_ID` (`ch.fonsie.rc-analog`) and `VST3_CLASS_ID` in `lib.rs`, the parameter `id_string`s, choice orders (projects store the index; append new types at the end), and the preset JSON format.
