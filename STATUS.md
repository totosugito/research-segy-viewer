# Status

- **Done and verified by `./gate.sh` (fmt, clippy -D warnings, 62 tests):** steps 1-7 (parser, amplitude, image and wiggle renderers, viewport, egui app), live scenario runner (15 steps), egui_kittest interaction tests, zoom box + trace selection (waveform highlight), up to 5 X label rows, feature demo script (76 checked steps), live `--demo`, offscreen demo videos (`primary_demo`, `demo_video`).
- **Not done:** step 8 (comparison with SeiSee, never observed), step 9 (release-profile performance pass on 100k traces), step 10 (feature sweep vs the reference).
- **Not verified:** performance target; Shift/Alt+wheel with a real mouse; behaviour parity with SeiSee.
- **Next step:** step 8 in a separate project/branch using `~/.agents/skills/observe-and-recreate`.
- **Open issues:** see ISSUES.md.
