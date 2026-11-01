# SEG-Y viewer — instructions for agents

Read-only, fast Rust/egui SEG-Y viewer with the richest possible wiggle/image drawing features (no header editing or writing). Read `.agents/skills/segy-viewer/SKILL.md` first: it holds the scope, the step-by-step plan and the SeiSee screen-capture workflow. Style follows `https://github.com/storytold/gridcraft (AGENTS.md)` (no panics, no unsafe, hostile file input).

## Read before working
1. `STATUS.md` (what is verified vs not), `ISSUES.md` (ranked problems), `docs/TESTING.md` (test layers and rules), `docs/ENVIRONMENT.md` (what this machine can and cannot do).
2. General workflow rules: `~/.agents/skills/agent-dev-workflow/SKILL.md` (evidence, tests, long runs, monitoring) and, for reference-product work, `~/.agents/skills/observe-and-recreate/SKILL.md`.

## Project rules
- Gate: `./gate.sh` must be green before every commit; one task per commit. Run it yourself, do not trust a report.
- Claims need artifacts: say OBSERVED / INFERRED / ASSUMED. Never write "matches SeiSee" without a side-by-side.
- UI tests use real input events; new features add a step (caption + gesture + check) to the shared demo script.
- Look at the rendered images/frames before reporting a visual feature done.
- Keep sessions short (one task, 5-10 min); report status every minute while something runs.
