# SEG-Y viewer

Read-only SEG-Y (rev 0/1) viewer in Rust + egui, focused on **drawing**: wiggle, image (variable density) and overlay. No header editing, no writing, no processing beyond display gain/AGC.

## Build and run on a new computer
Requirements: Rust stable (>= 1.80) via [rustup](https://rustup.rs), a C linker (`build-essential`), and on Linux the GUI libs (`libx11-dev libxkbcommon-dev libwayland-dev libgl1-mesa-dev libvulkan-dev`). For videos: `ffmpeg`.
```bash
git clone https://github.com/totosugito/research-segy-viewer.git
cd research-segy-viewer
cargo build --release -p app
./target/release/app file.sgy           # GUI (no file: open dialog)
./target/release/app file.sgy --demo    # live captioned demo in the real window
./gate.sh                               # fmt + clippy -D warnings + all tests
cargo bench -p render                   # performance (release profile)
```
Only source is in git. Not committed (recreate locally): `target/` (build output), `.toolchain/` (optional project-local Rust; `gate.sh` uses it if present, otherwise system cargo), `refs/` (demo videos, reference captures), `*.sgy` data.

**Sample data:** no SEG-Y file is included. Some tests/demos look for `/home/toto/Downloads/3X_75_PR.SGY` and **silently skip** when it is missing (see ISSUES.md #2), so on another machine a green gate does not exercise the file-based tests. Put any 2D/3D SEG-Y at that path or edit the `SAMPLE_SEGY_PATH`/`DEFAULT_SGY` constants.

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
