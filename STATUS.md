# Status

- **Current step**: Step 6 completed (`render` viewport model: independent H/V pan+zoom, time window in ms, trace decimation, `1-2-5 × 10^k` time/trace/header axis tick computation, cursor readout, and auto-switch `Wiggle -> Image` when traces `< ~2.0` px apart; pure and unit tested).
- **Completed steps**:
  - Step 1: Workspace scaffold (`segy-core`, `render`, `app`), `gate.sh`, `.gitignore` (`target/`, `refs/`, `.toolchain/`, `*.sgy`), `git init`.
  - Step 2: `segy-core` parser & unit tests.
  - Step 3: `render` amplitude pipeline & unit tests.
  - Step 4: `render` image renderer, colormap LUTs, unit tests, and criterion benchmark.
  - Step 5: `render` wiggle renderer, variable-area polygon builder, vertical min/max decimation, fast rasterizer, unit tests, and criterion benchmark.
  - Step 6: `render` viewport model (pan/zoom, time window, trace decimation, axis ticks, auto-switch wiggle->image) & unit tests.
- **Next step**: Step 7 (`app` egui UI: open file dialog `rfd`, mode toggle Wiggle/Image/Overlay, gain & clip sliders, colormap pick, interactive pan/zoom/axes, and offscreen Save PNG).
- **Open issues**: None.
