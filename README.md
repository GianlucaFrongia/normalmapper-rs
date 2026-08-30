# Normal Mapper

A small desktop app (Rust + egui/eframe) that turns an image into a tangent-space
normal map.

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
- **Live preview** with Source / Height / Normal tabs, fit-to-window or a zoom
  slider, and scroll-to-pan. "Fast preview" generates from a copy downscaled to
  1024px; exports always use the full source resolution.
- **Save** the normal map (`Ctrl/Cmd+S`) or the height map as PNG, TGA or JPEG,
  plus reload and reset-settings.
- Light/dark theme switch; generation time shown in the status bar.

## Layout

- `src/normalmap.rs` — height extraction, blur, and the normal-map math (no UI
  dependencies, covered by unit tests: `cargo test`).
- `src/main.rs` — eframe app, panels, texture upload, file I/O.
