# Issues and improvements (ranked)

Tags: OBSERVED = seen in output, INFERRED = concluded, ASSUMED = not checked.

1. **HIGH, the reference was never observed.** All behaviour comes from general seismic-viewer knowledge (ASSUMED). Do step 8 (separate project/branch with the `observe-and-recreate` skill): launch SeiSee under wine, capture every mode/menu, fill the feature ledger, then compare.
2. **HIGH, some tests pass vacuously.** `kittest_*`, `primary_features`, `demo_features` and image tests return OK when `/home/toto/Downloads/3X_75_PR.SGY` is absent (OBSERVED in code). Fix: synthetic SEG-Y fixtures for logic tests; make a skip visible.
3. **MED, performance unproven.** Budget 100k traces x 2000 samples <16 ms/frame is a target (ASSUMED). Live scenario on 1592 traces: mean ~7 ms, max ~22 ms (OBSERVED; warnings on CDP axis change and Save PNG). Run `cargo bench -p render` in release and on a generated 100k-trace file.
4. **MED, Shift+wheel zoom-H likely broken (INFERRED).** egui turns Shift+vertical wheel into horizontal scroll; the app reads only vertical. Hint text promises it. Test with a real mouse and fix or remove the hint.
5. **MED, wiggle legibility.** Full-section wiggle is illegible; auto-switch is off by default; an earlier render showed black block artifacts on the right (OBSERVED). Decide defaults against SeiSee.
6. **LOW, status bar overflows** in narrow windows (selected-trace stats cut off) (OBSERVED).
7. **LOW, selected-trace waveform** is scaled to min(peak, 3 x RMS), spikes clip (design choice; compare with reference).
8. **LOW, demo caption banner** covers the bottom of the section (demo only).
9. **LOW, live `--demo` uses direct state changes** (no real pointer); the offscreen `primary_demo` uses real input.
10. **INFO, sample file headers** (Inline/Xline/Offset = 0) cannot demonstrate multi-label rows.
