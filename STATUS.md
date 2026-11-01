# Status

- **Current step**: Step 7 completed (`app` egui/eframe viewer: open SEG-Y via `rfd` file dialog, drag & drop, and CLI argument `app <file.sgy>`; Wiggle/Image/Overlay mode toggle; gain, clip, colormap, polarity, AGC, DC removal, and wiggle controls; mouse pan/zoom, time/trace axes with ticks, live cursor readout; progressive non-blocking coarse→full rendering into a viewport-sized buffer on state change; offscreen Save PNG; `snapshot` example and GUI/controller tests).
- **Completed steps**:
  - Step 1: Workspace scaffold (`segy-core`, `render`, `app`), `gate.sh`, `.gitignore` (`target/`, `refs/`, `.toolchain/`, `*.sgy`), `git init`.
  - Step 2: `segy-core` parser & unit tests.
  - Step 3: `render` amplitude pipeline & unit tests.
  - Step 4: `render` image renderer, colormap LUTs, unit tests, and criterion benchmark.
  - Step 5: `render` wiggle renderer, variable-area polygon builder, vertical min/max decimation, fast rasterizer, unit tests, and criterion benchmark.
  - Step 6: `render` viewport model (pan/zoom, time window, trace decimation, axis ticks, auto-switch wiggle->image) & unit tests.
  - Step 7: `app` egui/eframe UI, `render` unified section/PNG renderer, progressive non-blocking controller, `snapshot` example, and headless/snapshot tests.
- **Next step**: Step 8 (Compare with SeiSee: capture SeiSee and the app for each mode with identical file/region/size, diff visually and numerically on resampled grayscale, fix discrepancies, and log findings in `refs/seisee/observed.md`).
- **Open issues**: None.
