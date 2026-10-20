//! Pillow-compatible thumbnailing for RGB analysis images.
//!
//! Pillow's six resize filters are recreated here instead of being mapped onto
//! the (smaller) filter set of the `image` crate, because `BOX` and `HAMMING`
//! have no `image` equivalent and their kernels differ enough to change which
//! straight lines a detector sees.
//!
//! The coordinate and kernel conventions follow Pillow's `Resample.c`:
//!
//! * one output pixel covers `src / dst` input pixels;
//! * the filter support is multiplied by that scale when downscaling
//!   (Pillow's antialias mode), so all source pixels contribute;
//! * weights are sampled at input pixel centres and normalized over the
//!   in-bounds window, without extending the edge pixels;
//! * the horizontal pass is rounded to 8 bits before the vertical pass, just
//!   like Pillow's 8-bit intermediate image;
//! * the final value is clamped and rounded half up.
//!
//! Only up to `1 / 1` scaling is used by [`thumbnail`]: Pillow's thumbnail
//! never enlarges an image.

use image::{DynamicImage, Rgb, RgbImage};

/// Pillow resampling filters that `Image.thumbnail` accepts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Filter {
    Nearest,
    Box,
    Bilinear,
    Hamming,
    Bicubic,
    Lanczos,
}

impl Filter {
    /// Parse a settings/CLI resample name. Unknown names fall back to Lanczos,
    /// which is also the application default. Settings validation rejects
    /// unknown values before they reach this function.
    fn parse(name: &str) -> Self {
        match name.trim().to_ascii_lowercase().as_str() {
            "nearest" => Self::Nearest,
            "box" => Self::Box,
            "bilinear" => Self::Bilinear,
            "hamming" => Self::Hamming,
            "bicubic" => Self::Bicubic,
            _ => Self::Lanczos,
        }
    }

    /// Filter radius in source pixels, before antialias scaling.
    fn support(self) -> f64 {
        match self {
            // Nearest is handled separately and never uses the kernel path.
            Self::Nearest => 0.5,
            Self::Box => 0.5,
            Self::Bilinear | Self::Hamming => 1.0,
            Self::Bicubic => 2.0,
            Self::Lanczos => 3.0,
        }
    }

    /// Kernel evaluated at `x` source pixels from the sample centre.
    fn kernel(self, x: f64) -> f64 {
        match self {
            Self::Nearest => {
                if x > -0.5 && x <= 0.5 {
                    1.0
                } else {
                    0.0
                }
            }
            // Pillow's box filter includes the right edge.
            Self::Box => {
                if x > -0.5 && x <= 0.5 {
                    1.0
                } else {
                    0.0
                }
            }
            Self::Bilinear => {
                let x = x.abs();
                if x < 1.0 { 1.0 - x } else { 0.0 }
            }
            Self::Hamming => {
                let x = x.abs();
                if x >= 1.0 {
                    0.0
                } else {
                    sinc(x) * (0.54 + 0.46 * (std::f64::consts::PI * x).cos())
                }
            }
            // Catmull-Rom cubic convolution (a = -0.5), as used by Pillow.
            Self::Bicubic => {
                let x = x.abs();
                if x < 1.0 {
                    (1.5 * x - 2.5) * x * x + 1.0
                } else if x < 2.0 {
                    ((-0.5 * x + 2.5) * x - 4.0) * x + 2.0
                } else {
                    0.0
                }
            }
            Self::Lanczos => {
                let x = x.abs();
                if x >= 3.0 {
                    0.0
                } else if x == 0.0 {
                    1.0
                } else {
                    sinc(x) * sinc(x / 3.0)
                }
            }
        }
    }
}

fn sinc(x: f64) -> f64 {
    if x == 0.0 {
        1.0
    } else {
        let px = std::f64::consts::PI * x;
        px.sin() / px
    }
}

/// Pillow's `clip8`: clamp, then round half up.
fn clip8(value: f64) -> u8 {
    if value >= 255.0 {
        255
    } else if value < 0.0 {
        0
    } else {
        (value + 0.5) as u8
    }
}

/// Round an aspect-derived coordinate exactly like Pillow's
/// `round_aspect(number, key)`: pick floor or ceiling, preferring floor on a
/// tie, and never return less than one.
fn round_aspect(number: f64, key: impl Fn(i64) -> f64) -> u32 {
    let low = number.floor() as i64;
    let high = number.ceil() as i64;
    let chosen = if key(high) < key(low) { high } else { low };
    chosen.max(1) as u32
}

/// Pillow `Image.thumbnail` target dimensions for a square bound.
///
/// Returns `None` when the image already fits, which also means Pillow will
/// not enlarge it. Otherwise the longest side becomes `size`, the short side
/// is rounded to the nearest aspect-preserving integer.
fn thumbnail_size(width: u32, height: u32, size: u32) -> Option<(u32, u32)> {
    if width == 0 || height == 0 {
        return None;
    }
    if size >= width && size >= height {
        return None;
    }
    let size = size.max(1);
    let bound = size as f64;
    let aspect = width as f64 / height as f64;
    let (mut target_width, mut target_height) = (size, size);
    // Pillow compares `x / y >= width / height` with x == y == size.
    if 1.0 >= aspect {
        target_width = round_aspect(bound * aspect, |n| (aspect - n as f64 / bound).abs());
    } else {
        target_height = round_aspect(bound / aspect, |n| {
            if n == 0 {
                0.0
            } else {
                (aspect - bound / n as f64).abs()
            }
        });
    }
    Some((target_width.max(1), target_height.max(1)))
}

/// One output sample's source window.
struct Coefficients {
    start: usize,
    weights: Vec<f64>,
}

/// Build Pillow-style normalized filter coefficients for one axis.
fn coefficients(source: usize, target: usize, filter: Filter, extent: f64) -> Vec<Coefficients> {
    let scale = extent / target as f64;
    let filter_scale = scale.max(1.0);
    let support = filter.support() * filter_scale;
    let inverse_scale = 1.0 / filter_scale;
    (0..target)
        .map(|index| {
            let center = (index as f64 + 0.5) * scale;
            // Pillow casts the positive offsets to int (truncation) and then
            // clamps to the image. For positive values this is a floor.
            let mut start = (center - support + 0.5) as isize;
            if start < 0 {
                start = 0;
            }
            let mut end = (center + support + 0.5) as isize;
            if end > source as isize {
                end = source as isize;
            }
            if end <= start {
                // Only reachable for degenerate dimensions; keep one sample.
                start = (center as isize).clamp(0, source as isize - 1);
                end = start + 1;
            }
            let mut weights: Vec<f64> = (start..end)
                .map(|pixel| filter.kernel((pixel as f64 - center + 0.5) * inverse_scale))
                .collect();
            let sum: f64 = weights.iter().sum();
            if sum != 0.0 {
                for weight in &mut weights {
                    *weight /= sum;
                }
            }
            Coefficients {
                start: start as usize,
                weights,
            }
        })
        .collect()
}

/// Resample an RGB image to exact dimensions.
fn resize_rgb(
    source: &RgbImage,
    width: u32,
    height: u32,
    filter: Filter,
    extent: (f64, f64),
) -> RgbImage {
    let (source_width, source_height) = source.dimensions();
    if (source_width, source_height) == (width, height) {
        return source.clone();
    }
    if width == 0 || height == 0 {
        return RgbImage::new(width, height);
    }

    if filter == Filter::Nearest {
        // Nearest on the same pixel-centre coordinates as the filtered path:
        // the sample index is floor((out + 0.5) * scale).
        let scale_x = source_width as f64 / width as f64;
        let scale_y = source_height as f64 / height as f64;
        let mut output = RgbImage::new(width, height);
        for y in 0..height {
            let source_y = (((y as f64 + 0.5) * scale_y) as usize).min(source_height as usize - 1);
            for x in 0..width {
                let source_x =
                    (((x as f64 + 0.5) * scale_x) as usize).min(source_width as usize - 1);
                output.put_pixel(x, y, *source.get_pixel(source_x as u32, source_y as u32));
            }
        }
        return output;
    }

    let horizontal = coefficients(source_width as usize, width as usize, filter, extent.0);
    let mut temporary = RgbImage::new(width, source_height);
    for y in 0..source_height {
        for (x, coeff) in horizontal.iter().enumerate() {
            let mut red = 0.0;
            let mut green = 0.0;
            let mut blue = 0.0;
            for (offset, &weight) in coeff.weights.iter().enumerate() {
                let pixel = source.get_pixel((coeff.start + offset) as u32, y).0;
                red += f64::from(pixel[0]) * weight;
                green += f64::from(pixel[1]) * weight;
                blue += f64::from(pixel[2]) * weight;
            }
            temporary.put_pixel(x as u32, y, Rgb([clip8(red), clip8(green), clip8(blue)]));
        }
    }

    let vertical = coefficients(source_height as usize, height as usize, filter, extent.1);
    let mut output = RgbImage::new(width, height);
    for (y, coeff) in vertical.iter().enumerate() {
        for x in 0..width {
            let mut red = 0.0;
            let mut green = 0.0;
            let mut blue = 0.0;
            for (offset, &weight) in coeff.weights.iter().enumerate() {
                let pixel = temporary.get_pixel(x, (coeff.start + offset) as u32).0;
                red += f64::from(pixel[0]) * weight;
                green += f64::from(pixel[1]) * weight;
                blue += f64::from(pixel[2]) * weight;
            }
            output.put_pixel(x, y as u32, Rgb([clip8(red), clip8(green), clip8(blue)]));
        }
    }
    output
}

/// Return an RGB thumbnail no larger than `size` in either dimension.
///
/// This reproduces Pillow's `Image.thumbnail((size, size), filter)`:
/// the image is converted to RGB first, never enlarged, and the short side is
/// rounded to the closest integer that preserves the aspect ratio. `filter`
/// accepts `lanczos`, `bicubic`, `bilinear`, `nearest`, `box` or `hamming`
/// (case-insensitive, matching the Python implementation).
pub fn thumbnail(image: &DynamicImage, size: u32, filter: &str) -> RgbImage {
    // Pillow's estimate() converts to RGB before thumbnailing.
    let rgb = image.to_rgb8();
    let (width, height) = rgb.dimensions();
    let Some((target_width, target_height)) = thumbnail_size(width, height, size) else {
        return rgb;
    };
    if (target_width, target_height) == (width, height) {
        return rgb;
    }
    let filter = Filter::parse(filter);
    // Pillow thumbnail's default reducing_gap=2 first averages integer blocks.
    let fx = (width / target_width / 2).max(1);
    let fy = (height / target_height / 2).max(1);
    if filter != Filter::Nearest && (fx > 1 || fy > 1) {
        let reduced = RgbImage::from_fn(width.div_ceil(fx), height.div_ceil(fy), |x, y| {
            let mut sums = [0_u64; 3];
            let mut count = 0;
            for sy in y * fy..((y + 1) * fy).min(height) {
                for sx in x * fx..((x + 1) * fx).min(width) {
                    let p = rgb.get_pixel(sx, sy);
                    for ch in 0..3 {
                        sums[ch] += u64::from(p[ch]);
                    }
                    count += 1;
                }
            }
            Rgb(sums.map(|sum| ((sum + count / 2) / count) as u8))
        });
        resize_rgb(
            &reduced,
            target_width,
            target_height,
            filter,
            (width as f64 / fx as f64, height as f64 / fy as f64),
        )
    } else {
        resize_rgb(
            &rgb,
            target_width,
            target_height,
            filter,
            (width as f64, height as f64),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{Rgba, RgbaImage};

    fn rgb(width: u32, height: u32, f: impl Fn(u32, u32) -> [u8; 3]) -> DynamicImage {
        let mut image = RgbImage::new(width, height);
        for y in 0..height {
            for x in 0..width {
                image.put_pixel(x, y, Rgb(f(x, y)));
            }
        }
        DynamicImage::ImageRgb8(image)
    }

    #[test]
    fn all_six_filters_scale_and_keep_constant_color() {
        for name in [
            "lanczos", "bicubic", "bilinear", "nearest", "box", "hamming",
        ] {
            let image = rgb(800, 600, |_, _| [10, 20, 30]);
            let thumb = thumbnail(&image, 400, name);
            assert_eq!(thumb.dimensions(), (400, 300), "{name}");
            for pixel in thumb.pixels() {
                assert_eq!(pixel.0, [10, 20, 30], "{name}");
            }
        }
    }

    #[test]
    fn never_upscales_and_rounds_aspect_like_pillow() {
        let small = rgb(320, 200, |_, _| [1, 2, 3]);
        let thumb = thumbnail(&small, 2400, "lanczos");
        assert_eq!(thumb.dimensions(), (320, 200));

        let wide = rgb(1000, 333, |_, _| [1, 2, 3]);
        let thumb = thumbnail(&wide, 400, "lanczos");
        // 400 * 333 / 1000 = 133.2, and 133 rounds closer than 134.
        assert_eq!(thumb.dimensions(), (400, 133));

        let tall = rgb(333, 1000, |_, _| [1, 2, 3]);
        let thumb = thumbnail(&tall, 400, "lanczos");
        assert_eq!(thumb.dimensions(), (133, 400));
    }

    #[test]
    fn filter_kernels_are_distinct_and_correct() {
        assert_eq!(Filter::Box.kernel(0.49), 1.0);
        assert_eq!(Filter::Box.kernel(0.51), 0.0);
        assert_eq!(Filter::Bilinear.kernel(0.5), 0.5);
        assert!((Filter::Hamming.kernel(0.5) - 0.54 * 2.0 / std::f64::consts::PI).abs() < 1e-12);
        assert!((Filter::Bicubic.kernel(0.5) - 0.5625).abs() < 1e-12);
        assert!((Filter::Lanczos.kernel(0.5) - 0.607_927_101_854_026_6).abs() < 1e-12);
        assert_eq!(Filter::Bicubic.kernel(2.0), 0.0);
        assert_eq!(Filter::Lanczos.kernel(3.0), 0.0);
        // The two filters that do not exist in the `image` crate are not
        // aliases for another kernel; box and nearest share a kernel but use
        // it differently (antialiased area sampling vs a single sample).
        assert_ne!(Filter::Hamming.kernel(0.25), Filter::Bilinear.kernel(0.25));
    }

    #[test]
    fn box_downscale_is_an_exact_two_by_two_average() {
        let source = rgb(4, 4, |x, y| {
            let value = if (x < 2) == (y < 2) { 0 } else { 255 };
            [value, value, value]
        });
        let thumb = thumbnail(&source, 2, "box");
        assert_eq!(thumb.dimensions(), (2, 2));
        assert_eq!(thumb.get_pixel(0, 0).0, [0, 0, 0]);
        assert_eq!(thumb.get_pixel(1, 0).0, [255, 255, 255]);
        assert_eq!(thumb.get_pixel(0, 1).0, [255, 255, 255]);
        assert_eq!(thumb.get_pixel(1, 1).0, [0, 0, 0]);

        // Box integrates the whole area while nearest picks one pixel; this is
        // the main behavioural difference between the two 0.5-support kernels.
        let nearest = thumbnail(&source, 1, "nearest");
        let boxed = thumbnail(&source, 1, "box");
        assert_eq!(nearest.get_pixel(0, 0).0, [0, 0, 0]);
        assert_eq!(boxed.get_pixel(0, 0).0, [128, 128, 128]);
    }

    #[test]
    fn thumbnail_drops_alpha_like_convert_rgb() {
        let mut rgba = RgbaImage::from_pixel(4, 4, Rgba([200, 10, 20, 0]));
        rgba.put_pixel(0, 0, Rgba([200, 10, 20, 255]));
        let thumb = thumbnail(&DynamicImage::ImageRgba8(rgba), 2, "bilinear");
        assert_eq!(thumb.get_pixel(0, 0).0, [200, 10, 20]);
    }
}
