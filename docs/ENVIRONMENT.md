# Environment notes (this machine, 2026-11-01)

- Session: KDE on **Wayland**. `spectacle -a` captures the active window (often the terminal); use full screen (`-f`) and check the image.
- **Screen recording is not possible unattended**: `spectacle -R` needs a portal permission the human must grant. `ffmpeg` cannot grab Wayland. Use offscreen rendering for videos.
- `xdotool` only reaches XWayland windows. `ydotool`/`wtype` exist; `/dev/uinput` is `root:input` and the user is in `input` (not tried).
- Offscreen GPU rendering works (Vulkan ICDs incl. software `lvp`); `egui_kittest` + wgpu renders the real UI to PNG.
- `ffmpeg` available (frames + concat file → mp4).
- Rust is **not** installed system-wide: the toolchain lives in `.toolchain/` (gitignored, ~1.6 GB). `gate.sh` sets it up.
- Reference viewer SeiSee: wine, `~/.wine/drive_c/Program Files (x86)/DMNG/SeiSee/SeiSee.exe`; launcher `~/.local/share/applications/wine/Programs/SeiSee/SeiSee.desktop`. Not yet captured.
