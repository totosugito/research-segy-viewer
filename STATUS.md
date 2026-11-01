# Status

- **Current step**: Step 5 completed (`render` wiggle renderer: pure path/polygon output, positive/negative/both variable-area fill with interpolated zero crossings, line-only mode, excursion scale and clipping, vertical min/max decimation to <=2 points/pixel when dense, fast scanline RGBA rasterizer, unit tests including sample file, and criterion benchmark).
- **Completed steps**:
  - Step 1: Workspace scaffold (`segy-core`, `render`, `app`), `gate.sh`, `.gitignore` (`target/`, `refs/`, `.toolchain/`, `*.sgy`), `git init`.
  - Step 2: `segy-core` parser & unit tests.
  - Step 3: `render` amplitude pipeline & unit tests.
  - Step 4: `render` image renderer, colormap LUTs, unit tests, and criterion benchmark.
  - Step 5: `render` wiggle renderer, variable-area polygon builder, vertical min/max decimation, fast rasterizer, unit tests, and criterion benchmark.
- **Next step**: Step 6 (`render` viewport model: independent H/V pan+zoom, time window, trace decimation, axis tick computation, auto-switch wiggle->image when traces <~2px apart; pure and unit tested).
- **Open issues**: None.
