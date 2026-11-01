# Testing

Run everything: `./gate.sh` (fmt, clippy -D warnings, all tests). Benchmarks: `cargo bench -p render` (release profile; debug numbers are not measurements).

## Layers
| Layer | Where | Proves | Does not prove |
|---|---|---|---|
| Unit | `crates/*/src` tests | parsing, math, geometry, hostile input never panics | anything visual |
| Controller | `crates/app/src/controller.rs` tests | state transitions (zoom box, selection, label rows) | mouse/keyboard handling |
| Real-input interaction | `crates/app/tests/primary_features.rs`, `kittest_interactions.rs` | pointer, wheel, keys and button clicks reach the handlers and change state | how it looks |
| Feature script | `crates/app/tests/demo_features.rs`, `src/demo.rs` | every demo step applies and its check passes | real input path (state-only steps) |
| Live scenario | `app file.sgy --scenario smoke` | real window runs, frame budget (fail >50 ms, warn >16 ms) | correctness of pixels |
| Offscreen video | `--example primary_demo`, `--example demo_video` | human-reviewable output, deterministic | automatic correctness: **look at the frames** |
| Reference comparison | step 8, `refs/seisee/` | parity with the reference viewer | — (not done yet) |

## Rules
1. Visual claims need an image you opened. Passing tests are not evidence that a picture is legible.
2. Use real input events for UI tests (pointer move/press/release, wheel, keys, click by label), not controller calls.
3. A new feature = a new step in the script (caption + gesture + check). The same script feeds the test and the video.
4. After writing a test, make it fail once on purpose (break the feature) to prove it can fail.
5. Wait for the progressive renderer (`needs_render()` false) before asserting pixels or capturing.
6. Tests that need `/home/toto/Downloads/3X_75_PR.SGY` currently **return OK when the file is missing** (silent skip): prefer synthetic data for logic; treat a skipped run as "not tested".
7. Do not run screen capture while a live scenario runs (it caused a false 80 ms frame failure).
8. Only glyphs present in the bundled font in button labels (no "✕"): a missing glyph panics the renderer.

## Adding a feature test
Add a `step!`/gesture in `src/demo.rs` (state-only, also used by the live `--demo`) and, if it is mouse/keyboard driven, in `examples/primary_demo/script.rs`; both must include a check. Run `cargo test -p app`, then regenerate the video and open 2–3 frames.

## Sample data caveat
`3X_75_PR.SGY` fills only Trace#/Seq/CDP headers (Inline, Xline, Offset are 0), so header-driven features (label rows) cannot show different values with it. Use a 3D file with full headers for those demos.
