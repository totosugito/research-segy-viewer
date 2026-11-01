---
name: segy-viewer
description: Plan and build a SEG-Y viewer in Rust (egui) focused only on drawing seismic traces as wiggle and image (variable-density) displays, using SeiSee (under wine) as the visual reference via screen capture. Use when working in this repo on SEG-Y reading, wiggle/image rendering, or comparing output against SeiSee.
---

# SEG-Y viewer — wiggle / image only

## Scope (hard limits)
READ-ONLY SEG-Y (rev 0/1). Goal: the **richest and fastest drawing** of traces. Everything else (header editing, writing, processing, picking, 3D) is out; header reading is only what drawing needs.
IN, drawing features (maximize these):
- Modes: wiggle, image (VD), wiggle+image overlay; fill positive/negative/both, line-only, variable-area.
- Amplitude: gain (dB), percentile/RMS/absolute clip, per-trace vs global normalize, AGC, polarity flip, DC removal, trace balance, wiggle excursion scale/overlap.
- Color: several colormaps (gray, seismic, rainbow, viridis...), reverse, bias/center, clip on color, background/line/fill colors.
- Geometry: time/depth axis, trace-number / CDP / inline / offset header as horizontal axis (read from chosen header byte offset), pan, independent zoom H/V, fit, time window, trace skip/decimate, reverse direction, horizontal/vertical orientation.
- Overlays: axes + ticks, grid, header-value labels, cursor readout (trace, time, amplitude), minimap/overview, optional spectrum-free amplitude scale bar.
- Output: Save PNG at arbitrary resolution (offscreen).
- Navigation of large files: sort/group by header key only if it affects display order.

## Performance requirements (first-class, benchmark them)
- Memory-map; never load the whole file. Lazy trace index (constant-stride fast path when fixed length; offset table only for variable length).
- Decode only visible traces/samples; skip traces when zoomed out (min/max envelope per pixel column, not per trace point).
- Image mode: render straight into a pixel buffer at viewport size (never larger), colormap via 256/4096-entry LUT, parallelize over columns (rayon), SIMD-friendly f32 loops, no per-pixel allocation. Upload as one texture; re-render only on view/param change.
- Wiggle: per-trace min/max decimation to <=2 points/pixel vertically, scanline/polygon fill into the same buffer or batched mesh; no per-point egui shapes when dense.
- Cache: decoded-tile LRU keyed by (trace range, sample range, LOD); background decode threads; UI never blocks (progressive render: coarse LOD first, refine).
- Budgets: pan/zoom frame <16 ms on 100k traces x 2000 samples; open file <200 ms to first picture; steady memory < 500 MB. Add `criterion` benches and fail the gate on regressions >20%.

OUT: header editor, SEG-Y writer, processing/filters (beyond display gain/AGC), picking, 3D.

## Conventions (borrowed from gridcraft, https://github.com/storytold/gridcraft)
- Rust only, egui/eframe UI, workspace of small crates. Read gridcraft's `AGENTS.md` + `Cargo.toml` (https://github.com/storytold/gridcraft) for style: `#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic)]`, `unsafe_code = "forbid"`, errors via `Result` + `thiserror`, file-derived numbers are hostile (validate counts/offsets, cap allocations).
- Layering: `segy-core` (parse, no UI) → `render` (pure: samples → pixels / polyline paths, no egui) → `app` (egui). Render must be testable headless.
- Clean-room: SeiSee is *observed* black-box only. Never disassemble it, never commit its screenshots (keep under `refs/seisee/`, gitignored), never copy its icons/wording.

## Reference tooling
- Sample data: `/home/toto/Downloads/3X_75_PR.SGY` (5.1 MB, 751 samples/trace, dt 8000 µs, format 1 = IBM float). Don't commit it.
- SeiSee: `env WINEPREFIX=$HOME/.wine wine-stable "C:\Program Files (x86)\DMNG\SeiSee\SeiSee.exe" <file.sgy>` (install at `~/.wine/drive_c/Program Files (x86)/DMNG/SeiSee`).
- Screen capture (session is **Wayland**): `spectacle -b -n -a -o refs/seisee/<name>.png` (active window) ; `xdotool` only reaches XWayland windows, which wine uses, so `xdotool search --class seisee.exe` + `key`/`mousemove` can drive it. Capture each mode with identical file, size, gain.
- Record observations (not images) in `refs/seisee/observed.md`: default gain/clip, wiggle fill (positive lobe black?), trace spacing, axis/tick layout, colormap default, how zoom behaves.

## Plan (do in order; one commit per step; run `cargo fmt && cargo clippy -D warnings && cargo test` before each)
1. **Scaffold** workspace: `segy-core`, `render`, `app`. CI gate script.
2. **segy-core**: parse 3200-byte textual header (EBCDIC→ASCII), 400-byte binary header (samples/trace, interval, format code, fixed-length flag, rev), trace headers (at least inline/xline/offset/scalar/ns override). Sample formats: IBM float(1), i32(2), i16(3), IEEE f32(5), i8(8). Memory-map the file; lazy trace index; big-endian. Read only headers needed for drawing (ns, dt, format, chosen axis key). Tests: synthetic files built in code (incl. truncated, ns=0, bogus format) — no panics. Check against sample file: trace count = (size−3600)/(240+751·4).
3. **Amplitude pipeline**: per-trace/global RMS or percentile clip, gain (dB), AGC optional-off, polarity flip. Pure functions + unit tests.
4. **Image renderer**: traces×samples → RGBA via colormap LUT (gray, seismic red-white-blue, plus 1–2 more). Resample by min/max-decimation when zoomed out, bilinear when zoomed in. Never allocate more than the viewport.
5. **Wiggle renderer** (fast path per Performance section): per trace, baseline + amplitude·scale, clipped to ±N trace spacings; positive-lobe fill via polygon cut at baseline crossing (interpolate zero crossing); decimate to ≤2 points/pixel vertically when dense. Output as plain path data so it is testable; egui paints it.
6. **Viewport**: pan/zoom (separate trace and time axes), axes with ticks (trace number / time ms), wiggle auto-switches to image when traces < ~2 px apart (toggle).
7. **App**: open file dialog (`rfd`), mode toggle (Wiggle / Image / Overlay), gain & clip sliders, colormap pick, Save PNG (offscreen render).
8. **Compare with SeiSee**: for each mode capture SeiSee and the app with the same file/region/size; diff visually (and numerically on resampled grayscale: report mean abs diff). Fix discrepancies in clip/gain defaults, polarity, fill, axis orientation. Log findings in `refs/seisee/observed.md`.
9. **Performance pass**: criterion benches, tile cache + LOD + background decode, rayon; profile (`cargo flamegraph`) and hit the budgets on a generated 100k-trace file. Fuzz the parser; README.
10. **Feature sweep**: remaining drawing features from the Scope list, each compared against SeiSee where it has an equivalent.

## Done criteria
- Opens the sample file and shows wiggle, image, overlay that match SeiSee's character (polarity, clipping, orientation) at comparable settings.
- 100k-trace file pans at interactive rate; zero panics on fuzzed input.
- All gates green.

## Working rules
- Work the Plan steps in order, one per session/commit.
- Run the gates yourself (`./gate.sh`; benches from step 9) and paste real output; never claim green without it.
- Capture SeiSee and the app's own output (step 8), view both PNGs, and record differences in `refs/seisee/observed.md` before changing code.
- Update `STATUS.md` (current step, next step, open issues) after every step so a fresh session can resume.
- Ask the human only for: scope changes, adding heavy dependencies, or anything touching files outside this repo.
