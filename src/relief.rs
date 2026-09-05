//! Hand-drawn relief: four height canvases, one per side of a texel.
//!
//! The generator infers height from an image's brightness. That is a good
//! guess for photographed stone and a bad one for a sprite, where colour means
//! paint rather than depth: a dark outline is not a trench, and a highlight is
//! not a hill. So the relief lets you say it outright instead of guessing —
//! for every texel, how high its **top**, **left**, **right** and **bottom**
//! edges stand.
//!
//! Four numbers per texel is exactly what a normal needs. The horizontal slope
//! is `right - left` and the vertical one is `bottom - top`: two subtractions,
//! no kernel, no neighbours, nothing inferred. That is why the bake is instant
//! and why it can run on every stroke, so the lit preview answers the brush.

use eframe::egui;
use image::{Rgba, RgbaImage};

use crate::normalmap::{self, HeightMap, MapKind, Settings, Shading};
use crate::shape::{self, Cut};
use crate::theme;

/// The height of a texel nobody has painted: flat, halfway up the range.
pub const FLAT: u8 = 128;
/// Longest side of a relief canvas. Painting texel by texel stops being the
/// point long before this, and every stroke rebakes the whole canvas.
pub const MAX_SIDE: u32 = 512;
const DEFAULT_SIDE: u32 = 32;
/// The ramp the brush works in. Ten per cent is coarse enough that a relief
/// stays readable as a handful of levels rather than a gradient.
pub const STEP: f32 = 0.1;
/// Every height the swatches offer, 0% to 100%.
pub const STEPS: usize = 11;
/// Deepest the undo stack goes.
const UNDO_DEPTH: usize = 64;

/// Which edge of the texel a canvas holds the height of.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Side {
    Top,
    Left,
    Right,
    Bottom,
}

impl Side {
    pub const ALL: [Side; 4] = [Side::Top, Side::Left, Side::Right, Side::Bottom];

    pub fn label(self) -> &'static str {
        match self {
            Side::Top => "Top",
            Side::Left => "Left",
            Side::Right => "Right",
            Side::Bottom => "Bottom",
        }
    }

    /// Filename suffix used when a single side is saved.
    pub fn suffix(self) -> &'static str {
        match self {
            Side::Top => "top",
            Side::Left => "left",
            Side::Right => "right",
            Side::Bottom => "bottom",
        }
    }

    fn index(self) -> usize {
        match self {
            Side::Top => 0,
            Side::Left => 1,
            Side::Right => 2,
            Side::Bottom => 3,
        }
    }
}

/// What a stamp of the brush does to the texels under it.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Tool {
    Pencil,
    Eraser,
    Bucket,
    Picker,
    Raise,
    Lower,
    Smooth,
}

impl Tool {
    pub const ALL: [Tool; 7] = [
        Tool::Pencil,
        Tool::Eraser,
        Tool::Bucket,
        Tool::Picker,
        Tool::Raise,
        Tool::Lower,
        Tool::Smooth,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Tool::Pencil => "Pencil",
            Tool::Eraser => "Eraser",
            Tool::Bucket => "Fill",
            Tool::Picker => "Pick",
            Tool::Raise => "Raise",
            Tool::Lower => "Lower",
            Tool::Smooth => "Smooth",
        }
    }

    pub fn hint(self) -> &'static str {
        match self {
            Tool::Pencil => "Set every texel under the brush to the Height below",
            Tool::Eraser => "Back to unpainted — flat, and transparent on the canvas",
            Tool::Bucket => "Flood the connected run of equal texels with the Height",
            Tool::Picker => "Read a texel's height back into the Height slider",
            Tool::Raise => "Nudge the texels under the brush up",
            Tool::Lower => "Nudge the texels under the brush down",
            Tool::Smooth => "Average each texel with its four neighbours",
        }
    }

    pub fn icon(self) -> theme::Icon {
        match self {
            Tool::Pencil => theme::Icon::Pencil,
            Tool::Eraser => theme::Icon::Eraser,
            Tool::Bucket => theme::Icon::Bucket,
            Tool::Picker => theme::Icon::Picker,
            Tool::Raise => theme::Icon::Raise,
            Tool::Lower => theme::Icon::Lower,
            Tool::Smooth => theme::Icon::Smooth,
        }
    }
}

/// One stroke, as the texels it changed: enough to undo it and to put it back.
/// A brush stroke stays on one canvas, but a shape writes all four at once and
/// has to come back off in a single press of Undo.
struct Edit {
    changes: Vec<(Side, usize, Option<u8>, Option<u8>)>,
}

impl Edit {
    /// The canvas to show when this edit is undone. Undoing something you
    /// cannot see is worse than not undoing it.
    fn side(&self) -> Option<Side> {
        self.changes.first().map(|&(side, ..)| side)
    }
}

/// Everything the four canvases turn into, rebuilt on every change.
pub struct Baked {
    pub normal: RgbaImage,
    pub height: RgbaImage,
    pub shading: Shading,
}

pub struct Relief {
    width: u32,
    height: u32,
    /// Top, Left, Right, Bottom — `None` is an unpainted texel, which bakes
    /// as flat and draws as transparent.
    canvases: [Vec<Option<u8>>; 4],

    pub side: Side,
    pub tool: Tool,
    /// The height the brush declares, 0..=1.
    pub value: f32,
    /// Brush width in texels.
    pub brush: u32,
    /// Bump depth for the bake. The relief has its own, because it is not
    /// derived from the image and should not move when the image knobs do.
    pub depth: f32,
    /// Size for the "New canvas" control.
    pub new_size: [u32; 2],
    /// How strongly the loaded image shows through behind the canvas. You are
    /// describing the shape of something, and you cannot aim a texel at a
    /// sprite you cannot see.
    pub reference: f32,
    /// How strongly the canvas itself is drawn, so the heights can be faded
    /// back to check them against the art underneath.
    pub opacity: f32,

    /// The region the next shape applies to. Empty means nothing is selected,
    /// which is different from everything being selected: a shape with no
    /// region is a no-op, not a canvas-wide dome.
    selection: Vec<bool>,
    /// Whether clicking the canvas picks a region instead of painting on it.
    pub selecting: bool,
    /// How far a colour can drift and still count as the same region.
    pub tolerance: f32,
    /// The shape the Apply button would stamp into the selection.
    pub cut: Cut,

    undo: Vec<Edit>,
    redo: Vec<Edit>,
    stroke: Option<Edit>,
    /// Texels already recorded in the stroke in progress, one flag per side
    /// per texel, so a brush that passes over the same place twice still
    /// undoes to where it started.
    touched: Vec<bool>,
    last_texel: Option<(i32, i32)>,

    /// Bumped on every change that the bake or the textures depend on.
    pub revision: u64,
    dirty: bool,
    baked: Option<Baked>,
    canvas_tex: Option<(Side, u64, egui::TextureHandle)>,
    normal_tex: Option<(u64, egui::TextureHandle)>,
    last_settings: Option<Settings>,
}

impl Default for Relief {
    fn default() -> Self {
        let n = (DEFAULT_SIDE * DEFAULT_SIDE) as usize;
        Self {
            width: DEFAULT_SIDE,
            height: DEFAULT_SIDE,
            canvases: [vec![None; n], vec![None; n], vec![None; n], vec![None; n]],
            side: Side::Top,
            tool: Tool::Pencil,
            value: 0.7,
            brush: 1,
            depth: 2.0,
            new_size: [DEFAULT_SIDE, DEFAULT_SIDE],
            reference: 0.6,
            opacity: 1.0,
            selection: vec![false; n],
            selecting: false,
            tolerance: 0.05,
            cut: Cut::default(),
            undo: Vec::new(),
            redo: Vec::new(),
            stroke: None,
            touched: vec![false; n * 4],
            last_texel: None,
            revision: 0,
            dirty: true,
            baked: None,
            canvas_tex: None,
            normal_tex: None,
            last_settings: None,
        }
    }
}

// -------------------------------------------------------------- the canvases

impl Relief {
    pub fn dims(&self) -> (u32, u32) {
        (self.width, self.height)
    }

    pub fn size_vec2(&self) -> egui::Vec2 {
        egui::vec2(self.width as f32, self.height as f32)
    }

    /// Has anything at all been painted, on any side?
    pub fn has_content(&self) -> bool {
        Side::ALL.iter().any(|&s| self.drawn(s))
    }

    /// Has this side been painted?
    pub fn drawn(&self, side: Side) -> bool {
        self.canvases[side.index()].iter().any(Option::is_some)
    }

    /// The height of one texel on one side, or `None` if it is unpainted.
    pub fn get(&self, side: Side, x: i32, y: i32) -> Option<u8> {
        self.index(x, y)
            .and_then(|i| self.canvases[side.index()][i])
    }

    fn index(&self, x: i32, y: i32) -> Option<usize> {
        if x < 0 || y < 0 || x >= self.width as i32 || y >= self.height as i32 {
            return None;
        }
        Some(y as usize * self.width as usize + x as usize)
    }

    /// The height used by the bake: unpainted texels are flat.
    fn at(&self, side: Side, i: usize) -> f32 {
        self.canvases[side.index()][i].unwrap_or(FLAT) as f32 / 255.0
    }

    /// Everything downstream is stale.
    fn mark(&mut self) {
        self.dirty = true;
        self.revision = self.revision.wrapping_add(1);
    }

    /// Resize every canvas, keeping whatever overlaps the new rectangle.
    pub fn resize(&mut self, w: u32, h: u32) {
        let w = w.clamp(1, MAX_SIDE);
        let h = h.clamp(1, MAX_SIDE);
        if (w, h) == (self.width, self.height) {
            return;
        }
        let n = (w * h) as usize;
        for canvas in &mut self.canvases {
            let mut next = vec![None; n];
            for y in 0..h.min(self.height) {
                for x in 0..w.min(self.width) {
                    next[(y * w + x) as usize] = canvas[(y * self.width + x) as usize];
                }
            }
            *canvas = next;
        }
        self.width = w;
        self.height = h;
        self.touched = vec![false; n * 4];
        // A selection names texels by index, and the indices have all moved.
        self.selection = vec![false; n];
        self.new_size = [w, h];
        // The old strokes name texels that may no longer exist.
        self.undo.clear();
        self.redo.clear();
        self.stroke = None;
        self.mark();
    }

    /// Match a newly loaded image, capped at [`MAX_SIDE`].
    pub fn fit_to(&mut self, w: u32, h: u32) {
        let (w, h) = fitted_dims(w, h);
        self.resize(w, h);
    }

    /// Everything downstream is stale — for the knobs the bake reads that are
    /// not on the canvases.
    pub fn touch(&mut self) {
        self.mark();
    }

    pub fn clear_side(&mut self, side: Side) {
        if !self.drawn(side) {
            return;
        }
        self.begin_stroke();
        for i in 0..self.canvases[side.index()].len() {
            self.write_at(side, i, None);
        }
        self.end_stroke();
    }
}

// ------------------------------------------------------------ the selection

impl Relief {
    /// Is anything selected? A shape needs somewhere to go.
    pub fn has_selection(&self) -> bool {
        self.selection.iter().any(|&s| s)
    }

    pub fn selected_count(&self) -> usize {
        self.selection.iter().filter(|&&s| s).count()
    }

    pub fn selection(&self) -> &[bool] {
        &self.selection
    }

    pub fn clear_selection(&mut self) {
        if self.has_selection() {
            self.selection = vec![false; (self.width * self.height) as usize];
            self.mark();
        }
    }

    pub fn select_all(&mut self) {
        self.selection = vec![true; (self.width * self.height) as usize];
        self.mark();
    }

    /// Everything the sprite covers — the one selection worth a button of its
    /// own, because a whole-silhouette bevel is most of what these tools get
    /// used for.
    pub fn select_opaque(&mut self, src: &RgbaImage) {
        self.set_selection(shape::select_opaque(src, self.width, self.height), false);
    }

    /// The magic wand: the contiguous run of one colour under the pointer.
    pub fn select_at(&mut self, src: &RgbaImage, x: i32, y: i32, add: bool) {
        if x < 0 || y < 0 || x >= self.width as i32 || y >= self.height as i32 {
            return;
        }
        let picked = shape::select_similar(
            src,
            self.width,
            self.height,
            x as u32,
            y as u32,
            self.tolerance,
        );
        self.set_selection(picked, add);
    }

    fn set_selection(&mut self, picked: Vec<bool>, add: bool) {
        if add {
            for (dst, src) in self.selection.iter_mut().zip(picked) {
                *dst |= src;
            }
        } else {
            self.selection = picked;
        }
        self.mark();
    }

    /// Stamp the current shape into the selection, as one undoable edit.
    /// Returns how many texels it wrote.
    pub fn apply_shape(&mut self) -> usize {
        let cells = shape::rasterize(&self.selection, self.width, self.height, &self.cut);
        let mut written = 0;
        self.begin_stroke();
        for (i, cell) in cells.into_iter().enumerate() {
            let Some(edges) = cell else { continue };
            written += 1;
            for side in Side::ALL {
                self.write_at(side, i, Some(edges[side.index()]));
            }
        }
        self.end_stroke();
        written
    }
}

// ---------------------------------------------------------------- the brush

impl Relief {
    pub fn stroking(&self) -> bool {
        self.stroke.is_some()
    }

    pub fn begin_stroke(&mut self) {
        self.stroke = Some(Edit {
            changes: Vec::new(),
        });
        self.touched.iter_mut().for_each(|t| *t = false);
        self.last_texel = None;
    }

    pub fn end_stroke(&mut self) {
        self.last_texel = None;
        let Some(mut edit) = self.stroke.take() else {
            return;
        };
        if edit.changes.is_empty() {
            return;
        }
        // Where every touched texel actually ended up, now that it has.
        for change in &mut edit.changes {
            change.3 = self.canvases[change.0.index()][change.1];
        }
        edit.changes
            .retain(|&(_, _, before, after)| before != after);
        if edit.changes.is_empty() {
            return;
        }
        self.undo.push(edit);
        if self.undo.len() > UNDO_DEPTH {
            self.undo.remove(0);
        }
        // A new stroke is a new history; anything undone past it is gone.
        self.redo.clear();
    }

    /// Paint from wherever the brush was last to `(x, y)`, so a fast drag
    /// leaves a line rather than a dotted trail.
    pub fn stroke_to(&mut self, x: i32, y: i32) {
        let (x0, y0) = self.last_texel.unwrap_or((x, y));
        // Plain DDA: at most one texel of the line per step in the long axis.
        let steps = (x - x0).abs().max((y - y0).abs()).max(1);
        for s in 0..=steps {
            let t = s as f32 / steps as f32;
            let px = (x0 as f32 + (x - x0) as f32 * t).round() as i32;
            let py = (y0 as f32 + (y - y0) as f32 * t).round() as i32;
            self.stamp(px, py);
        }
        self.last_texel = Some((x, y));
    }

    /// One stamp of the current tool.
    fn stamp(&mut self, x: i32, y: i32) {
        let side = self.side;
        let value = (self.value.clamp(0.0, 1.0) * 255.0).round() as u8;

        match self.tool {
            Tool::Picker => {
                if let Some(v) = self.get(side, x, y) {
                    self.value = v as f32 / 255.0;
                }
                return;
            }
            Tool::Bucket => {
                self.fill(side, x, y, Some(value));
                return;
            }
            _ => {}
        }

        let b = self.brush as i32;
        let lo = -b / 2;
        let cells: Vec<(i32, i32)> = (lo..lo + b)
            .flat_map(|dy| (lo..lo + b).map(move |dx| (dx, dy)))
            .map(|(dx, dy)| (x + dx, y + dy))
            .collect();

        if self.tool == Tool::Smooth {
            // Read the whole footprint before writing any of it, or the brush
            // smears in whichever direction the loop happens to run.
            let averaged: Vec<(i32, i32, u8)> = cells
                .iter()
                .filter(|&&(cx, cy)| self.index(cx, cy).is_some())
                .map(|&(cx, cy)| {
                    let mut sum = self.sample(side, cx, cy);
                    for (nx, ny) in [(cx - 1, cy), (cx + 1, cy), (cx, cy - 1), (cx, cy + 1)] {
                        sum += self.sample(side, nx, ny);
                    }
                    (cx, cy, (sum / 5.0).round() as u8)
                })
                .collect();
            for (cx, cy, v) in averaged {
                self.write(side, cx, cy, Some(v));
            }
            return;
        }

        for (cx, cy) in cells {
            let next = match self.tool {
                Tool::Eraser => None,
                Tool::Raise | Tool::Lower => {
                    // Counted in rungs rather than added in eighths of a per
                    // cent: nudging a texel repeatedly has to land on a swatch
                    // every time, and rounding error would walk it off the
                    // ramp within three presses.
                    let dir = if self.tool == Tool::Raise { 1.0 } else { -1.0 };
                    let base = self.get(side, cx, cy).unwrap_or(FLAT) as f32 / 255.0;
                    Some(rung_value((base / STEP).round() + dir))
                }
                _ => Some(value),
            };
            self.write(side, cx, cy, next);
        }
    }

    /// A texel's height for averaging, clamped at the border so the edge of
    /// the canvas does not pull the smooth tool towards flat.
    fn sample(&self, side: Side, x: i32, y: i32) -> f32 {
        let x = x.clamp(0, self.width as i32 - 1);
        let y = y.clamp(0, self.height as i32 - 1);
        self.get(side, x, y).unwrap_or(FLAT) as f32
    }

    fn fill(&mut self, side: Side, x: i32, y: i32, value: Option<u8>) {
        let Some(start) = self.index(x, y) else {
            return;
        };
        let target = self.canvases[side.index()][start];
        if target == value {
            return;
        }
        let mut stack = vec![(x, y)];
        while let Some((cx, cy)) = stack.pop() {
            let Some(i) = self.index(cx, cy) else {
                continue;
            };
            if self.canvases[side.index()][i] != target {
                continue;
            }
            self.write_at(side, i, value);
            stack.extend([(cx - 1, cy), (cx + 1, cy), (cx, cy - 1), (cx, cy + 1)]);
        }
    }

    fn write(&mut self, side: Side, x: i32, y: i32, value: Option<u8>) {
        if let Some(i) = self.index(x, y) {
            self.write_at(side, i, value);
        }
    }

    fn write_at(&mut self, side: Side, i: usize, value: Option<u8>) {
        let before = self.canvases[side.index()][i];
        if before == value {
            return;
        }
        self.canvases[side.index()][i] = value;
        let flag = side.index() * self.canvases[0].len() + i;
        if let Some(stroke) = &mut self.stroke
            && !self.touched[flag]
        {
            // Only the value the texel started at is recorded here; where it
            // ended up is read back once, when the stroke finishes. A wide
            // brush crosses the same texel many times, and rescanning the
            // list each time would make a long stroke quadratic.
            self.touched[flag] = true;
            stroke.changes.push((side, i, before, value));
        }
        self.mark();
    }
}

// ---------------------------------------------------------------- the history

impl Relief {
    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    pub fn can_redo(&self) -> bool {
        !self.redo.is_empty()
    }

    /// Undo the last stroke, and select a side it was on.
    pub fn undo(&mut self) {
        let Some(edit) = self.undo.pop() else { return };
        for &(side, i, before, _) in &edit.changes {
            self.canvases[side.index()][i] = before;
        }
        if let Some(side) = edit.side() {
            self.side = side;
        }
        self.redo.push(edit);
        self.mark();
    }

    pub fn redo(&mut self) {
        let Some(edit) = self.redo.pop() else { return };
        for &(side, i, _, after) in &edit.changes {
            self.canvases[side.index()][i] = after;
        }
        if let Some(side) = edit.side() {
            self.side = side;
        }
        self.undo.push(edit);
        self.mark();
    }
}

// ------------------------------------------------------------------ the bake

impl Relief {
    /// Rebake if anything moved, and keep the textures in step. Cheap enough
    /// to call every frame: it does nothing unless something changed.
    pub fn sync(&mut self, ctx: &egui::Context, settings: &Settings, albedo: Option<&RgbaImage>) {
        // The bake borrows flip X/Y, tileable and the AO and roughness knobs
        // from the image pipeline, so it is stale when those move too.
        if self.last_settings != Some(*settings) {
            self.last_settings = Some(*settings);
            self.mark();
        }
        if self.dirty || self.baked.is_none() {
            self.dirty = false;
            self.baked = Some(self.bake(settings, albedo));
        }

        self.refresh_canvas(ctx);
        if self.normal_tex.as_ref().map(|(r, _)| *r) != Some(self.revision) {
            let image = self.baked.as_ref().map(|b| to_color_image(&b.normal));
            if let Some(image) = image {
                self.normal_tex = Some((
                    self.revision,
                    ctx.load_texture("relief-normal", image, egui::TextureOptions::NEAREST),
                ));
            }
        }
    }

    /// Bring the canvas texture up to date with the canvas. Called again from
    /// the viewport, after a stroke has been taken and before it is drawn:
    /// [`Self::sync`] runs at the top of the frame, so on its own it would
    /// always be showing the texel you painted one frame ago.
    pub fn refresh_canvas(&mut self, ctx: &egui::Context) {
        let key = (self.side, self.revision);
        if self.canvas_tex.as_ref().map(|(s, r, _)| (*s, *r)) == Some(key) {
            return;
        }
        let image = self.canvas_image(self.side);
        self.canvas_tex = Some((
            self.side,
            self.revision,
            ctx.load_texture("relief-canvas", image, egui::TextureOptions::NEAREST),
        ));
    }

    pub fn baked(&self) -> Option<&Baked> {
        self.baked.as_ref()
    }

    pub fn canvas_texture(&self, side: Side) -> Option<egui::TextureId> {
        self.canvas_tex
            .as_ref()
            .filter(|(s, _, _)| *s == side)
            .map(|(_, _, t)| t.id())
    }

    pub fn normal_texture(&self) -> Option<egui::TextureId> {
        self.normal_tex.as_ref().map(|(_, t)| t.id())
    }

    /// The canvas as an image: painted texels grey by height, unpainted ones
    /// transparent, so "nothing here" and "flat, deliberately" look different.
    pub fn canvas_image(&self, side: Side) -> egui::ColorImage {
        let mut buf = vec![0u8; (self.width * self.height) as usize * 4];
        for (i, cell) in self.canvases[side.index()].iter().enumerate() {
            if let Some(v) = cell {
                buf[i * 4] = *v;
                buf[i * 4 + 1] = *v;
                buf[i * 4 + 2] = *v;
                buf[i * 4 + 3] = 255;
            }
        }
        egui::ColorImage::from_rgba_unmultiplied([self.width as usize, self.height as usize], &buf)
    }

    /// One side's heights as a greyscale image, for export.
    pub fn side_image(&self, side: Side) -> RgbaImage {
        let mut out = RgbaImage::new(self.width, self.height);
        for (px, cell) in out.pixels_mut().zip(self.canvases[side.index()].iter()) {
            let v = cell.unwrap_or(FLAT);
            *px = Rgba([v, v, v, 255]);
        }
        out
    }

    /// Four edge heights per texel into a normal map, a height map, and the
    /// maps the lit preview needs.
    fn bake(&self, settings: &Settings, albedo: Option<&RgbaImage>) -> Baked {
        let (w, h) = (self.width, self.height);
        let n = (w * h) as usize;
        let sx = if settings.flip_x { -1.0 } else { 1.0 };
        let sy = if settings.flip_y { -1.0 } else { 1.0 };
        let depth = self.depth.max(0.001);

        let mut buf = vec![0u8; n * 4];
        let mut data = vec![0.0f32; n];
        for i in 0..n {
            let (t, l, r, b) = (
                self.at(Side::Top, i),
                self.at(Side::Left, i),
                self.at(Side::Right, i),
                self.at(Side::Bottom, i),
            );
            data[i] = (t + l + r + b) * 0.25;
            // The slopes are the whole point: no kernel, no neighbours. What
            // the texel's edges were declared to be is what it is.
            let dx = (r - l) * depth;
            let dy = (b - t) * depth;
            // `dy` runs down the canvas and +Y runs up it, so the vertical
            // slope keeps its sign where the horizontal one is negated. Same
            // convention as the generator: off is OpenGL, `flip_y` is DirectX.
            let (nx, ny, nz) = (-dx * sx, dy * sy, 1.0);
            let len = (nx * nx + ny * ny + nz * nz).sqrt();
            buf[i * 4] = encode(nx / len);
            buf[i * 4 + 1] = encode(ny / len);
            buf[i * 4 + 2] = encode(nz / len);
            buf[i * 4 + 3] = 255;
        }
        let normal = RgbaImage::from_raw(w, h, buf).expect("buffer matches dimensions");

        let hm = HeightMap {
            width: w,
            height: h,
            data,
        };
        // AO reads depth from `strength`, and the relief keeps its own.
        let mut s = *settings;
        s.strength = depth;
        let height = hm.to_rgba();
        let ao = normalmap::render(&hm, &s, MapKind::Ao);
        let roughness = normalmap::render(&hm, &s, MapKind::Roughness);

        let albedo = match albedo {
            Some(img) if img.dimensions() == (w, h) => img.clone(),
            Some(img) => image::imageops::resize(img, w, h, image::imageops::FilterType::Triangle),
            // Nothing loaded: light a neutral surface, so the shape is all
            // there is to see.
            None => RgbaImage::from_pixel(w, h, Rgba([204, 204, 204, 255])),
        };

        Baked {
            shading: Shading {
                width: w,
                height: h,
                albedo,
                normal: normal.clone(),
                ao,
                roughness,
            },
            normal,
            height,
        }
    }
}

/// The height one rung of the ramp is stored as.
pub fn rung_value(rung: f32) -> u8 {
    (rung.clamp(0.0, (STEPS - 1) as f32) * STEP * 255.0).round() as u8
}

/// An image's dimensions shrunk to fit a relief canvas.
pub fn fitted_dims(w: u32, h: u32) -> (u32, u32) {
    let scale = (MAX_SIDE as f32 / w.max(h) as f32).min(1.0);
    (
        ((w as f32 * scale).round() as u32).max(1),
        ((h as f32 * scale).round() as u32).max(1),
    )
}

#[inline]
fn encode(v: f32) -> u8 {
    ((v * 0.5 + 0.5).clamp(0.0, 1.0) * 255.0).round() as u8
}

fn to_color_image(img: &RgbaImage) -> egui::ColorImage {
    egui::ColorImage::from_rgba_unmultiplied(
        [img.width() as usize, img.height() as usize],
        img.as_raw(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn flat_relief() -> Relief {
        let mut r = Relief::default();
        r.resize(4, 4);
        r
    }

    /// The whole promise: right higher than left tilts the surface, and the
    /// normal leans the way the slope says.
    #[test]
    fn edge_heights_become_the_slope() {
        let mut r = flat_relief();
        r.begin_stroke();
        r.side = Side::Right;
        r.value = 1.0;
        r.stamp(1, 1);
        r.side = Side::Left;
        r.value = 0.0;
        r.stamp(1, 1);
        r.end_stroke();

        let baked = r.bake(&Settings::default(), None);
        let px = baked.normal.get_pixel(1, 1);
        // right > left means the height rises to the right, so the normal
        // leans left: X below the 128 neutral.
        assert!(px[0] < 128, "expected a left-leaning normal, got {px:?}");
        // Nothing was said about top or bottom, so there is no vertical tilt.
        assert_eq!(px[1], 128);
        // And an untouched texel stays flat.
        assert_eq!(baked.normal.get_pixel(0, 0).0[..3], [128, 128, 255]);
    }

    /// The vertical half of the same promise, and the axis that is easy to
    /// get backwards: a texel whose bottom edge stands higher than its top
    /// slopes up as you go down, so its normal leans up the canvas.
    #[test]
    fn a_high_bottom_edge_leans_the_normal_up() {
        let mut r = flat_relief();
        r.begin_stroke();
        r.side = Side::Bottom;
        r.value = 1.0;
        r.stamp(1, 1);
        r.side = Side::Top;
        r.value = 0.0;
        r.stamp(1, 1);
        r.end_stroke();

        let baked = r.bake(&Settings::default(), None);
        let px = baked.normal.get_pixel(1, 1);
        assert!(px[1] > 128, "expected an up-leaning normal, got {px:?}");
        assert_eq!(px[0], 128, "and nothing sideways");

        let dx = Settings {
            flip_y: true,
            ..Settings::default()
        };
        assert!(
            r.bake(&dx, None).normal.get_pixel(1, 1).0[1] < 128,
            "DirectX is the other way round"
        );
    }

    /// A shape writes all four canvases, and has to come back off in one
    /// press: an artist who has to hit Undo four times will stop trusting it.
    #[test]
    fn a_shape_applies_and_undoes_as_one_step() {
        let mut r = Relief::default();
        r.resize(16, 16);
        r.select_all();
        r.cut.shape = crate::shape::Shape::Dome;
        assert!(r.apply_shape() > 0);
        assert!(
            Side::ALL.iter().all(|&s| r.drawn(s)),
            "all four are written"
        );

        r.undo();
        assert!(!r.has_content(), "and all four come back off together");
        r.redo();
        assert!(Side::ALL.iter().all(|&s| r.drawn(s)));
    }

    /// The end of the chain: a dome laid down as edge heights bakes to a
    /// normal map whose rims lean outwards, which is what makes it read as
    /// round when a light goes past.
    #[test]
    fn a_domes_rims_lean_apart() {
        let mut r = Relief::default();
        r.resize(16, 16);
        r.select_all();
        r.cut.shape = crate::shape::Shape::Dome;
        r.apply_shape();

        let baked = r.bake(&Settings::default(), None);
        let left = baked.normal.get_pixel(0, 8).0[0];
        let right = baked.normal.get_pixel(15, 8).0[0];
        let top = baked.normal.get_pixel(8, 0).0[1];
        let bottom = baked.normal.get_pixel(8, 15).0[1];
        assert!(left < 128, "the left rim faces left, got {left}");
        assert!(right > 128, "the right rim faces right, got {right}");
        assert!(top > 128, "the top rim faces up, got {top}");
        assert!(bottom < 128, "the bottom rim faces down, got {bottom}");
    }

    /// A selection names texels by index, so it cannot outlive a resize.
    #[test]
    fn resizing_drops_the_selection() {
        let mut r = Relief::default();
        r.resize(16, 16);
        r.select_all();
        assert_eq!(r.selected_count(), 256);
        r.resize(8, 8);
        assert_eq!(r.selected_count(), 0);
    }

    #[test]
    fn unpainted_texels_bake_flat() {
        let r = flat_relief();
        assert!(!r.has_content());
        let baked = r.bake(&Settings::default(), None);
        for px in baked.normal.pixels() {
            assert_eq!(px.0[..3], [128, 128, 255]);
        }
    }

    #[test]
    fn a_stroke_undoes_and_redoes_as_one() {
        let mut r = flat_relief();
        r.begin_stroke();
        r.stroke_to(0, 0);
        r.stroke_to(3, 0);
        r.end_stroke();
        assert!(r.drawn(Side::Top));

        r.undo();
        assert!(!r.drawn(Side::Top), "one stroke should undo in one step");
        r.redo();
        assert!(r.drawn(Side::Top));
    }

    /// A stroke that crosses its own path still undoes to where the canvas
    /// started, not to the middle of itself.
    #[test]
    fn a_texel_painted_twice_undoes_to_the_start() {
        let mut r = flat_relief();
        r.begin_stroke();
        r.value = 0.25;
        r.stamp(1, 1);
        r.end_stroke();

        r.begin_stroke();
        r.value = 0.5;
        r.stamp(1, 1);
        r.value = 1.0;
        r.stamp(1, 1);
        r.end_stroke();
        assert_eq!(r.get(Side::Top, 1, 1), Some(255));

        r.undo();
        assert_eq!(r.get(Side::Top, 1, 1), Some(64));
        r.redo();
        assert_eq!(r.get(Side::Top, 1, 1), Some(255));
    }

    /// A stroke that puts everything back where it found it is not history.
    #[test]
    fn a_stroke_that_changes_nothing_is_not_recorded() {
        let mut r = flat_relief();
        r.begin_stroke();
        r.value = 0.25;
        r.stamp(1, 1);
        r.end_stroke();

        r.begin_stroke();
        r.value = 1.0;
        r.stamp(1, 1);
        r.value = 0.25;
        r.stamp(1, 1);
        r.end_stroke();
        assert!(!r.can_redo());

        r.undo();
        assert_eq!(
            r.get(Side::Top, 1, 1),
            None,
            "one undo should reach bare canvas"
        );
    }

    #[test]
    fn a_dragged_stroke_leaves_no_gaps() {
        let mut r = flat_relief();
        r.begin_stroke();
        r.stroke_to(0, 0);
        r.stroke_to(3, 3);
        r.end_stroke();
        for i in 0..4 {
            assert!(r.get(Side::Top, i, i).is_some(), "gap at {i}");
        }
    }

    #[test]
    fn fill_stops_at_a_different_height() {
        let mut r = flat_relief();
        r.begin_stroke();
        r.value = 1.0;
        for y in 0..4 {
            r.stamp(1, y);
        }
        r.end_stroke();

        r.begin_stroke();
        r.tool = Tool::Bucket;
        r.value = 0.25;
        r.stamp(0, 0);
        r.end_stroke();

        // The left column is filled, the painted wall is untouched, and the
        // far side never got reached.
        assert_eq!(r.get(Side::Top, 0, 3), Some(64));
        assert_eq!(r.get(Side::Top, 1, 0), Some(255));
        assert_eq!(r.get(Side::Top, 3, 0), None);
    }

    /// Raise and Lower move by a rung, so a texel nudged from a swatch lands
    /// on a swatch.
    #[test]
    fn nudging_stays_on_the_ramp() {
        let mut r = flat_relief();
        r.begin_stroke();
        r.value = 0.3;
        r.stamp(0, 0);
        r.tool = Tool::Raise;
        r.stamp(0, 0);
        r.stamp(0, 0);
        r.end_stroke();
        assert_eq!(
            r.get(Side::Top, 0, 0),
            Some(FLAT),
            "0.3 up two rungs is 0.5"
        );

        r.begin_stroke();
        r.tool = Tool::Lower;
        for _ in 0..10 {
            r.stamp(0, 0);
        }
        r.end_stroke();
        assert_eq!(r.get(Side::Top, 0, 0), Some(0), "should stop at the floor");
    }

    #[test]
    fn resize_keeps_what_still_fits() {
        let mut r = flat_relief();
        r.begin_stroke();
        r.value = 1.0;
        r.stamp(0, 0);
        r.stamp(3, 3);
        r.end_stroke();

        r.resize(2, 2);
        assert_eq!(r.get(Side::Top, 0, 0), Some(255));
        assert_eq!(r.dims(), (2, 2));
    }

    #[test]
    fn fit_to_caps_at_the_maximum() {
        let mut r = Relief::default();
        r.fit_to(4096, 2048);
        assert_eq!(r.dims(), (MAX_SIDE, MAX_SIDE / 2));
    }
}
