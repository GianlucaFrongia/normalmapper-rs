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
  with contrast, invert, and a Gaussian pre-blur to tame noise. **Ignore
  transparent** hands every see-through texel the height of the nearest opaque
  one: a sprite's surround is whatever colour was left under the eraser, and
  differentiating against it rings the silhouette in a cliff nobody drew.
- **Normal generation** with Sobel, Scharr, Prewitt or 1-texel kernels, a
  logarithmic strength slider, flip X, flip Y, and seamless/tileable wrap
  sampling. Off, **Flip Y** is the OpenGL convention (+Y up — Unity, Godot,
  Bevy, Blender); on, it is DirectX (+Y down — Unreal).
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
  Zoom is measured in source pixels on every tab; turn on pixel art mode for
  nearest-neighbour sampling, so you can inspect individual texels.
- **Pixel art mode** — power-of-two zoom rungs (1/64× … 64×) so texels never
  come out uneven, nearest-neighbour at *every* zoom, the image snapped to
  whole device pixels, a texel grid from 8× up, and a fit that is allowed to
  magnify a small sprite. Pairs with the **1 px kernel**, a central difference
  over a single texel that keeps a hard edge one pixel wide instead of
  smearing it across three like Sobel does.
- **Relief** — four height canvases, painted texel by texel: how high every
  texel's **top**, **left**, **right** and **bottom** edge stands. Right minus
  left is the horizontal slope and bottom minus top the vertical one, so the
  normal falls straight out of the four numbers — no kernel, no neighbours,
  nothing inferred from the picture. That is why it bakes on every stroke, and
  why the lit column answers the brush while you are still holding it. Pencil,
  eraser, fill, eyedropper, raise, lower and smooth; a 1–8 texel brush;
  per-stroke undo (`Ctrl/Cmd+Z`); its own Depth, so the image's Strength slider
  cannot move it. Heights come in ten-per-cent rungs, picked from a row of
  swatches showing the grey each one bakes to — a relief in eleven greys can be
  read straight off the canvas, and Raise and Lower count in rungs so a texel
  nudged from a swatch lands on a swatch. Unpainted texels stay transparent on
  the canvas and bake as flat — "nothing here" and "flat, deliberately" are
  different claims, and the loaded image shows through behind them unfiltered,
  as it is, whatever the zoom (**Reference** fades it, **Canvas** fades the
  heights back off it). Drag to paint, right-drag to pan. Paint with no image
  open at all, or let a newly opened image size the canvas for you (capped at
  512 a side).
- **Lit preview** — a column down the right of the window that Blinn-Phong
  shades the normal, AO and roughness maps together, with a drag-to-aim light
  ball plus ambient, specular and albedo controls. The only honest way to judge
  a normal map, and it sits beside the knobs that made it rather than floating
  over them: turn a slider on the left and watch the light answer on the right.
  Drag its edge to resize it, or fold it away with **Lit** in the toolbar.
  It shades the painted relief as soon as there is one, and the image's own
  maps until then; a line under the header says which.
- **Non-blocking generation** — all four maps are computed on a worker thread
  (rayon-parallel inside), with an 80 ms debounce so dragging a slider queues
  one job instead of sixty. Stale results are discarded by generation number.
- **Export** the visible map or relief canvas (`Ctrl/Cmd+S`) as PNG/TGA/JPEG, or "Export all…"
  to write `name_normal.png`, `name_height.png`, `name_ao.png` and
  `name_roughness.png` into a folder.
- **Batch convert** a whole folder: pick input and output directories and which
  maps to write, then it runs across all images in parallel with a progress bar.
- **Presets** — save/load the settings as JSON, and the last-used settings are
  restored on the next launch.
- **The look** is the app's own, not egui's default: a `Visuals` set in
  `theme.rs` — three surface levels, an indigo accent, filled slider rails,
  small-caps section rules instead of headings, pill tabs, every readout in the
  monospace face, and the image lifted off a sunken well on a soft shadow.
  Light and dark differ only in their numbers.
- Light/dark theme switch; generation time shown in the status bar.

## Taking the maps to an engine

The maps are only half the job; how they are imported decides whether they
survive.

- **Green channel.** OpenGL (+Y up) for Unity, Godot, Bevy and Blender —
  leave **Flip Y** off. DirectX (+Y down) for Unreal — turn it on. A map in
  the wrong convention does not error; every bump simply lights as a dent,
  which is why there is a test asserting the top of a bump is the green half.
- **Colour space.** A normal map is a vector packed into RGB, not a picture.
  Import it as linear / Non-Color: Unity's *Normal map* texture type, Unreal's
  *TC_Normalmap*, Godot with sRGB off. Read as sRGB it will be subtly, and
  unfixably, wrong.
- **Filtering.** For pixel art: point/nearest, no mipmaps, no lossy
  compression. Saving a normal map as JPEG destroys it — the app says so if
  you try.
- **Wiring it up.** Unity 2D/URP takes the normal as a Sprite *Secondary
  Texture* named `_NormalMap`, on a `Sprite-Lit-Default` material with a
  `Light2D` in the scene. Godot 4 wants a `CanvasTexture` with `texture_normal`
  (and `texture_specular`) under a `PointLight2D`.
- **AO and roughness are 3D-shaped.** 2D lit pipelines mostly have nowhere to
  put them: Godot's `CanvasTexture` takes a specular map rather than a
  roughness one, and Unity 2D has no AO slot at all. The usual answer is to
  multiply AO into the albedo at bake time and use roughness to author the
  specular map. They are exported because the lit preview shades with them,
  and because 3D material workflows do take all four.
- **Hand-paint, don't infer.** For sprites the luminance path is a first draft
  at best — colour is paint, not depth, and a dark outline is not a trench.
  That is what the relief is for, and it is how lit pixel art is actually
  authored.

The **lit preview** shades the way an engine does rather than the way that is
cheapest: lighting in linear space and encoding on the way out, ambient
occlusion applied to the ambient term only, and specular gated on N·L so a face
turned away from the light cannot catch a highlight.

## Layout

- `src/normalmap.rs` — height extraction, blur, and the normal/AO/roughness math
  (no UI dependencies, covered by unit tests: `cargo test`).
- `src/worker.rs` — the background generation thread and its request/response
  channels.
- `src/batch.rs` — the folder-conversion window and its worker.
- `src/light.rs` — the lit-preview column and the light-direction ball.
- `src/relief.rs` — the four hand-painted edge-height canvases, the brush and
  its undo history, and the bake that turns four numbers per texel into a
  normal map (unit-tested: `cargo test`).
- `src/theme.rs` — the look: the palettes, the style they are installed as, and
  the few widgets egui has no stock version of (a section rule, a pill tab, the
  well the image sits in, and the relief tools' icons).
- `src/main.rs` — eframe app, panels, texture upload, file I/O.
