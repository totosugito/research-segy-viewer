# Status

- **Current step**: Step 2 completed (`segy-core` read-only SEG-Y rev 0/1 parser, EBCDIC→ASCII textual header, binary header, trace header & axis key views, formats 1/2/3/5/8 decoders, mmap + lazy constant-stride/variable-length trace index, hostile input & `3X_75_PR.SGY` tests).
- **Completed steps**:
  - Step 1: Workspace scaffold (`segy-core`, `render`, `app`), `gate.sh`, `.gitignore` (`target/`, `refs/`, `.toolchain/`, `*.sgy`), `git init`.
  - Step 2: `segy-core` parser & unit tests.
- **Next step**: Step 3 (`render` amplitude pipeline: per-trace/global RMS or percentile clip, gain in dB, optional AGC, polarity flip, DC removal / trace balance, pure functions + unit tests).
- **Open issues**: None.
