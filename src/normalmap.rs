//! Height extraction, filtering and normal-map generation.

use image::{Rgba, RgbaImage};
use rayon::prelude::*;

/// Which part of the source image is treated as height information.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
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
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kernel {
    Sobel,
    Scharr,
    Prewitt,
}

impl Kernel {
    pub const ALL: [Kernel; 3] = [Kernel::Sobel, Kernel::Scharr, Kernel::Prewitt];

    pub fn label(self) -> &'static str {
        match self {
            Kernel::Sobel => "Sobel",
            Kernel::Scharr => "Scharr",
            Kernel::Prewitt => "Prewitt",
        }
    }

    /// Horizontal 3x3 weights, row-major. The vertical kernel is its transpose.
    fn weights(self) -> [f32; 9] {
        match self {
            Kernel::Sobel => [-1.0, 0.0, 1.0, -2.0, 0.0, 2.0, -1.0, 0.0, 1.0],
            Kernel::Scharr => [-3.0, 0.0, 3.0, -10.0, 0.0, 10.0, -3.0, 0.0, 3.0],
            Kernel::Prewitt => [-1.0, 0.0, 1.0, -1.0, 0.0, 1.0, -1.0, 0.0, 1.0],
        }
    }

    /// Scales each kernel so equal strengths give comparable depth.
    fn normalizer(self) -> f32 {
        match self {
            Kernel::Sobel => 1.0 / 4.0,
            Kernel::Scharr => 1.0 / 16.0,
            Kernel::Prewitt => 1.0 / 3.0,
        }
    }
}

/// Every knob the generator exposes to the UI.
#[derive(Clone, Copy, PartialEq, Debug)]
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
        }
    }
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

    HeightMap { width, height, data }
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
        assert!(((a as i32 - 128) - (128 - b as i32)).abs() <= 1, "{a} vs {b}");
        assert!((a as i32 - 128).abs() > 8, "expected a real Y tilt, got {a}");
    }

    #[test]
    fn blur_preserves_dimensions_and_range() {
        let s = Settings { blur: 4.0, ..Settings::default() };
        let hm = height_map(&solid(32, 24, 200), &s);
        assert_eq!((hm.width, hm.height), (32, 24));
        assert!(hm.data.iter().all(|v| (0.0..=1.0).contains(v)));
    }
}
