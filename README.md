# RC-Analog

An analog colour effect for Linux, built with
[nih-plug](https://github.com/robbert-vdh/nih-plug). Exports **CLAP** and **VST3**.
Six modules (Noise, Wobble, Distort, Digital, Space, Loss) add the
imperfections of old gear: crackle and hiss, tape wow and flutter,
saturation, early-sampler grit, reverb, and the level and top-end loss of
worn tape. A "Flux" control in each module adds random, organic fluctuation.

The concept follows XLN Audio's RC-20 Retro Color (six effect modules, a big
knob each, a Flux control each, a global Magnitude slider, focus filters, a
master section). The DSP, the noise generators, the presets and the panel are
original. Nothing is sampled or copied.

| Item         | Value                                                   |
| ------------ | ------------------------------------------------------- |
| Product name | RC-Analog                                               |
| Vendor       | Fonsie Audio                                            |
| CLAP id      | `ch.fonsie.rc-analog`                                   |
| Features     | `audio-effect`, `stereo`, `mono`, `distortion`          |
| I/O          | stereo in/out, or mono in/out                           |
| Latency      | 23 samples (the distortion's 2x oversampler), fixed     |
| Parameters   | 59, grouped by module, natural ranges in the host       |
| GUI          | egui, 1240 x 760 design surface, freely resizable       |

## Signal flow

```
in gain -> NOISE (pre) -> WOBBLE -> DISTORT -> DIGITAL -> SPACE -> LOSS
        -> EQ (low cut, high cut, tone) -> NOISE (post) -> width -> out gain
```

**Magnitude** scales all six big knobs, the input and output gain and the EQ
at once. At 0 the plugin passes the signal through untouched (apart from
the fixed latency). A module at amount 0 is fully bypassed.

| Module  | Big knob | Controls | Flux |
| ------- | -------- | -------- | ---- |
| NOISE   | Volume | Type (Vinyl, Dust, Tape Hiss, Cassette, VHS, Hum 50/60, Radio, White, Pink), Tone (tilt), Follow (level tracks the input), Duck (pushed down by input peaks), Routing Pre/Post | level and crackle density wander |
| WOBBLE  | Depth (pitch deviation, up to ±2 % wow) | Wow/Flutter balance, wow and flutter rates, Mono/Stereo (stereo wow = chorus), Mix | irregular wow and flutter, wandering rates |
| DISTORT | Drive (0 - 36 dB) | Type (Tape, Tube, Cassette, Transistor, Fuzz), focus range, Mix | drive wobbles ±12 dB |
| DIGITAL | Crush | Rate/Bits balance (down to 750 Hz and 2.5 bits), Smooth, focus range, Full/Cut, Mix | clock jitter, wandering rate and depth, sputter past 50 % |
| SPACE   | Wet (equal-power dry/wet) | Type (Room, Hall, Plate, Spring), Decay (RT60), Pre Delay, focus range (input band and damping), Mono/Stereo | pre-delay drift, deeper tank modulation |
| LOSS    | Amount | Wear/Flutter balance, Dropouts, Gap (head spacing loss), Mono/Stereo | wear more erratic, flutter rate unsteady |

Details:

- The focus ranges split the signal into a 12 dB/oct band and the rest. The
  two parts add back up exactly, so only the band is processed. A switched-off
  or zero-amount module adds nothing.
- Distortion runs 2x oversampled. It always sits in the path (as a pure
  delay when off), so the reported latency never changes. An RMS follower
  levels the output, so Drive changes the character more than the loudness.
- Space is an 8-line feedback delay network with input diffusion and
  per-line damping. Spring adds a dispersive allpass chain.
- Loss closes its lowpass further whenever wear or a dropout pulls the level
  down, which is how spacing loss behaves on real tape.
- Noise fades out once the host transport is stopped and the input has been
  silent for 0.4 s.
- Each module header has a 5-segment strip showing what the module is doing
  right now (noise level, pitch deviation, gain reduction, crush depth,
  reverb level, level loss).

## Building

```sh
cargo xtask bundle rc_analog --release
./install.sh               # build, symlink into ~/.clap and ~/.vst3
./install.sh --copy        # copy instead of symlink
./install.sh --system      # copy to /usr/lib/clap and /usr/lib/vst3, via sudo
./install.sh --uninstall   # remove
```

`--clap` or `--vst3` limits the install to one format, and `--no-build`
installs the existing bundles. `install.sh` builds for `x86-64-v3`;
`TARGET_CPU= ./install.sh` builds for any x86_64.

`vendor/vst3-com` is nih-plug's `vst3-com` with one macro fixed. It is wired
in by `[patch]` in `Cargo.toml` and can go once upstream has the fix.

## Development

```sh
cargo test                                   # DSP, headless editor, allocation test
cargo clippy --all-targets
cargo run --release --example bench          # CPU per case, % of one core
cargo run --release --example gui_preview    # editor in a window, no host
RC_ANALOG_DEMO=1 RC_ANALOG_PRESET=4 RC_ANALOG_THEME=3 cargo run --release --example gui_preview
```

With every module on (Hall reverb), the chain costs about 1.5 % of one core
at 48 kHz on a Ryzen 5 PRO 3500U.

## Interface

- Knobs and sliders: drag (Shift for fine), mouse wheel; double-click resets.
  A hovered knob shows its value in place of its name, and the readout next
  to Magnitude shows the focused control.
- Focus ranges: press near a handle and drag. Handles never cross.
  Double-click resets both.
- A module's power square toggles it. A switched-off module is hatched, and
  clicking the hatch switches it back on.
- Header: preset browser (factory and user presets), SAVE (zenity dialog;
  presets saved to `$XDG_DATA_HOME/rc-analog/presets` show up in the list),
  CPU, themes (Tokyo Night, Catppuccin, Dracula, Gruvbox; saved with the
  project).

## Licence

GPL-3.0-or-later, forced by nih-plug's VST3 export. Copyright (C) 2026 Beat
Weber. Full text in [LICENSE](LICENSE).
