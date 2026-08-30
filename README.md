# Normal Mapper

A small desktop app (Rust + egui/eframe) that turns an image into a full 2D
texture set: tangent-space normal, height, ambient occlusion and roughness.

## Run

```sh
cargo run --release
```

## Features

- **Open** images via dialog (`Ctrl/Cmd+O`) or by dropping a file on the window;
  PNG, JPEG, BMP, TGA, TIFF, WebP, GIF.
- **Height extraction** from luminance, average RGB, or a single R/G/B/A channel,
  with contrast, invert, and a Gaussian pre-blur to tame noise.
- **Normal generation** with Sobel, Scharr or Prewitt kernels, a logarithmic
  strength slider, flip X, flip Y (OpenGL/Bevy vs. DirectX/Unity green channel),
  and seamless/tileable wrap sampling.
- **Ambient occlusion** — horizon-based, marching eight directions over the
  height field; radius and amount sliders.
- **Roughness** — a base level plus fine height detail isolated with a
  high-pass, optionally inverted.
- **Live preview** with Source / Height / Normal / AO / Roughness tabs.
  "Fast preview" generates from a copy downscaled to 1024px; exports always use
  the full resolution.
- **Zoom and pan** — scroll or pinch to zoom at the cursor, drag to pan,
  double-click to toggle fit/1:1. `+` / `-` step, `0` is 1:1, `F` fits, and
  there are `− 100% + 1:1 Fit` buttons in the toolbar plus a zoom slider.
  Zoom is measured in source pixels on every tab, and switches to nearest-
  neighbour sampling above 100% so you can inspect individual texels.
- **Non-blocking generation** — all four maps are computed on a worker thread
  (rayon-parallel inside), with an 80 ms debounce so dragging a slider queues
  one job instead of sixty. Stale results are discarded by generation number.
- **Export** the visible map (`Ctrl/Cmd+S`) as PNG/TGA/JPEG, or "Export all…"
  to write `name_normal.png`, `name_height.png`, `name_ao.png` and
  `name_roughness.png` into a folder.
- **Batch convert** a whole folder: pick input and output directories and which
  maps to write, then it runs across all images in parallel with a progress bar.
- **Presets** — save/load the settings as JSON, and the last-used settings are
  restored on the next launch.
- Light/dark theme switch; generation time shown in the status bar.

## Layout

- `src/normalmap.rs` — height extraction, blur, and the normal/AO/roughness math
  (no UI dependencies, covered by unit tests: `cargo test`).
- `src/worker.rs` — the background generation thread and its request/response
  channels.
- `src/batch.rs` — the folder-conversion window and its worker.
- `src/main.rs` — eframe app, panels, texture upload, file I/O.
