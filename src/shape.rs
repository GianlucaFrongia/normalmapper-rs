//! Shapes: say what a region *is*, and let the tool work out the slopes.
//!
//! Painting a 32×32 sprite texel by texel is a lot of work for a shape the
//! eye reads in one word — "the arm is a tube", "the head is a ball". So the
//! other half of the relief is this: select a region, name its cross-section,
//! and the four edge heights fall out of a distance field.
//!
//! Everything here is one idea applied seven ways. Measure how far each texel
//! sits from the edge of its region, turn that distance into a height with a
//! profile curve, and sample the curve at the four points that matter — the
//! midpoints of the texel's edges, half a texel out from its centre. That last
//! step is why the shapes land in the same four canvases the brush paints:
//! a dome's border texel gets a genuinely different left and right height, so
//! it has a slope to light, rather than the flat plateau a single height per
//! texel would give it.

use image::RgbaImage;

/// Big enough to stand in for infinity in the distance transform, small
/// enough that squaring and subtracting it stays finite.
const FAR: f32 = 1e10;
/// Sentinel for the parabola envelope, well below any distance FAR can give.
const EDGE: f32 = 1e20;

/// The cross-section a shape gives the region it is applied to.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Shape {
    Flat,
    Dome,
    Cone,
    Bevel,
    RoundBevel,
    /// A tube lying across the canvas: round top to bottom, flat along X.
    TubeX,
    /// A tube standing up the canvas: round left to right, flat along Y.
    TubeY,
}

impl Shape {
    pub const ALL: [Shape; 7] = [
        Shape::Flat,
        Shape::Dome,
        Shape::Cone,
        Shape::Bevel,
        Shape::RoundBevel,
        Shape::TubeX,
        Shape::TubeY,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Shape::Flat => "Flat",
            Shape::Dome => "Dome",
            Shape::Cone => "Cone",
            Shape::Bevel => "Bevel",
            Shape::RoundBevel => "Round bevel",
            Shape::TubeX => "Tube ↔",
            Shape::TubeY => "Tube ↕",
        }
    }

    pub fn hint(self) -> &'static str {
        match self {
            Shape::Flat => "A plateau, all one height. No slope, so nothing to light",
            Shape::Dome => "Rounded like a ball: steep at the rim, flat on top",
            Shape::Cone => "Straight sides rising to the middle",
            Shape::Bevel => "A flat top with a straight ramp round the edge",
            Shape::RoundBevel => "A flat top eased into the edge",
            Shape::TubeX => "A tube lying across: round top to bottom",
            Shape::TubeY => "A tube standing up: round left to right",
        }
    }

    /// Whether the shape rounds over a fixed border width, rather than over
    /// however wide the region happens to be. The Width slider is only for
    /// these two.
    pub fn uses_width(self) -> bool {
        matches!(self, Shape::Bevel | Shape::RoundBevel)
    }

    /// The cross-section itself: how high the surface stands at an inset of
    /// `t`, where 0 is the region's edge and 1 its deepest point.
    fn profile(self, t: f32) -> f32 {
        let t = t.clamp(0.0, 1.0);
        match self {
            Shape::Flat => 1.0,
            // A quarter circle: the rim falls away vertically, which is what
            // makes a dome read as a ball rather than a mound.
            Shape::Dome | Shape::TubeX | Shape::TubeY => (1.0 - (1.0 - t) * (1.0 - t)).sqrt(),
            Shape::Cone | Shape::Bevel => t,
            Shape::RoundBevel => t * t * (3.0 - 2.0 * t),
        }
    }
}

/// A shape, and the two numbers that say how tall it stands.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Cut {
    pub shape: Shape,
    /// Height at the region's edge, 0..=1.
    pub floor: f32,
    /// Height at its deepest point, 0..=1.
    pub peak: f32,
    /// Ramp width in texels, for the bevels.
    pub width: f32,
    /// Carve the shape in instead of standing it out.
    pub concave: bool,
}

impl Default for Cut {
    fn default() -> Self {
        Self {
            shape: Shape::Dome,
            floor: 0.0,
            peak: 1.0,
            width: 3.0,
            concave: false,
        }
    }
}

/// Rasterise a shape over `selected`, returning each selected texel's four
/// edge heights as `[top, left, right, bottom]` — the order [`crate::relief::Side`]
/// indexes by. Unselected texels come back `None` and are left alone.
pub fn rasterize(selected: &[bool], w: u32, h: u32, cut: &Cut) -> Vec<Option<[u8; 4]>> {
    let n = (w * h) as usize;
    let mut out = vec![None; n];
    if !selected.iter().any(|&s| s) {
        return out;
    }

    // Distances live on a grid one texel bigger all round, so a region running
    // off the edge of the canvas still has an edge there. Anything outside the
    // selection — including that ring — is what the distance is measured to.
    let (pw, ph) = (w + 2, h + 2);
    let field = match cut.shape {
        Shape::TubeX => axis_distance(selected, w, h, true),
        Shape::TubeY => axis_distance(selected, w, h, false),
        _ => euclidean_distance(selected, w, h),
    };

    // How far in the deepest texel sits, which is what "1" on the profile
    // means for the shapes that fill their region rather than a fixed border.
    let deepest = field.iter().cloned().fold(0.0f32, f32::max).max(1e-6);
    let scale = if cut.shape.uses_width() {
        1.0 / cut.width.max(0.5)
    } else {
        1.0 / deepest
    };

    // Half a texel out from the centre, in each of the four directions.
    const EDGES: [(f32, f32); 4] = [(0.0, -0.5), (-0.5, 0.0), (0.5, 0.0), (0.0, 0.5)];
    let height = |t: f32| {
        let p = cut.shape.profile(t);
        // Concave is the same shape read from the other end: the border stands
        // at the peak and the middle sinks to the floor.
        let v = if cut.concave {
            cut.peak - (cut.peak - cut.floor) * p
        } else {
            cut.floor + (cut.peak - cut.floor) * p
        };
        (v.clamp(0.0, 1.0) * 255.0).round() as u8
    };

    for y in 0..h {
        for x in 0..w {
            let i = (y * w + x) as usize;
            if !selected[i] {
                continue;
            }
            let mut edges = [0u8; 4];
            for (e, (dx, dy)) in EDGES.iter().enumerate() {
                let d = sample(&field, pw, ph, x as f32 + dx, y as f32 + dy);
                edges[e] = height(d * scale);
            }
            out[i] = Some(edges);
        }
    }
    out
}

/// Bilinear read of the padded distance field at a point in canvas
/// coordinates, where a texel's centre is at its integer position.
fn sample(field: &[f32], pw: u32, ph: u32, x: f32, y: f32) -> f32 {
    let (px, py) = (x + 1.0, y + 1.0);
    let (x0, y0) = (px.floor(), py.floor());
    let (fx, fy) = (px - x0, py - y0);
    let at = |gx: f32, gy: f32| -> f32 {
        if gx < 0.0 || gy < 0.0 || gx >= pw as f32 || gy >= ph as f32 {
            return 0.0;
        }
        field[gy as usize * pw as usize + gx as usize]
    };
    let top = at(x0, y0) * (1.0 - fx) + at(x0 + 1.0, y0) * fx;
    let bottom = at(x0, y0 + 1.0) * (1.0 - fx) + at(x0 + 1.0, y0 + 1.0) * fx;
    top * (1.0 - fy) + bottom * fy
}

/// Exact Euclidean distance from every texel to the nearest unselected one,
/// on the padded grid. Felzenszwalb and Huttenlocher's transform: two passes
/// of a one-dimensional lower envelope, linear in the number of texels.
fn euclidean_distance(selected: &[bool], w: u32, h: u32) -> Vec<f32> {
    let (pw, ph) = ((w + 2) as usize, (h + 2) as usize);
    let mut f = vec![0.0f32; pw * ph];
    for y in 0..h as usize {
        for x in 0..w as usize {
            if selected[y * w as usize + x] {
                f[(y + 1) * pw + (x + 1)] = FAR;
            }
        }
    }

    let mut column = vec![0.0f32; ph.max(pw)];
    let mut result = vec![0.0f32; ph.max(pw)];
    let mut v = vec![0usize; ph.max(pw)];
    let mut z = vec![0.0f32; ph.max(pw) + 1];

    for y in 0..ph {
        column[..pw].copy_from_slice(&f[y * pw..y * pw + pw]);
        envelope(&column[..pw], &mut result[..pw], &mut v, &mut z);
        f[y * pw..y * pw + pw].copy_from_slice(&result[..pw]);
    }
    for x in 0..pw {
        for y in 0..ph {
            column[y] = f[y * pw + x];
        }
        envelope(&column[..ph], &mut result[..ph], &mut v, &mut z);
        for y in 0..ph {
            f[y * pw + x] = result[y];
        }
    }
    // The passes work in squared distances, because that is what keeps the
    // envelope a parabola.
    for d in &mut f {
        *d = d.max(0.0).sqrt();
    }
    f
}

/// One-dimensional squared distance transform of a sampled function.
fn envelope(f: &[f32], d: &mut [f32], v: &mut [usize], z: &mut [f32]) {
    let n = f.len();
    let cross = |f: &[f32], q: usize, p: usize| -> f32 {
        ((f[q] + (q * q) as f32) - (f[p] + (p * p) as f32)) / (2.0 * q as f32 - 2.0 * p as f32)
    };

    let mut k = 0usize;
    v[0] = 0;
    z[0] = -EDGE;
    z[1] = EDGE;
    for q in 1..n {
        let mut s = cross(f, q, v[k]);
        while k > 0 && s <= z[k] {
            k -= 1;
            s = cross(f, q, v[k]);
        }
        k += 1;
        v[k] = q;
        z[k] = s;
        z[k + 1] = EDGE;
    }

    k = 0;
    for (q, dq) in d.iter_mut().enumerate().take(n) {
        while z[k + 1] < q as f32 {
            k += 1;
        }
        let step = q as f32 - v[k] as f32;
        *dq = step * step + f[v[k]];
    }
}

/// Distance to the nearest unselected texel along one axis only, which is what
/// makes a tube a tube: it rounds across its axis and stays flat along it.
fn axis_distance(selected: &[bool], w: u32, h: u32, vertical: bool) -> Vec<f32> {
    let (pw, ph) = ((w + 2) as usize, (h + 2) as usize);
    let mut d = vec![0.0f32; pw * ph];
    let inside = |x: usize, y: usize| -> bool {
        x >= 1 && y >= 1 && x <= w as usize && y <= h as usize && {
            selected[(y - 1) * w as usize + (x - 1)]
        }
    };

    // A sweep out and a sweep back: each texel keeps the shorter run to an
    // edge, which for one axis is all a distance transform needs to be.
    let (outer, inner) = if vertical { (pw, ph) } else { (ph, pw) };
    for a in 0..outer {
        let mut run = 0.0f32;
        for b in 0..inner {
            let (x, y) = if vertical { (a, b) } else { (b, a) };
            run = if inside(x, y) { run + 1.0 } else { 0.0 };
            d[y * pw + x] = run;
        }
        let mut run = 0.0f32;
        for b in (0..inner).rev() {
            let (x, y) = if vertical { (a, b) } else { (b, a) };
            run = if inside(x, y) { run + 1.0 } else { 0.0 };
            let i = y * pw + x;
            d[i] = d[i].min(run);
        }
    }
    d
}

// ------------------------------------------------------------- the selection

/// Every texel the sprite actually covers.
pub fn select_opaque(src: &RgbaImage, w: u32, h: u32) -> Vec<bool> {
    (0..(w * h) as usize)
        .map(|i| {
            let (x, y) = (i as u32 % w, i as u32 / w);
            source_texel(src, x, y, w, h)[3] > 0
        })
        .collect()
}

/// The contiguous run of similar colour under `(x, y)` — the magic wand, and
/// the reason this is worth having for pixel art: a sprite is already drawn in
/// flat regions, so the artist has done the selecting.
pub fn select_similar(
    src: &RgbaImage,
    w: u32,
    h: u32,
    x: u32,
    y: u32,
    tolerance: f32,
) -> Vec<bool> {
    let n = (w * h) as usize;
    let mut out = vec![false; n];
    if x >= w || y >= h {
        return out;
    }
    let seed = source_texel(src, x, y, w, h);
    let cutoff = (tolerance.clamp(0.0, 1.0) * 255.0) as i32;

    let mut stack = vec![(x, y)];
    out[(y * w + x) as usize] = true;
    while let Some((cx, cy)) = stack.pop() {
        for (nx, ny) in neighbours(cx, cy, w, h) {
            let i = (ny * w + nx) as usize;
            if out[i] || !alike(seed, source_texel(src, nx, ny, w, h), cutoff) {
                continue;
            }
            out[i] = true;
            stack.push((nx, ny));
        }
    }
    out
}

fn neighbours(x: u32, y: u32, w: u32, h: u32) -> impl Iterator<Item = (u32, u32)> {
    [(1i32, 0i32), (-1, 0), (0, 1), (0, -1)]
        .into_iter()
        .filter_map(move |(dx, dy)| {
            let (nx, ny) = (x as i32 + dx, y as i32 + dy);
            (nx >= 0 && ny >= 0 && nx < w as i32 && ny < h as i32).then_some((nx as u32, ny as u32))
        })
}

/// Two texels the wand should treat as the same colour. Transparent texels
/// match each other whatever their RGB says: what sits under a fully erased
/// pixel is not a colour anyone chose.
fn alike(a: [u8; 4], b: [u8; 4], cutoff: i32) -> bool {
    if a[3] == 0 || b[3] == 0 {
        return a[3] == b[3];
    }
    (0..3).all(|c| (a[c] as i32 - b[c] as i32).abs() <= cutoff)
}

/// The source pixel under a canvas texel. Nearest neighbour, never averaged:
/// a blend of two neighbouring colours is a colour the artist never used, and
/// the wand would then match neither of them.
pub fn source_texel(src: &RgbaImage, x: u32, y: u32, w: u32, h: u32) -> [u8; 4] {
    let sx = (x as u64 * src.width() as u64 / w.max(1) as u64).min(src.width() as u64 - 1) as u32;
    let sy = (y as u64 * src.height() as u64 / h.max(1) as u64).min(src.height() as u64 - 1) as u32;
    src.get_pixel(sx, sy).0
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::Rgba;

    /// A filled rectangle in the middle of the canvas.
    fn block(w: u32, h: u32, x0: u32, y0: u32, x1: u32, y1: u32) -> Vec<bool> {
        (0..(w * h) as usize)
            .map(|i| {
                let (x, y) = (i as u32 % w, i as u32 / w);
                (x0..x1).contains(&x) && (y0..y1).contains(&y)
            })
            .collect()
    }

    #[test]
    fn distance_grows_towards_the_middle() {
        let sel = block(16, 16, 4, 4, 12, 12);
        let d = euclidean_distance(&sel, 16, 16);
        let at = |x: usize, y: usize| d[(y + 1) * 18 + (x + 1)];
        assert_eq!(at(0, 0), 0.0, "outside the region is the edge itself");
        assert!((at(4, 8) - 1.0).abs() < 1e-3, "one in is one away");
        assert!((at(7, 8) - 4.0).abs() < 1e-3, "four in is four away");
        assert!(at(7, 7) > at(5, 5), "the middle is furthest in");
    }

    /// The point of doing this in edge heights rather than one height per
    /// texel: the rim of a dome has a slope inside a single texel.
    #[test]
    fn a_domes_rim_slopes_within_one_texel() {
        let sel = block(16, 16, 4, 4, 12, 12);
        let cut = Cut {
            shape: Shape::Dome,
            ..Cut::default()
        };
        let out = rasterize(&sel, 16, 16, &cut);
        let [_, left, right, _] = out[(8 * 16 + 4) as usize].expect("the left rim is selected");
        assert!(
            right > left,
            "the left rim should rise inward: left={left} right={right}"
        );
        // And the far rim leans the other way.
        let [_, left, right, _] = out[(8 * 16 + 11) as usize].expect("the right rim is selected");
        assert!(right < left, "the right rim should fall outward");
        assert!(out[0].is_none(), "nothing outside the selection is touched");
    }

    #[test]
    fn a_flat_shape_has_no_slope_anywhere() {
        let sel = block(16, 16, 4, 4, 12, 12);
        let cut = Cut {
            shape: Shape::Flat,
            ..Cut::default()
        };
        for cell in rasterize(&sel, 16, 16, &cut).into_iter().flatten() {
            let [t, l, r, b] = cell;
            assert_eq!((l, b), (r, t), "a plateau is flat by definition");
        }
    }

    /// A tube is round across its axis and dead flat along it. That is the
    /// whole difference between a tube and a dome.
    #[test]
    fn a_tube_is_flat_along_its_axis() {
        let sel = block(16, 16, 2, 6, 14, 10);
        let cut = Cut {
            shape: Shape::TubeX,
            ..Cut::default()
        };
        let out = rasterize(&sel, 16, 16, &cut);
        let a = out[(8 * 16 + 4) as usize].unwrap();
        let b = out[(8 * 16 + 10) as usize].unwrap();
        assert_eq!(a, b, "two texels along the axis are the same height");
        assert_eq!((a[1], a[2]), (a[2], a[1]), "and have no sideways slope");
        // Across the axis it does round over.
        let rim = out[(6 * 16 + 8) as usize].unwrap();
        assert!(rim[3] > rim[0], "the top rim rises inward");
    }

    #[test]
    fn a_bevel_stops_at_its_width() {
        let sel = block(24, 24, 2, 2, 22, 22);
        let cut = Cut {
            shape: Shape::Bevel,
            width: 3.0,
            ..Cut::default()
        };
        let out = rasterize(&sel, 24, 24, &cut);
        let inner = out[(12 * 24 + 12) as usize].unwrap();
        assert_eq!(inner, [255, 255, 255, 255], "past the ramp it is a plateau");
        let ramp = out[(12 * 24 + 3) as usize].unwrap();
        assert!(ramp[2] > ramp[1], "inside the width it still slopes");
    }

    #[test]
    fn concave_turns_the_shape_inside_out() {
        let sel = block(16, 16, 4, 4, 12, 12);
        let out = |concave| {
            let cut = Cut {
                shape: Shape::Dome,
                concave,
                ..Cut::default()
            };
            rasterize(&sel, 16, 16, &cut)[(8 * 16 + 8) as usize].unwrap()[0]
        };
        assert!(out(true) < out(false), "a pit is lower in the middle");
    }

    #[test]
    fn the_wand_stops_at_a_different_colour() {
        let mut img = image::RgbaImage::from_pixel(8, 8, Rgba([10, 20, 30, 255]));
        for y in 0..8 {
            for x in 4..8 {
                img.put_pixel(x, y, Rgba([200, 20, 30, 255]));
            }
        }
        let sel = select_similar(&img, 8, 8, 1, 1, 0.05);
        assert!(sel[8 + 1], "the seed is selected");
        assert!(!sel[8 + 5], "the other colour is not");
        assert_eq!(sel.iter().filter(|&&s| s).count(), 32);
    }

    /// Erased pixels keep whatever RGB was under the eraser, so comparing
    /// their colour would split one transparent surround into several.
    #[test]
    fn the_wand_treats_all_transparent_texels_alike() {
        let mut img = image::RgbaImage::from_pixel(8, 8, Rgba([255, 0, 0, 0]));
        for x in 0..8 {
            img.put_pixel(x, 4, Rgba([0, 255, 0, 0]));
        }
        let sel = select_similar(&img, 8, 8, 0, 0, 0.0);
        assert!(sel.iter().all(|&s| s), "all of it is equally nothing");
    }
}
