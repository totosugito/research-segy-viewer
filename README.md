# SEG-Y viewer

Read-only SEG-Y (rev 0/1) viewer in Rust + egui, focused on **drawing**: wiggle, image (variable density) and overlay. No header editing, no writing, no processing beyond display gain/AGC.

## Run
```bash
export PATH=$PWD/.toolchain/cargo/bin:$PATH RUSTUP_HOME=$PWD/.toolchain/rustup CARGO_HOME=$PWD/.toolchain/cargo   # project-local Rust
cargo run --release -p app -- file.sgy          # GUI (no file: open dialog)
cargo run --release -p app -- file.sgy --demo   # live captioned demo in the real window
./gate.sh                                       # fmt + clippy -D warnings + all tests
cargo bench -p render                           # performance (release profile)
```

## Features (status = what has been checked, see docs/TESTING.md)
| Area | Feature | Checked by |
|---|---|---|
| Read | EBCDIC text header, binary header, trace headers, formats 1/2/3/5/8, lazy trace index | unit tests, sample file |
| Plot type | Wiggle, Image, Overlay; wiggle fill pos/neg/both/line, excursion, auto-switch | unit + real-input test, demo |
| Amplitude | gain dB, clip (percentile/RMS/peak), per-trace/global, DC removal, AGC, trace balance, polarity | unit + demo checks |
| Colour | Grayscale, Seismic, Viridis, Rainbow, Brown-White-Black; reverse, contrast, bias | demo checks |
| View | wheel zoom, zoom box (drag), right/middle-drag pan, Fit All/H/V, trace step, reverse order | real-input test, demo |
| Selection | click selects trace (waveform highlight, headers, min/max/rms), arrows, Esc | real-input test |
| Axes | time axis, trace/header axis, up to 5 label rows with any header or custom byte | unit + real-input test |
| Output | Save PNG (offscreen, any size), read-only headers window | demo checks |

## Not done / not verified (see ISSUES.md)
- No comparison with the reference viewer (SeiSee) has been made yet; behaviour is from general seismic-viewer knowledge.
- Performance target (100k traces x 2000 samples, <16 ms/frame) is a target, not a measurement.
- Shift+wheel / Alt+wheel single-axis zoom untested with a real mouse.

## Demos
- `cargo run --release -p app --example primary_demo` → `refs/demo/primary/segy_viewer_primary.mp4` (real injected input, ~1 min).
- `cargo run --release -p app --example demo_video` → `refs/demo/segy_viewer_demo.mp4` (all features, ~2.5 min).
Both render offscreen; no window, no screen-capture permission needed.

## Layout
`crates/segy-core` (parser) → `crates/render` (pure drawing, no UI) → `crates/app` (controller + egui GUI, demos).
Rules: no `unwrap/expect/panic`, `unsafe` forbidden, file-derived numbers are hostile (see AGENTS.md).
