# Status

- **Current step**: Step 3 completed (`render` amplitude pipeline: DC removal, O(N) sliding-window RMS AGC, trace balance, per-trace/global RMS/percentile/peak/absolute clip, gain in dB, polarity flip, zero-allocation scratch API, and unit tests).
- **Completed steps**:
  - Step 1: Workspace scaffold (`segy-core`, `render`, `app`), `gate.sh`, `.gitignore` (`target/`, `refs/`, `.toolchain/`, `*.sgy`), `git init`.
  - Step 2: `segy-core` parser & unit tests.
  - Step 3: `render` amplitude pipeline & unit tests.
- **Next step**: Step 4 (`render` image renderer: colormap LUTs, rayon column-parallel rendering, min/max decimation when zoomed out, bilinear interpolation when zoomed in, viewport-sized buffer only, unit tests, and criterion bench).
- **Open issues**: None.
