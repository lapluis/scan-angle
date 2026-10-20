//! Image loading, skew estimation and preview rendering.
//!
//! The Python implementation used Pillow for decoding/EXIF handling and
//! OpenCV's LSD for detection. This module keeps the same observable
//! behaviour with pure Rust crates:
//!
//! * `lsdetect` (a pure-Rust LSD implementation) supplies line segments;
//!   [`crate::detection::estimate_segments`] performs the same filtering,
//!   merging and consensus logic as the Python detector;
//! * [`crate::resample::thumbnail`] recreates Pillow's six resize filters and
//!   its no-upscale / aspect-rounding thumbnail rules;
//! * multi-page TIFFs are streamed page by page with the `tiff` crate so that
//!   orientation and sample depth can be handled per page, while animated
//!   WebP and APNG files are iterated with the `image` decoder;
//! * on Windows, TIFF pages whose colour layout the Rust decoder rejects
//!   (palette/fax and similar) are retried through WIC, with the page's Exif
//!   orientation applied afterwards;
//! * all eight EXIF orientations are applied to still and animated images.
//!
//! Pillow's decompression-bomb hard limit (`2 * MAX_IMAGE_PIXELS`, i.e.
//! 178,956,970 pixels) is enforced unless the caller explicitly opts into
//! large images.

use std::fs::File;
use std::io::{BufRead, BufReader, Read, Seek};
use std::path::Path;

use anyhow::{Result, bail};
use image::{
    AnimationDecoder, DynamicImage, ImageBuffer, ImageDecoder, ImageFormat, ImageReader, Rgb,
    RgbImage,
};
use tiff::ColorType as TiffColorType;
use tiff::decoder::{Decoder as TiffDecoder, DecodingResult, Limits as TiffLimits};
use tiff::tags::Tag;

use crate::detection::{DetectionSettings, Estimate, Line, estimate_segments};
use crate::resample::thumbnail;

/// Pillow's decompression-bomb hard limit (`2 * MAX_IMAGE_PIXELS`).
pub const MAX_PIXELS: u64 = 178_956_970;

/// Preview colors, matching the Python implementation.
const MERGED_COLOR: [u8; 3] = [30, 120, 255];
const SELECTED_COLOR: [u8; 3] = [255, 30, 40];

/// Estimate the skew angle of an image.
///
/// The image is converted to RGB, thumbnailed to `settings.analysis_size`
/// (never enlarged), converted to BT.601 grayscale, and passed to the pure
/// Rust LSD implementation. The returned tuple mirrors the Python
/// implementation: the estimate, the RGB analysis thumbnail (call
/// [`draw_preview`] with it), and the selected evidence lines.
pub fn estimate(
    image: &DynamicImage,
    settings: &DetectionSettings,
) -> Result<(Estimate, RgbImage, Vec<Line>)> {
    settings.validate()?;
    let small = thumbnail(image, settings.analysis_size, &settings.resample);
    let (width, height) = small.dimensions();
    if width == 0 || height == 0 {
        bail!("图片尺寸无效: {width}x{height}");
    }

    let gray = bt601_gray(&small);
    let segments = lsdetect::detect(&gray, width as usize, height as usize, lsdetect::SCALE)?;
    let coords: Vec<[f64; 4]> = segments
        .iter()
        .map(|segment| [segment.x1, segment.y1, segment.x2, segment.y2])
        .collect();

    let (result, selected) = estimate_segments(&coords, height, width, settings)?;
    Ok((result, small, selected))
}

/// Convert RGB pixels to BT.601 luma (the same weights as
/// `cv2.COLOR_RGB2GRAY` / Pillow's `L` conversion).
fn bt601_gray(image: &RgbImage) -> Vec<u8> {
    image
        .pixels()
        .map(|pixel| {
            let [red, green, blue] = pixel.0;
            ((4899 * u32::from(red) + 9617 * u32::from(green) + 1868 * u32::from(blue) + 8192)
                >> 14) as u8
        })
        .collect()
}

/// Draw all merged lines blue (width 1), then selected evidence red (width 3).
pub fn draw_preview(image: &RgbImage, merged: &[Line], selected: &[Line]) -> RgbImage {
    let mut preview = image.clone();
    for line in merged {
        draw_line(&mut preview, &line.coords, MERGED_COLOR, 1);
    }
    for line in selected {
        draw_line(&mut preview, &line.coords, SELECTED_COLOR, 3);
    }
    preview
}

/// Rasterize one line with a square brush. Pillow's preview layer is
/// deliberately simple (one-pixel merged lines, three-pixel selected lines),
/// so a Bresenham centre line with a square brush reproduces the visible
/// result without pulling in a drawing crate.
fn draw_line(image: &mut RgbImage, coords: &[f64; 4], color: [u8; 3], width: u32) {
    if !coords.iter().all(|value| value.is_finite()) {
        return;
    }
    let mut x0 = coords[0].round() as i64;
    let mut y0 = coords[1].round() as i64;
    let x1 = coords[2].round() as i64;
    let y1 = coords[3].round() as i64;
    let dx = (x1 - x0).abs();
    let sx = if x0 < x1 { 1 } else { -1 };
    let dy = -(y1 - y0).abs();
    let sy = if y0 < y1 { 1 } else { -1 };
    let mut error = dx + dy;
    let radius = (width.saturating_sub(1) / 2) as i64;
    loop {
        paint_point(image, x0, y0, radius, color);
        if x0 == x1 && y0 == y1 {
            break;
        }
        let doubled = 2 * error;
        if doubled >= dy {
            error += dy;
            x0 += sx;
        }
        if doubled <= dx {
            error += dx;
            y0 += sy;
        }
    }
}

fn paint_point(image: &mut RgbImage, x: i64, y: i64, radius: i64, color: [u8; 3]) {
    let (width, height) = (image.width() as i64, image.height() as i64);
    for py in (y - radius).max(0)..=(y + radius).min(height - 1) {
        for px in (x - radius).max(0)..=(x + radius).min(width - 1) {
            image.put_pixel(px as u32, py as u32, Rgb(color));
        }
    }
}

fn check_pixel_limit(width: u32, height: u32, allow_large: bool) -> Result<()> {
    let pixels = u64::from(width) * u64::from(height);
    if !allow_large && pixels > MAX_PIXELS {
        bail!(
            "图片尺寸 {width}x{height}（{pixels} 像素）超过像素上限 {MAX_PIXELS}；\
             如确认为可信的本地扫描图，请允许处理大图"
        );
    }
    Ok(())
}

fn format_error(error: impl std::fmt::Display) -> anyhow::Error {
    anyhow::anyhow!("{error}")
}

/// Visit every frame/page in an image file, using one-based page numbers.
///
/// Multi-page TIFFs are decoded incrementally (one page is never required to
/// fit the whole document in memory). Animated WebP and APNG files yield one
/// call per animation frame; every other supported still format yields a
/// single page. EXIF orientation is applied before the frame is handed to
/// `visitor`.
///
/// Per-page decode failures (including the pixel limit and unsupported TIFF
/// sample layouts) are reported through `visitor(page, Err(..))` so a batch
/// can record that page and continue with the rest of the document. Errors
/// that prevent enumerating pages at all (file open, unrecognised format,
/// broken TIFF directory chain) are returned instead.
pub fn visit_frames(
    path: &Path,
    allow_large: bool,
    visitor: &mut dyn FnMut(usize, Result<DynamicImage>),
) -> Result<()> {
    let file = File::open(path)?;
    let reader = ImageReader::new(BufReader::new(file))
        .with_guessed_format()
        .map_err(format_error)?;
    let format = reader
        .format()
        .ok_or_else(|| anyhow::anyhow!("无法识别的图片格式"))?;

    match format {
        ImageFormat::Tiff => visit_tiff(path, reader.into_inner(), allow_large, visitor),
        ImageFormat::Png => visit_png(reader.into_inner(), allow_large, visitor),
        ImageFormat::WebP => visit_webp(reader.into_inner(), allow_large, visitor),
        _ => visit_still(reader, allow_large, visitor),
    }
}

/// Static formats (JPEG, BMP, ...), plus PNG/WebP when they are not animated.
fn visit_still<R: BufRead + Seek>(
    mut reader: ImageReader<R>,
    allow_large: bool,
    visitor: &mut dyn FnMut(usize, Result<DynamicImage>),
) -> Result<()> {
    reader.no_limits();
    let decoder = reader.into_decoder().map_err(format_error)?;
    let (width, height) = decoder.dimensions();
    if let Err(error) = check_pixel_limit(width, height, allow_large) {
        visitor(1, Err(error));
        return Ok(());
    }

    let mut decoder = decoder;
    // A malformed EXIF block should not make an otherwise decodable image
    // disappear; Pillow likewise treats a missing tag as "no transform".
    let orientation = decoder
        .orientation()
        .unwrap_or(image::metadata::Orientation::NoTransforms);
    match DynamicImage::from_decoder(decoder) {
        Ok(mut image) => {
            image.apply_orientation(orientation);
            visitor(1, Ok(image));
        }
        Err(error) => visitor(1, Err(format_error(error))),
    }
    Ok(())
}

fn visit_png(
    reader: BufReader<File>,
    allow_large: bool,
    visitor: &mut dyn FnMut(usize, Result<DynamicImage>),
) -> Result<()> {
    use image::codecs::png::PngDecoder;

    let mut decoder = PngDecoder::new(reader).map_err(format_error)?;
    let (width, height) = decoder.dimensions();
    if let Err(error) = check_pixel_limit(width, height, allow_large) {
        visitor(1, Err(error));
        return Ok(());
    }
    let orientation = decoder
        .orientation()
        .unwrap_or(image::metadata::Orientation::NoTransforms);

    if decoder.is_apng().map_err(format_error)? {
        let frames = decoder.apng().map_err(format_error)?.into_frames();
        for (index, frame) in frames.enumerate() {
            let page = index + 1;
            match frame {
                Ok(frame) => {
                    let mut image = DynamicImage::ImageRgba8(frame.into_buffer());
                    image.apply_orientation(orientation);
                    visitor(page, Ok(image));
                }
                Err(error) => visitor(page, Err(format_error(error))),
            }
        }
    } else {
        match DynamicImage::from_decoder(decoder) {
            Ok(mut image) => {
                image.apply_orientation(orientation);
                visitor(1, Ok(image));
            }
            Err(error) => visitor(1, Err(format_error(error))),
        }
    }
    Ok(())
}

fn visit_webp(
    reader: BufReader<File>,
    allow_large: bool,
    visitor: &mut dyn FnMut(usize, Result<DynamicImage>),
) -> Result<()> {
    use image::codecs::webp::WebPDecoder;

    let mut decoder = WebPDecoder::new(reader).map_err(format_error)?;
    let (width, height) = decoder.dimensions();
    if let Err(error) = check_pixel_limit(width, height, allow_large) {
        visitor(1, Err(error));
        return Ok(());
    }
    let orientation = decoder
        .orientation()
        .unwrap_or(image::metadata::Orientation::NoTransforms);

    if decoder.has_animation() {
        for (index, frame) in decoder.into_frames().enumerate() {
            let page = index + 1;
            match frame {
                Ok(frame) => {
                    let mut image = DynamicImage::ImageRgba8(frame.into_buffer());
                    image.apply_orientation(orientation);
                    visitor(page, Ok(image));
                }
                Err(error) => visitor(page, Err(format_error(error))),
            }
        }
    } else {
        match DynamicImage::from_decoder(decoder) {
            Ok(mut image) => {
                image.apply_orientation(orientation);
                visitor(1, Ok(image));
            }
            Err(error) => visitor(1, Err(format_error(error))),
        }
    }
    Ok(())
}

/// Stream a multi-page TIFF one directory at a time.
fn visit_tiff<R: Read + Seek>(
    path: &Path,
    reader: R,
    allow_large: bool,
    visitor: &mut dyn FnMut(usize, Result<DynamicImage>),
) -> Result<()> {
    #[cfg(not(windows))]
    let _ = path;
    let mut decoder = TiffDecoder::new(reader).map_err(format_error)?;
    let mut limits = if allow_large {
        TiffLimits::unlimited()
    } else {
        TiffLimits::default()
    };
    // The explicit pixel check is the intended guard; do not let the tiff
    // crate's default 256 MiB buffer cap reject an allowed 16-bit page.
    limits.decoding_buffer_size = usize::MAX;
    limits.intermediate_buffer_size = if allow_large {
        usize::MAX
    } else {
        512 * 1024 * 1024
    };
    decoder = decoder.with_limits(limits);

    let mut page = 1;
    loop {
        let decoded = decode_tiff_page(&mut decoder, allow_large);
        match decoded {
            Ok(image) => visitor(page, Ok(image)),
            Err(error) => {
                // On Windows, WIC covers TIFF colour layouts (palette, fax,
                // ...) that the pure-Rust decoder rejects. Exif orientation is
                // not applied by WIC, so read it from the page and apply it.
                #[cfg(windows)]
                {
                    match tiff_wic_fallback(path, page, allow_large) {
                        Ok(image) => visitor(page, Ok(image)),
                        Err(fallback_error) => visitor(
                            page,
                            Err(anyhow::anyhow!(
                                "{error}（Windows 备选解码失败: {fallback_error}）"
                            )),
                        ),
                    }
                }
                #[cfg(not(windows))]
                visitor(page, Err(error));
            }
        }
        if !decoder.more_images() {
            break;
        }
        page += 1;
        if let Err(error) = decoder.next_image() {
            visitor(page, Err(format_error(error)));
            break;
        }
    }
    Ok(())
}

/// Windows fallback for TIFF colour layouts the Rust decoder rejects.
#[cfg(windows)]
fn tiff_wic_fallback(path: &Path, page: usize, allow_large: bool) -> Result<DynamicImage> {
    let mut image = crate::wic::decode_page(path, page, allow_large)?;
    if let Some(orientation) = tiff_page_orientation(path, page) {
        image.apply_orientation(orientation);
    }
    Ok(image)
}

/// Read the Exif orientation of one TIFF page without decoding its pixels.
#[cfg(windows)]
fn tiff_page_orientation(path: &Path, page: usize) -> Option<image::metadata::Orientation> {
    let file = File::open(path).ok()?;
    let mut decoder = TiffDecoder::new(BufReader::new(file)).ok()?;
    decoder.seek_to_image(page.checked_sub(1)?).ok()?;
    decoder
        .find_tag_unsigned::<u16>(Tag::Orientation)
        .ok()
        .flatten()
        .and_then(|value| image::metadata::Orientation::from_exif(value.min(255) as u8))
}

fn decode_tiff_page<R: Read + Seek>(
    decoder: &mut TiffDecoder<R>,
    allow_large: bool,
) -> Result<DynamicImage> {
    let (width, height) = decoder.dimensions().map_err(format_error)?;
    check_pixel_limit(width, height, allow_large)?;

    let orientation = decoder
        .find_tag_unsigned::<u16>(Tag::Orientation)
        .ok()
        .flatten()
        .and_then(|value| image::metadata::Orientation::from_exif(value.min(255) as u8))
        .unwrap_or(image::metadata::Orientation::NoTransforms);

    let color = decoder
        .colortype()
        .map_err(|error| anyhow::anyhow!("不支持的 TIFF 颜色类型: {error}"))?;

    // `Decoder::read_image` reads only the first plane for planar data, which
    // would silently drop channels. Reject that layout instead.
    let planar = decoder
        .find_tag_unsigned::<u16>(Tag::PlanarConfiguration)
        .map_err(format_error)?
        .unwrap_or(1);
    if planar == 2 && color.num_samples() > 1 {
        bail!("不支持的 TIFF planar（分平面）多通道数据");
    }
    // The tiff crate does not interpret FillOrder; reject the (rare) reversed
    // bit order for packed sub-byte samples instead of expanding them wrongly.
    let fill_order = decoder
        .find_tag_unsigned::<u16>(Tag::FillOrder)
        .map_err(format_error)?
        .unwrap_or(1);
    if fill_order != 1 && color.bit_depth() < 8 {
        bail!("不支持的 TIFF FillOrder={fill_order}（仅支持最高位优先）");
    }
    ensure_supported_tiff_color(&color)?;

    let data = decoder.read_image().map_err(format_error)?;
    let mut image = tiff_to_dynamic(width, height, color, data)?;
    image.apply_orientation(orientation);
    Ok(image)
}

fn checked_pixels(width: u32, height: u32) -> Result<usize> {
    let pixels = u64::from(width)
        .checked_mul(u64::from(height))
        .ok_or_else(|| anyhow::anyhow!("图片尺寸溢出"))?;
    usize::try_from(pixels).map_err(|_| anyhow::anyhow!("图片尺寸超出平台限制"))
}

fn truncate_to<T>(mut data: Vec<T>, length: usize, what: &str) -> Result<Vec<T>> {
    if data.len() < length {
        bail!(
            "{what}数据长度不足（需要 {length} 个样本，实际 {}）",
            data.len()
        );
    }
    data.truncate(length);
    Ok(data)
}

/// Scale an unsigned sample of `bits` bits to a full 16-bit sample.
fn scale_to_u16(value: u16, bits: u8) -> u16 {
    if bits == 16 {
        value
    } else {
        let maximum = (1u32 << bits) - 1;
        ((u32::from(value) * 65_535 + maximum / 2) / maximum) as u16
    }
}

fn gray8_from_packed(width: u32, height: u32, bits: u8, data: Vec<u8>) -> Result<DynamicImage> {
    let (width_usize, height_usize) = (width as usize, height as usize);
    let row_bytes = (width_usize * bits as usize).div_ceil(8);
    if data.len() < row_bytes * height_usize {
        bail!(
            "TIFF 灰度数据长度不足（位深 {bits}，需要 {} 字节，实际 {}）",
            row_bytes * height_usize,
            data.len()
        );
    }
    let maximum = (1u32 << bits) - 1;
    let mut expanded = Vec::with_capacity(width_usize * height_usize);
    for y in 0..height_usize {
        let row = &data[y * row_bytes..(y + 1) * row_bytes];
        for x in 0..width_usize {
            let bit_offset = x * bits as usize;
            let mut value = 0u32;
            for bit in 0..bits as usize {
                let position = bit_offset + bit;
                let set = (row[position / 8] >> (7 - position % 8)) & 1;
                value = (value << 1) | u32::from(set);
            }
            expanded.push(((value * 255 + maximum / 2) / maximum) as u8);
        }
    }
    let buffer = ImageBuffer::from_raw(width, height, expanded)
        .ok_or_else(|| anyhow::anyhow!("TIFF 灰度数据长度与尺寸不符"))?;
    Ok(DynamicImage::ImageLuma8(buffer))
}

/// Reject layouts before decoding so unsupported colour types never reach the
/// pixel conversion and can never be silently misinterpreted.
fn ensure_supported_tiff_color(color: &TiffColorType) -> Result<()> {
    let supported = match *color {
        // Packed 1/2/4-bit gray samples are expanded by gray8_from_packed.
        TiffColorType::Gray(bits) => (1..=16).contains(&bits),
        TiffColorType::GrayA(bits) => bits == 8 || (9..=16).contains(&bits),
        TiffColorType::RGB(bits) | TiffColorType::RGBA(bits) | TiffColorType::CMYK(bits) => {
            bits == 8 || (9..=16).contains(&bits)
        }
        TiffColorType::Palette(_)
        | TiffColorType::CMYKA(_)
        | TiffColorType::YCbCr(_)
        | TiffColorType::Lab(_)
        | TiffColorType::Multiband { .. } => false,
        _ => false,
    };
    if supported {
        Ok(())
    } else {
        bail!(
            "不支持的 TIFF 颜色类型 {color:?}；仅支持灰度、灰度+Alpha、RGB、RGBA 和 CMYK（CMYKA、调色板、YCbCr、Lab 与多波段均不支持）"
        )
    }
}

fn cmyk8_to_rgb(cmyk: &[u8]) -> [u8; 3] {
    let black = 1.0 - f32::from(cmyk[3]) / 255.0;
    [
        ((255.0 - f32::from(cmyk[0])) * black) as u8,
        ((255.0 - f32::from(cmyk[1])) * black) as u8,
        ((255.0 - f32::from(cmyk[2])) * black) as u8,
    ]
}

fn cmyk16_to_rgb(cmyk: &[u16]) -> [u16; 3] {
    let black = 1.0 - f32::from(cmyk[3]) / 65_535.0;
    [
        ((65_535.0 - f32::from(cmyk[0])) * black) as u16,
        ((65_535.0 - f32::from(cmyk[1])) * black) as u16,
        ((65_535.0 - f32::from(cmyk[2])) * black) as u16,
    ]
}

/// Convert a decoded TIFF page to a `DynamicImage`.
///
/// Supported layouts: 8/16-bit RGB, RGBA, grayscale, grayscale+alpha, CMYK
/// (converted to RGB), and 1/2/4/8/16-bit grayscale (packed samples expanded).
/// Anything else -- including palette, YCbCr, Lab, multiband and CMYKA -- is
/// rejected with an explicit error instead of being misinterpreted.
fn tiff_to_dynamic(
    width: u32,
    height: u32,
    color: TiffColorType,
    data: DecodingResult,
) -> Result<DynamicImage> {
    let pixels = checked_pixels(width, height)?;
    match color {
        TiffColorType::Gray(bits) => match data {
            DecodingResult::U8(values) if bits == 8 => {
                let values = truncate_to(values, pixels, "TIFF 灰度")?;
                let buffer = ImageBuffer::from_raw(width, height, values)
                    .ok_or_else(|| anyhow::anyhow!("TIFF 灰度数据长度与尺寸不符"))?;
                Ok(DynamicImage::ImageLuma8(buffer))
            }
            DecodingResult::U8(values) if (1..8).contains(&bits) => {
                gray8_from_packed(width, height, bits, values)
            }
            DecodingResult::U16(values) if (9..=16).contains(&bits) => {
                let values = truncate_to(values, pixels, "TIFF 灰度")?;
                let values = values
                    .into_iter()
                    .map(|value| value.min(255) as u8)
                    .collect();
                let buffer = ImageBuffer::from_raw(width, height, values)
                    .ok_or_else(|| anyhow::anyhow!("TIFF 灰度数据长度与尺寸不符"))?;
                Ok(DynamicImage::ImageLuma8(buffer))
            }
            other => bail!(
                "不支持的 TIFF 灰度采样布局：位深 {bits}，样本类型 {}",
                decoding_kind(&other)
            ),
        },
        TiffColorType::GrayA(bits) => match data {
            DecodingResult::U8(values) if bits == 8 => {
                let values = truncate_to(values, pixels * 2, "TIFF 灰度+Alpha")?;
                let buffer = ImageBuffer::from_raw(width, height, values)
                    .ok_or_else(|| anyhow::anyhow!("TIFF 灰度+Alpha 数据长度与尺寸不符"))?;
                Ok(DynamicImage::ImageLumaA8(buffer))
            }
            DecodingResult::U16(values) if (9..=16).contains(&bits) => {
                let values = truncate_to(values, pixels * 2, "TIFF 灰度+Alpha")?;
                let values = values
                    .into_iter()
                    .map(|value| scale_to_u16(value, bits))
                    .collect();
                let buffer = ImageBuffer::from_raw(width, height, values)
                    .ok_or_else(|| anyhow::anyhow!("TIFF 灰度+Alpha 数据长度与尺寸不符"))?;
                Ok(DynamicImage::ImageLumaA16(buffer))
            }
            other => bail!(
                "不支持的 TIFF 灰度+Alpha 采样布局：位深 {bits}，样本类型 {}",
                decoding_kind(&other)
            ),
        },
        TiffColorType::RGB(bits) => match data {
            DecodingResult::U8(values) if bits == 8 => {
                let values = truncate_to(values, pixels * 3, "TIFF RGB")?;
                let buffer = ImageBuffer::from_raw(width, height, values)
                    .ok_or_else(|| anyhow::anyhow!("TIFF RGB 数据长度与尺寸不符"))?;
                Ok(DynamicImage::ImageRgb8(buffer))
            }
            DecodingResult::U16(values) if (9..=16).contains(&bits) => {
                let values = truncate_to(values, pixels * 3, "TIFF RGB")?;
                let values = values
                    .into_iter()
                    .map(|value| scale_to_u16(value, bits))
                    .collect();
                let buffer = ImageBuffer::from_raw(width, height, values)
                    .ok_or_else(|| anyhow::anyhow!("TIFF RGB 数据长度与尺寸不符"))?;
                Ok(DynamicImage::ImageRgb16(buffer))
            }
            other => bail!(
                "不支持的 TIFF RGB 采样布局：位深 {bits}，样本类型 {}",
                decoding_kind(&other)
            ),
        },
        TiffColorType::RGBA(bits) => match data {
            DecodingResult::U8(values) if bits == 8 => {
                let values = truncate_to(values, pixels * 4, "TIFF RGBA")?;
                let buffer = ImageBuffer::from_raw(width, height, values)
                    .ok_or_else(|| anyhow::anyhow!("TIFF RGBA 数据长度与尺寸不符"))?;
                Ok(DynamicImage::ImageRgba8(buffer))
            }
            DecodingResult::U16(values) if (9..=16).contains(&bits) => {
                let values = truncate_to(values, pixels * 4, "TIFF RGBA")?;
                let values = values
                    .into_iter()
                    .map(|value| scale_to_u16(value, bits))
                    .collect();
                let buffer = ImageBuffer::from_raw(width, height, values)
                    .ok_or_else(|| anyhow::anyhow!("TIFF RGBA 数据长度与尺寸不符"))?;
                Ok(DynamicImage::ImageRgba16(buffer))
            }
            other => bail!(
                "不支持的 TIFF RGBA 采样布局：位深 {bits}，样本类型 {}",
                decoding_kind(&other)
            ),
        },
        TiffColorType::CMYK(bits) => match data {
            DecodingResult::U8(values) if bits == 8 => {
                let values = truncate_to(values, pixels * 4, "TIFF CMYK")?;
                let mut rgb = Vec::with_capacity(pixels * 3);
                for cmyk in values.as_chunks::<4>().0 {
                    rgb.extend_from_slice(&cmyk8_to_rgb(cmyk));
                }
                let buffer = ImageBuffer::from_raw(width, height, rgb)
                    .ok_or_else(|| anyhow::anyhow!("TIFF CMYK 数据长度与尺寸不符"))?;
                Ok(DynamicImage::ImageRgb8(buffer))
            }
            DecodingResult::U16(values) if (9..=16).contains(&bits) => {
                let values = truncate_to(values, pixels * 4, "TIFF CMYK")?;
                // Normalize the source depth first; cmyk16_to_rgb already
                // produces a full 16-bit result.
                let values: Vec<u16> = values
                    .into_iter()
                    .map(|value| scale_to_u16(value, bits))
                    .collect();
                let mut rgb = Vec::with_capacity(pixels * 3);
                for cmyk in values.as_chunks::<4>().0 {
                    rgb.extend_from_slice(&cmyk16_to_rgb(cmyk));
                }
                let buffer = ImageBuffer::from_raw(width, height, rgb)
                    .ok_or_else(|| anyhow::anyhow!("TIFF CMYK 数据长度与尺寸不符"))?;
                Ok(DynamicImage::ImageRgb16(buffer))
            }
            other => bail!(
                "不支持的 TIFF CMYK 采样布局：位深 {bits}，样本类型 {}",
                decoding_kind(&other)
            ),
        },
        other => bail!(
            "不支持的 TIFF 颜色类型 {other:?}；仅支持灰度、灰度+Alpha、RGB、RGBA 和 CMYK（CMYKA、调色板、YCbCr、Lab 与多波段均不支持）"
        ),
    }
}

fn decoding_kind(data: &DecodingResult) -> &'static str {
    match data {
        DecodingResult::U8(_) => "U8",
        DecodingResult::U16(_) => "U16",
        DecodingResult::U32(_) => "U32",
        DecodingResult::U64(_) => "U64",
        DecodingResult::F16(_) => "F16",
        DecodingResult::F32(_) => "F32",
        DecodingResult::F64(_) => "F64",
        DecodingResult::I8(_) => "I8",
        DecodingResult::I16(_) => "I16",
        DecodingResult::I32(_) => "I32",
        DecodingResult::I64(_) => "I64",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::File;
    use std::io::{BufWriter, Seek, Write};

    use image::GenericImageView;
    use tiff::encoder::TiffEncoder;
    use tiff::tags::Tag as TiffTag;

    fn synthetic_rectangle(width: u32, height: u32) -> DynamicImage {
        let background = Rgb([223, 238, 251]);
        let mut image = RgbImage::from_pixel(width, height, background);
        // White interior with a strong five-pixel black border.
        for y in 100..(height - 100) {
            for x in 100..(width - 100) {
                image.put_pixel(x, y, Rgb([255, 255, 255]));
            }
        }
        for x in 100..(width - 100) {
            for offset in 0..5 {
                image.put_pixel(x, 100 + offset, Rgb([0, 0, 0]));
                image.put_pixel(x, height - 101 - offset, Rgb([0, 0, 0]));
            }
        }
        for y in 100..(height - 100) {
            for offset in 0..5 {
                image.put_pixel(100 + offset, y, Rgb([0, 0, 0]));
                image.put_pixel(width - 101 - offset, y, Rgb([0, 0, 0]));
            }
        }
        DynamicImage::ImageRgb8(image)
    }

    fn sample_line(x1: f64, y1: f64, x2: f64, y2: f64) -> Line {
        Line {
            coords: [x1, y1, x2, y2],
            length: (x2 - x1).hypot(y2 - y1),
            support_length: (x2 - x1).hypot(y2 - y1),
            span_length: (x2 - x1).hypot(y2 - y1),
            gap_length: 0.0,
            angle: 0.0,
            member_count: 1,
            orientation: crate::detection::Orientation::Horizontal,
        }
    }

    #[test]
    fn preview_layers_match_the_python_colors() {
        let original = RgbImage::from_pixel(100, 100, Rgb([255, 255, 255]));
        let merged = vec![
            sample_line(10.0, 20.0, 90.0, 20.0),
            sample_line(10.0, 60.0, 90.0, 60.0),
        ];
        let selected = vec![merged[0].clone()];
        let preview = draw_preview(&original, &merged, &selected);
        assert_eq!(preview.get_pixel(50, 20).0, SELECTED_COLOR);
        assert_eq!(preview.get_pixel(50, 60).0, MERGED_COLOR);
        assert_eq!(preview.get_pixel(50, 40).0, [255, 255, 255]);
        // The source image is never modified.
        assert_eq!(original.get_pixel(50, 20).0, [255, 255, 255]);
    }

    #[test]
    fn estimate_finds_a_straight_rectangle_and_never_upscales() {
        let image = synthetic_rectangle(1000, 1200);
        let settings = DetectionSettings::default();
        let (result, small, lines) = estimate(&image, &settings).unwrap();
        assert_eq!(small.dimensions(), (1000, 1200));
        assert!(result.angle.is_some());
        assert!(result.angle.unwrap().abs() < 0.15);
        assert_eq!(result.status, "可信");
        assert!(!lines.is_empty());
        assert_eq!(result.count, lines.len());

        let bounds = DetectionSettings {
            analysis_size: 400,
            ..DetectionSettings::default()
        };
        let (_, small, _) = estimate(&image, &bounds).unwrap();
        // Pillow's thumbnail bound is square: the 1200 px side becomes 400
        // and the 1000 px side becomes round(1000 * 400 / 1200) = 333.
        assert_eq!(small.dimensions(), (333, 400));

        let tiny = DynamicImage::ImageRgb8(RgbImage::from_pixel(320, 200, Rgb([1, 2, 3])));
        let (_, small, _) = estimate(&tiny, &DetectionSettings::default()).unwrap();
        assert_eq!(small.dimensions(), (320, 200));
    }

    #[test]
    fn bt601_gray_uses_luma_weights() {
        let mut image = RgbImage::new(3, 1);
        image.put_pixel(0, 0, Rgb([255, 0, 0]));
        image.put_pixel(1, 0, Rgb([0, 255, 0]));
        image.put_pixel(2, 0, Rgb([0, 0, 255]));
        let gray = bt601_gray(&image);
        assert_eq!(gray[0], 76);
        assert_eq!(gray[1], 150);
        assert_eq!(gray[2], 29);
    }

    #[allow(clippy::too_many_arguments)]
    fn write_tiff_page<W: Write + Seek>(
        encoder: &mut TiffEncoder<W>,
        data: &[u8],
        width: u32,
        height: u32,
        bits: &[u16],
        samples: u16,
        photometric: u16,
        orientation: u16,
    ) -> tiff::TiffResult<()> {
        let mut directory = encoder.image_directory()?;
        directory.write_tag(TiffTag::ImageWidth, width)?;
        directory.write_tag(TiffTag::ImageLength, height)?;
        directory.write_tag(TiffTag::BitsPerSample, bits)?;
        directory.write_tag(TiffTag::Compression, 1u16)?;
        directory.write_tag(TiffTag::PhotometricInterpretation, photometric)?;
        directory.write_tag(TiffTag::Orientation, orientation)?;
        directory.write_tag(TiffTag::SamplesPerPixel, samples)?;
        directory.write_tag(TiffTag::RowsPerStrip, height)?;
        directory.write_tag(TiffTag::PlanarConfiguration, 1u16)?;
        if photometric == 6 {
            directory.write_tag(TiffTag::ChromaSubsampling, &[1u16, 1u16][..])?;
        }
        let offset = directory.write_data(data)?;
        directory.write_tag(TiffTag::StripOffsets, offset as u32)?;
        directory.write_tag(TiffTag::StripByteCounts, data.len() as u32)?;
        directory.finish()
    }

    #[test]
    fn visit_frames_streams_multipage_tiff_with_per_page_orientation() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("pages.tiff");
        let file = File::create(&path).unwrap();
        let mut encoder = TiffEncoder::new(BufWriter::new(file)).unwrap();
        // Page 1: 4x2, EXIF orientation 6 (rotate 90 clockwise) -> 2x4.
        write_tiff_page(&mut encoder, &[0, 1, 2, 3, 4, 5, 6, 7], 4, 2, &[8], 1, 1, 6).unwrap();
        // Page 2: ordinary 3x2 grayscale.
        write_tiff_page(&mut encoder, &[10, 11, 12, 13, 14, 15], 3, 2, &[8], 1, 1, 1).unwrap();
        // Each image_directory() was finished above; dropping the encoder
        // flushes the buffered writer.
        drop(encoder);

        let mut frames = Vec::new();
        visit_frames(&path, false, &mut |page, result| {
            frames.push((page, result.unwrap()));
        })
        .unwrap();
        assert_eq!(frames.len(), 2);
        assert_eq!(frames[0].0, 1);
        assert_eq!(frames[0].1.dimensions(), (2, 4));
        let rotated = frames[0].1.to_luma8();
        assert_eq!(rotated.get_pixel(0, 0).0, [4]);
        assert_eq!(frames[1].0, 2);
        assert_eq!(frames[1].1.dimensions(), (3, 2));
        assert_eq!(frames[1].1.to_luma8().get_pixel(2, 1).0, [15]);
    }

    #[test]
    fn visit_frames_expands_sixteen_bit_gray() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("sixteen.tiff");
        let file = File::create(&path).unwrap();
        let mut encoder = TiffEncoder::new(BufWriter::new(file)).unwrap();
        let samples: [u16; 4] = [0, 12_345, 40_000, 65_535];
        let bytes: Vec<u8> = samples
            .iter()
            .flat_map(|value| value.to_ne_bytes())
            .collect();
        write_tiff_page(&mut encoder, &bytes, 2, 2, &[16], 1, 1, 1).unwrap();
        // Each image_directory() was finished above; dropping the encoder
        // flushes the buffered writer.
        drop(encoder);

        let mut images = Vec::new();
        visit_frames(&path, false, &mut |_, result| {
            images.push(result.unwrap());
        })
        .unwrap();
        assert_eq!(images.len(), 1);
        match &images[0] {
            DynamicImage::ImageLuma8(buffer) => {
                assert_eq!(buffer.get_pixel(1, 0).0, [255]);
                assert_eq!(buffer.get_pixel(0, 1).0, [255]);
            }
            other => panic!(
                "expected Pillow-compatible clipped grayscale, got {:?}",
                other.color()
            ),
        }
    }

    #[test]
    fn visit_frames_expands_one_bit_gray() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("one-bit.tiff");
        let file = File::create(&path).unwrap();
        let mut encoder = TiffEncoder::new(BufWriter::new(file)).unwrap();
        // 8x2, one bit per sample, rows padded to a byte.
        write_tiff_page(
            &mut encoder,
            &[0b1010_0000, 0b0000_0101],
            8,
            2,
            &[1],
            1,
            1,
            1,
        )
        .unwrap();
        // Each image_directory() was finished above; dropping the encoder
        // flushes the buffered writer.
        drop(encoder);

        let mut images = Vec::new();
        visit_frames(&path, false, &mut |_, result| {
            images.push(result.unwrap());
        })
        .unwrap();
        let gray = images[0].to_luma8();
        assert_eq!(gray.dimensions(), (8, 2));
        assert_eq!(gray.get_pixel(0, 0).0, [255]);
        assert_eq!(gray.get_pixel(1, 0).0, [0]);
        assert_eq!(gray.get_pixel(2, 0).0, [255]);
        assert_eq!(gray.get_pixel(5, 1).0, [255]);
        assert_eq!(gray.get_pixel(7, 1).0, [255]);
    }

    #[test]
    fn tiff_conversion_rejects_unsupported_color_types() {
        // The pure-Rust conversion must never reinterpret YCbCr/Lab/palette
        // samples as RGB. (On Windows the file-level fallback may decode some
        // of these correctly through WIC, but this function never guesses.)
        assert!(ensure_supported_tiff_color(&TiffColorType::YCbCr(8)).is_err());
        assert!(ensure_supported_tiff_color(&TiffColorType::Lab(8)).is_err());
        assert!(ensure_supported_tiff_color(&TiffColorType::Palette(8)).is_err());
        assert!(ensure_supported_tiff_color(&TiffColorType::CMYKA(8)).is_err());
        assert!(
            tiff_to_dynamic(
                2,
                1,
                TiffColorType::YCbCr(8),
                DecodingResult::U8(vec![0; 6])
            )
            .is_err()
        );
    }

    #[test]
    fn visit_frames_rejects_images_over_the_pixel_limit() {
        let directory = tempfile::tempdir().unwrap();

        // 200 million pixels exceeds Pillow's hard limit.
        let too_large = directory.path().join("large.tiff");
        let file = File::create(&too_large).unwrap();
        let mut encoder = TiffEncoder::new(BufWriter::new(file)).unwrap();
        write_tiff_page(&mut encoder, &[0u8], 20_000, 10_000, &[8], 1, 1, 1).unwrap();
        // Each image_directory() was finished above; dropping the encoder
        // flushes the buffered writer.
        drop(encoder);
        let mut errors = Vec::new();
        visit_frames(&too_large, false, &mut |_, result| {
            errors.push(result.err().map(|error| error.to_string()));
        })
        .unwrap();
        assert_eq!(errors.len(), 1);
        assert!(errors[0].as_ref().unwrap().contains("178956970"));
    }

    #[test]
    fn visit_frames_reads_still_png_as_page_one() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("still.png");
        let image = DynamicImage::ImageRgb8(RgbImage::from_pixel(12, 9, Rgb([7, 8, 9])));
        image.save(&path).unwrap();

        let mut frames = Vec::new();
        visit_frames(&path, false, &mut |page, result| {
            frames.push((page, result.unwrap()));
        })
        .unwrap();
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0].0, 1);
        assert_eq!(frames[0].1.dimensions(), (12, 9));
        assert_eq!(frames[0].1.to_rgb8().get_pixel(0, 0).0, [7, 8, 9]);
    }

    #[test]
    fn visit_frames_reports_an_error_for_an_unreadable_file() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("broken.png");
        std::fs::write(&path, b"not an image").unwrap();
        let called = std::cell::Cell::new(false);
        let result = visit_frames(&path, false, &mut |_, _| called.set(true));
        assert!(result.is_err());
        assert!(!called.get());
    }
}
