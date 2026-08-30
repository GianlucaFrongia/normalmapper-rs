//! Height extraction, filtering and normal-map generation.

use image::{Rgba, RgbaImage};
use rayon::prelude::*;
use serde::{Deserialize, Serialize};

/// Which part of the source image is treated as height information.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub enum HeightSource {
    Luminance,
    Average,
    Red,
    Green,
    Blue,
    Alpha,
}

impl HeightSource {
    pub const ALL: [HeightSource; 6] = [
        HeightSource::Luminance,
        HeightSource::Average,
        HeightSource::Red,
        HeightSource::Green,
        HeightSource::Blue,
        HeightSource::Alpha,
    ];

    pub fn label(self) -> &'static str {
        match self {
            HeightSource::Luminance => "Luminance",
            HeightSource::Average => "Average RGB",
            HeightSource::Red => "Red channel",
            HeightSource::Green => "Green channel",
            HeightSource::Blue => "Blue channel",
            HeightSource::Alpha => "Alpha channel",
        }
    }

    fn sample(self, px: &Rgba<u8>) -> f32 {
        let [r, g, b, a] = px.0.map(|c| c as f32 / 255.0);
        match self {
            HeightSource::Luminance => 0.2126 * r + 0.7152 * g + 0.0722 * b,
            HeightSource::Average => (r + g + b) / 3.0,
            HeightSource::Red => r,
            HeightSource::Green => g,
            HeightSource::Blue => b,
            HeightSource::Alpha => a,
        }
    }
}

/// Edge-detection kernel used to differentiate the height field.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub enum Kernel {
    Sobel,
    Scharr,
    Prewitt,
    /// Central difference over a single texel — no cross-row smoothing.
    Pixel,
}

impl Kernel {
    pub const ALL: [Kernel; 4] = [
        Kernel::Sobel,
        Kernel::Scharr,
        Kernel::Prewitt,
        Kernel::Pixel,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Kernel::Sobel => "Sobel",
            Kernel::Scharr => "Scharr",
            Kernel::Prewitt => "Prewitt",
            Kernel::Pixel => "1 px (pixel art)",
        }
    }

    /// Horizontal 3x3 weights, row-major. The vertical kernel is its transpose.
    fn weights(self) -> [f32; 9] {
        match self {
            Kernel::Sobel => [-1.0, 0.0, 1.0, -2.0, 0.0, 2.0, -1.0, 0.0, 1.0],
            Kernel::Scharr => [-3.0, 0.0, 3.0, -10.0, 0.0, 10.0, -3.0, 0.0, 3.0],
            Kernel::Prewitt => [-1.0, 0.0, 1.0, -1.0, 0.0, 1.0, -1.0, 0.0, 1.0],
            // Only the centre row: a hard edge stays one pixel wide.
            Kernel::Pixel => [0.0, 0.0, 0.0, -1.0, 0.0, 1.0, 0.0, 0.0, 0.0],
        }
    }

    /// Scales each kernel so equal strengths give comparable depth.
    fn normalizer(self) -> f32 {
        match self {
            Kernel::Sobel => 1.0 / 4.0,
            Kernel::Scharr => 1.0 / 16.0,
            Kernel::Prewitt => 1.0 / 3.0,
            Kernel::Pixel => 1.0 / 2.0,
        }
    }
}

/// Every knob the generator exposes to the UI.
#[derive(Clone, Copy, PartialEq, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub source: HeightSource,
    pub kernel: Kernel,
    /// Bump depth multiplier.
    pub strength: f32,
    /// Gaussian pre-blur radius in pixels; 0 disables it.
    pub blur: f32,
    /// Contrast applied to the height field around 0.5.
    pub contrast: f32,
    pub invert_height: bool,
    pub flip_x: bool,
    /// Off = OpenGL (+Y up), on = DirectX (+Y down).
    pub flip_y: bool,
    /// Sample across the borders so the result tiles seamlessly.
    pub tileable: bool,

    /// Horizon search radius in pixels for ambient occlusion; 0 disables it.
    pub ao_radius: f32,
    /// How much of the computed occlusion to keep.
    pub ao_strength: f32,

    /// Roughness value for perfectly smooth areas.
    pub roughness_base: f32,
    /// How strongly fine height detail pushes roughness up.
    pub roughness_detail: f32,
    pub roughness_invert: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            source: HeightSource::Luminance,
            kernel: Kernel::Sobel,
            strength: 2.0,
            blur: 0.0,
            contrast: 1.0,
            invert_height: false,
            flip_x: false,
            flip_y: false,
            tileable: false,
            ao_radius: 8.0,
            ao_strength: 1.0,
            roughness_base: 0.4,
            roughness_detail: 8.0,
            roughness_invert: false,
        }
    }
}

/// One of the texture maps the generator can produce.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize, Deserialize)]
pub enum MapKind {
    Height,
    Normal,
    Ao,
    Roughness,
}

impl MapKind {
    pub const ALL: [MapKind; 4] = [
        MapKind::Height,
        MapKind::Normal,
        MapKind::Ao,
        MapKind::Roughness,
    ];

    pub fn label(self) -> &'static str {
        match self {
            MapKind::Height => "Height",
            MapKind::Normal => "Normal",
            MapKind::Ao => "AO",
            MapKind::Roughness => "Roughness",
        }
    }

    /// Filename suffix used by the exporters, e.g. `brick_normal.png`.
    pub fn suffix(self) -> &'static str {
        match self {
            MapKind::Height => "height",
            MapKind::Normal => "normal",
            MapKind::Ao => "ao",
            MapKind::Roughness => "roughness",
        }
    }
}

/// Render one map from an already-extracted height field.
pub fn render(hm: &HeightMap, settings: &Settings, kind: MapKind) -> RgbaImage {
    match kind {
        MapKind::Height => hm.to_rgba(),
        MapKind::Normal => normal_map(hm, settings),
        MapKind::Ao => ambient_occlusion(hm, settings),
        MapKind::Roughness => roughness(hm, settings),
    }
}

/// Extract the height field once and render every requested map from it.
pub fn generate(
    src: &RgbaImage,
    settings: &Settings,
    kinds: &[MapKind],
) -> Vec<(MapKind, RgbaImage)> {
    let hm = height_map(src, settings);
    kinds
        .iter()
        .map(|&k| (k, render(&hm, settings, k)))
        .collect()
}

/// A single-channel f32 image.
pub struct HeightMap {
    pub width: u32,
    pub height: u32,
    pub data: Vec<f32>,
}

impl HeightMap {
    #[inline]
    fn get(&self, x: i32, y: i32, tileable: bool) -> f32 {
        let (w, h) = (self.width as i32, self.height as i32);
        let (x, y) = if tileable {
            (x.rem_euclid(w), y.rem_euclid(h))
        } else {
            (x.clamp(0, w - 1), y.clamp(0, h - 1))
        };
        self.data[(y * w + x) as usize]
    }

    /// Grayscale RGBA rendering, for the height preview tab.
    pub fn to_rgba(&self) -> RgbaImage {
        let mut out = RgbaImage::new(self.width, self.height);
        for (px, &v) in out.pixels_mut().zip(self.data.iter()) {
            let c = (v.clamp(0.0, 1.0) * 255.0).round() as u8;
            *px = Rgba([c, c, c, 255]);
        }
        out
    }
}

/// Extract the height field from `src` and apply contrast, inversion and blur.
pub fn height_map(src: &RgbaImage, settings: &Settings) -> HeightMap {
    let (width, height) = src.dimensions();
    let mut data: Vec<f32> = src
        .pixels()
        .map(|px| {
            let mut v = settings.source.sample(px);
            if settings.invert_height {
                v = 1.0 - v;
            }
            if (settings.contrast - 1.0).abs() > f32::EPSILON {
                v = ((v - 0.5) * settings.contrast + 0.5).clamp(0.0, 1.0);
            }
            v
        })
        .collect();

    if settings.blur > 0.0 {
        data = gaussian_blur(&data, width, height, settings.blur, settings.tileable);
    }

    HeightMap {
        width,
        height,
        data,
    }
}

/// Turn a height field into a tangent-space normal map.
pub fn normal_map(hm: &HeightMap, settings: &Settings) -> RgbaImage {
    let (w, h) = (hm.width, hm.height);
    let kernel = settings.kernel.weights();
    let norm = settings.kernel.normalizer();
    let sx = if settings.flip_x { -1.0 } else { 1.0 };
    let sy = if settings.flip_y { -1.0 } else { 1.0 };
    let strength = settings.strength.max(0.001);

    let mut buf = vec![0u8; w as usize * h as usize * 4];
    buf.par_chunks_mut(w as usize * 4)
        .enumerate()
        .for_each(|(y, row)| {
            for x in 0..w as usize {
                let mut dx = 0.0;
                let mut dy = 0.0;
                for j in 0..3usize {
                    for i in 0..3usize {
                        let s = hm.get(
                            x as i32 + i as i32 - 1,
                            y as i32 + j as i32 - 1,
                            settings.tileable,
                        );
                        // The vertical kernel is the transpose of the horizontal one.
                        dx += s * kernel[j * 3 + i];
                        dy += s * kernel[i * 3 + j];
                    }
                }
                dx *= norm * strength;
                dy *= norm * strength;

                let (nx, ny, nz) = (-dx * sx, -dy * sy, 1.0);
                let len = (nx * nx + ny * ny + nz * nz).sqrt();

                let o = x * 4;
                row[o] = encode(nx / len);
                row[o + 1] = encode(ny / len);
                row[o + 2] = encode(nz / len);
                row[o + 3] = 255;
            }
        });

    RgbaImage::from_raw(w, h, buf).expect("buffer matches dimensions")
}

/// Horizon-based ambient occlusion over the height field.
///
/// For each pixel we march outwards in eight directions, track the steepest
/// slope seen (the horizon angle), and treat its sine as the fraction of that
/// direction's sky that is blocked.
pub fn ambient_occlusion(hm: &HeightMap, settings: &Settings) -> RgbaImage {
    const DIRS: usize = 8;
    let (w, h) = (hm.width, hm.height);
    let radius = settings.ao_radius;
    if radius <= 0.0 || settings.ao_strength <= 0.0 {
        return RgbaImage::from_pixel(w, h, Rgba([255, 255, 255, 255]));
    }

    let steps = (radius.ceil() as i32).clamp(1, 48);
    let step_len = radius / steps as f32;
    let depth = settings.strength.max(0.001);
    let dirs: Vec<(f32, f32)> = (0..DIRS)
        .map(|d| {
            let a = d as f32 * std::f32::consts::TAU / DIRS as f32;
            (a.cos(), a.sin())
        })
        .collect();

    let mut buf = vec![0u8; w as usize * h as usize * 4];
    buf.par_chunks_mut(w as usize * 4)
        .enumerate()
        .for_each(|(y, row)| {
            for x in 0..w as usize {
                let h0 = hm.get(x as i32, y as i32, settings.tileable);
                let mut occ = 0.0;
                for &(cs, sn) in &dirs {
                    let mut horizon = 0.0f32;
                    for s in 1..=steps {
                        let dist = s as f32 * step_len;
                        let sx = (x as f32 + cs * dist).round() as i32;
                        let sy = (y as f32 + sn * dist).round() as i32;
                        let dh = (hm.get(sx, sy, settings.tileable) - h0) * depth;
                        horizon = horizon.max(dh / dist);
                    }
                    // sin(atan(slope)) — the blocked share of that direction.
                    occ += horizon / (horizon * horizon + 1.0).sqrt();
                }
                let ao = (1.0 - (occ / DIRS as f32) * settings.ao_strength).clamp(0.0, 1.0);
                let c = (ao * 255.0).round() as u8;
                let o = x * 4;
                row[o] = c;
                row[o + 1] = c;
                row[o + 2] = c;
                row[o + 3] = 255;
            }
        });

    RgbaImage::from_raw(w, h, buf).expect("buffer matches dimensions")
}

/// Radius of the low-pass used to isolate fine detail for the roughness map.
const ROUGHNESS_HIGHPASS: f32 = 3.0;

/// Roughness from local height detail: flat areas keep the base value, busy
/// areas are pushed towards 1.
pub fn roughness(hm: &HeightMap, settings: &Settings) -> RgbaImage {
    let (w, h) = (hm.width, hm.height);
    let smooth = gaussian_blur(&hm.data, w, h, ROUGHNESS_HIGHPASS, settings.tileable);

    let mut out = RgbaImage::new(w, h);
    for (px, (&v, &s)) in out.pixels_mut().zip(hm.data.iter().zip(smooth.iter())) {
        let detail = (v - s).abs() * settings.roughness_detail;
        let mut r = (settings.roughness_base + detail).clamp(0.0, 1.0);
        if settings.roughness_invert {
            r = 1.0 - r;
        }
        let c = (r * 255.0).round() as u8;
        *px = Rgba([c, c, c, 255]);
    }
    out
}

#[inline]
fn encode(v: f32) -> u8 {
    ((v * 0.5 + 0.5).clamp(0.0, 1.0) * 255.0).round() as u8
}

/// Separable Gaussian blur over a single-channel f32 buffer.
fn gaussian_blur(data: &[f32], w: u32, h: u32, radius: f32, tileable: bool) -> Vec<f32> {
    let sigma = radius.max(0.01);
    let r = (sigma * 3.0).ceil() as i32;
    let raw: Vec<f32> = (-r..=r)
        .map(|i| (-((i * i) as f32) / (2.0 * sigma * sigma)).exp())
        .collect();
    let sum: f32 = raw.iter().sum();
    let kernel: Vec<f32> = raw.iter().map(|k| k / sum).collect();

    let horizontal = blur_pass(data, w, h, &kernel, r, true, tileable);
    blur_pass(&horizontal, w, h, &kernel, r, false, tileable)
}

fn blur_pass(
    data: &[f32],
    w: u32,
    h: u32,
    kernel: &[f32],
    r: i32,
    horizontal: bool,
    tileable: bool,
) -> Vec<f32> {
    let (wi, hi) = (w as i32, h as i32);
    let mut out = vec![0.0f32; data.len()];
    out.par_chunks_mut(w as usize)
        .enumerate()
        .for_each(|(y, row)| {
            for x in 0..wi {
                let mut acc = 0.0;
                for (k, weight) in kernel.iter().enumerate() {
                    let off = k as i32 - r;
                    let (mut sx, mut sy) = if horizontal {
                        (x + off, y as i32)
                    } else {
                        (x, y as i32 + off)
                    };
                    if tileable {
                        sx = sx.rem_euclid(wi);
                        sy = sy.rem_euclid(hi);
                    } else {
                        sx = sx.clamp(0, wi - 1);
                        sy = sy.clamp(0, hi - 1);
                    }
                    acc += data[(sy * wi + sx) as usize] * weight;
                }
                row[x as usize] = acc;
            }
        });
    out
}

// ------------------------------------------------------------- lit preview

/// The maps a lit preview needs, all at the same (small) resolution.
pub struct Shading {
    pub width: u32,
    pub height: u32,
    pub albedo: RgbaImage,
    pub normal: RgbaImage,
    pub ao: RgbaImage,
    pub roughness: RgbaImage,
}

/// A single directional light, plus the ambient term.
#[derive(Clone, Copy, Debug)]
pub struct Light {
    /// Direction towards the light, in tangent space (+Y up, +Z out of screen).
    pub dir: [f32; 3],
    pub ambient: f32,
    pub specular: f32,
    /// Light the source image, rather than a neutral grey.
    pub use_albedo: bool,
    /// The normal map was generated with a flipped green channel.
    pub flip_y: bool,
}

impl Default for Light {
    fn default() -> Self {
        Self {
            dir: [-0.4, 0.5, 0.75],
            ambient: 0.25,
            specular: 0.3,
            use_albedo: true,
            flip_y: false,
        }
    }
}

/// Blinn-Phong shade the generated maps so the bumps can actually be judged.
pub fn shade(s: &Shading, light: &Light) -> RgbaImage {
    let (w, h) = (s.width, s.height);
    let l = normalize(light.dir);
    // The viewer looks straight down -Z, so the half-vector is cheap.
    let half = normalize([l[0], l[1], l[2] + 1.0]);
    let gy = if light.flip_y { -1.0 } else { 1.0 };

    let mut buf = vec![0u8; w as usize * h as usize * 4];
    buf.par_chunks_mut(w as usize * 4)
        .enumerate()
        .for_each(|(y, row)| {
            for x in 0..w as usize {
                let i = y * w as usize + x;
                let np = s.normal.as_raw();
                let n = normalize([
                    np[i * 4] as f32 / 127.5 - 1.0,
                    (np[i * 4 + 1] as f32 / 127.5 - 1.0) * gy,
                    np[i * 4 + 2] as f32 / 127.5 - 1.0,
                ]);
                let ao = s.ao.as_raw()[i * 4] as f32 / 255.0;
                let rough = s.roughness.as_raw()[i * 4] as f32 / 255.0;

                let diffuse = dot(n, l).max(0.0);
                // Rough surfaces get a wide, weak highlight; smooth ones a tight one.
                let shininess = 2.0 + (1.0 - rough).powi(2) * 128.0;
                let spec = dot(n, half).max(0.0).powf(shininess) * light.specular * (1.0 - rough);

                let o = x * 4;
                for c in 0..3 {
                    let albedo = if light.use_albedo {
                        s.albedo.as_raw()[i * 4 + c] as f32 / 255.0
                    } else {
                        0.8
                    };
                    let v = albedo * (light.ambient * ao + diffuse * ao) + spec;
                    row[o + c] = (v.clamp(0.0, 1.0) * 255.0).round() as u8;
                }
                row[o + 3] = 255;
            }
        });

    RgbaImage::from_raw(w, h, buf).expect("buffer matches dimensions")
}

#[inline]
fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

#[inline]
fn normalize(v: [f32; 3]) -> [f32; 3] {
    let len = dot(v, v).sqrt();
    if len > f32::EPSILON {
        [v[0] / len, v[1] / len, v[2] / len]
    } else {
        [0.0, 0.0, 1.0]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn solid(w: u32, h: u32, v: u8) -> RgbaImage {
        RgbaImage::from_pixel(w, h, Rgba([v, v, v, 255]))
    }

    #[test]
    fn flat_height_gives_flat_normals() {
        let s = Settings::default();
        let hm = height_map(&solid(16, 16, 128), &s);
        let nm = normal_map(&hm, &s);
        for px in nm.pixels() {
            assert_eq!(px.0, [128, 128, 255, 255]);
        }
    }

    #[test]
    fn horizontal_ramp_tilts_x_only() {
        // Height rising to the right: normals lean -X, Y stays neutral.
        let mut img = RgbaImage::new(8, 8);
        for (x, _y, px) in img.enumerate_pixels_mut() {
            let v = (x * 32).min(255) as u8;
            *px = Rgba([v, v, v, 255]);
        }
        let s = Settings::default();
        let nm = normal_map(&height_map(&img, &s), &s);
        let px = nm.get_pixel(4, 4).0;
        assert!(px[0] < 120, "expected -X tilt, got {px:?}");
        assert_eq!(px[1], 128);
    }

    #[test]
    fn flip_y_mirrors_green() {
        let mut img = RgbaImage::new(8, 8);
        for (_x, y, px) in img.enumerate_pixels_mut() {
            let v = (y * 32).min(255) as u8;
            *px = Rgba([v, v, v, 255]);
        }
        let gl = Settings::default();
        let dx = Settings { flip_y: true, ..gl };
        let a = normal_map(&height_map(&img, &gl), &gl).get_pixel(4, 4).0[1];
        let b = normal_map(&height_map(&img, &dx), &dx).get_pixel(4, 4).0[1];
        // Equal and opposite, up to one step of rounding.
        assert!(
            ((a as i32 - 128) - (128 - b as i32)).abs() <= 1,
            "{a} vs {b}"
        );
        assert!(
            (a as i32 - 128).abs() > 8,
            "expected a real Y tilt, got {a}"
        );
    }

    #[test]
    fn flat_surface_is_unoccluded() {
        let s = Settings::default();
        let hm = height_map(&solid(24, 24, 90), &s);
        for px in ambient_occlusion(&hm, &s).pixels() {
            assert_eq!(px.0, [255, 255, 255, 255]);
        }
    }

    #[test]
    fn a_pit_is_darker_than_its_rim() {
        // A single deep hole in the middle of a flat plate.
        let mut img = solid(32, 32, 255);
        for y in 14..18 {
            for x in 14..18 {
                img.put_pixel(x, y, Rgba([0, 0, 0, 255]));
            }
        }
        let s = Settings {
            ao_radius: 10.0,
            ..Settings::default()
        };
        let ao = ambient_occlusion(&height_map(&img, &s), &s);
        assert!(
            ao.get_pixel(16, 16).0[0] < ao.get_pixel(1, 1).0[0],
            "the pit floor should be occluded"
        );
    }

    #[test]
    fn roughness_tracks_detail() {
        let s = Settings::default();
        // Checkerboard noise on the left half, flat on the right.
        let mut img = solid(32, 16, 128);
        for y in 0..16 {
            for x in 0..16 {
                let v = if (x + y) % 2 == 0 { 0 } else { 255 };
                img.put_pixel(x, y, Rgba([v, v, v, 255]));
            }
        }
        let r = roughness(&height_map(&img, &s), &s);
        assert!(r.get_pixel(8, 8).0[0] > r.get_pixel(28, 8).0[0]);
    }

    #[test]
    fn pixel_kernel_keeps_edges_one_texel_wide() {
        // A single bright column: only its two neighbours should tilt.
        let mut img = solid(9, 3, 0);
        for y in 0..3 {
            img.put_pixel(4, y, Rgba([255, 255, 255, 255]));
        }
        let s = Settings {
            kernel: Kernel::Pixel,
            ..Settings::default()
        };
        let nm = normal_map(&height_map(&img, &s), &s);
        assert_eq!(
            nm.get_pixel(4, 1).0[0],
            128,
            "the lit column itself is flat"
        );
        assert!(nm.get_pixel(3, 1).0[0] < 120);
        assert!(nm.get_pixel(5, 1).0[0] > 136);
        assert_eq!(nm.get_pixel(1, 1).0[0], 128, "no bleed two texels out");
    }

    #[test]
    fn light_facing_the_normal_is_brightest() {
        let flat = RgbaImage::from_pixel(4, 4, Rgba([128, 128, 255, 255]));
        let s = Shading {
            width: 4,
            height: 4,
            albedo: solid(4, 4, 255),
            normal: flat,
            ao: solid(4, 4, 255),
            roughness: solid(4, 4, 255),
        };
        let head_on = shade(
            &s,
            &Light {
                dir: [0.0, 0.0, 1.0],
                specular: 0.0,
                ..Light::default()
            },
        );
        let grazing = shade(
            &s,
            &Light {
                dir: [1.0, 0.0, 0.1],
                specular: 0.0,
                ..Light::default()
            },
        );
        assert!(head_on.get_pixel(2, 2).0[0] > grazing.get_pixel(2, 2).0[0]);
    }

    #[test]
    fn settings_round_trip_through_json() {
        let s = Settings {
            blur: 2.5,
            ao_radius: 12.0,
            tileable: true,
            ..Settings::default()
        };
        let json = serde_json::to_string(&s).unwrap();
        assert_eq!(serde_json::from_str::<Settings>(&json).unwrap(), s);
    }

    #[test]
    fn blur_preserves_dimensions_and_range() {
        let s = Settings {
            blur: 4.0,
            ..Settings::default()
        };
        let hm = height_map(&solid(32, 24, 200), &s);
        assert_eq!((hm.width, hm.height), (32, 24));
        assert!(hm.data.iter().all(|v| (0.0..=1.0).contains(v)));
    }
}
