use crate::diagnostics::hydrate_session_bus_env;
use crate::x11_display::{is_native_x11_session, with_x11_display, X11_CAPTURE_TIMEOUT};
use anyhow::{bail, Context, Result};
use base64::{engine::general_purpose::STANDARD, Engine};
use image::codecs::jpeg::JpegEncoder;
use image::imageops::FilterType;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use std::io::Cursor;

const MAX_SCREENSHOT_SOURCE_PIXELS: u64 = 128 * 1024 * 1024;

pub const DEFAULT_SCREENSHOT_MAX_DIMENSION: u32 = 1920;
pub const DEFAULT_SCREENSHOT_MAX_BYTES: usize = 2 * 1024 * 1024;
pub const ABSOLUTE_SCREENSHOT_MAX_DIMENSION: u32 = 4096;
pub const ABSOLUTE_SCREENSHOT_MAX_BYTES: usize = 4 * 1024 * 1024;
pub const DEFAULT_SCREENSHOT_JPEG_QUALITY: u8 = 80;
pub const MIN_SCREENSHOT_JPEG_QUALITY: u8 = 1;
pub const MAX_SCREENSHOT_JPEG_QUALITY: u8 = 95;
const MIN_SCREENSHOT_MAX_BYTES: usize = 1024;

#[derive(Debug, Clone)]
pub struct RawScreenshotCapture {
    pub mime_type: String,
    pub bytes: Vec<u8>,
    pub source: String,
    pub width: u32,
    pub height: u32,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct ScreenshotCapture {
    pub mime_type: String,
    pub data_url: String,
    pub source: String,
    /// Width of the returned image payload.
    pub width: u32,
    /// Height of the returned image payload.
    pub height: u32,
    /// Coordinate-space width before payload downscaling.
    pub coordinate_width: u32,
    /// Coordinate-space height before payload downscaling.
    pub coordinate_height: u32,
    /// Returned pixels per coordinate-space pixel.
    pub scale: f32,
    pub resized: bool,
    pub bytes: usize,
    pub original_bytes: usize,
    pub max_bytes: usize,
    pub format: ScreenshotOutputFormat,
    pub quality: Option<u8>,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct ScreenshotPayloadOptions {
    pub max_width: Option<u32>,
    pub max_height: Option<u32>,
    pub max_bytes: Option<usize>,
    pub scale: Option<f32>,
    pub format: Option<ScreenshotOutputFormat>,
    pub quality: Option<u8>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum ScreenshotOutputFormat {
    Png,
    Jpeg,
}

impl ScreenshotOutputFormat {
    fn mime_type(self) -> &'static str {
        match self {
            Self::Png => "image/png",
            Self::Jpeg => "image/jpeg",
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct ResolvedScreenshotPayloadOptions {
    max_width: u32,
    max_height: u32,
    max_bytes: usize,
    scale: f32,
    format: ScreenshotOutputFormat,
    quality: u8,
}

impl ScreenshotPayloadOptions {
    fn resolve(self) -> ResolvedScreenshotPayloadOptions {
        let max_width = self
            .max_width
            .unwrap_or(DEFAULT_SCREENSHOT_MAX_DIMENSION)
            .clamp(1, ABSOLUTE_SCREENSHOT_MAX_DIMENSION);
        let max_height = self
            .max_height
            .unwrap_or(DEFAULT_SCREENSHOT_MAX_DIMENSION)
            .clamp(1, ABSOLUTE_SCREENSHOT_MAX_DIMENSION);
        let max_bytes = self
            .max_bytes
            .unwrap_or(DEFAULT_SCREENSHOT_MAX_BYTES)
            .clamp(MIN_SCREENSHOT_MAX_BYTES, ABSOLUTE_SCREENSHOT_MAX_BYTES);
        let scale = self
            .scale
            .filter(|value| value.is_finite() && *value > 0.0)
            .unwrap_or(1.0)
            .min(1.0);
        let format = self.format.unwrap_or(ScreenshotOutputFormat::Png);
        let quality = self
            .quality
            .unwrap_or(DEFAULT_SCREENSHOT_JPEG_QUALITY)
            .clamp(MIN_SCREENSHOT_JPEG_QUALITY, MAX_SCREENSHOT_JPEG_QUALITY);

        ResolvedScreenshotPayloadOptions {
            max_width,
            max_height,
            max_bytes,
            scale,
            format,
            quality,
        }
    }
}

/// Capture the root window of the X display named by `DISPLAY`.
pub async fn capture_screenshot_raw() -> Result<RawScreenshotCapture> {
    hydrate_session_bus_env();
    capture_with_x11().await
}

/// `GetImage` on the root window of a native X11 session. Pixels are device
/// pixels, the space xdotool/XTEST input and X11 window origins use.
async fn capture_with_x11() -> Result<RawScreenshotCapture> {
    if !is_native_x11_session() {
        bail!("no X display: export DISPLAY=:N and XDG_SESSION_TYPE=x11");
    }
    let image = with_x11_display(X11_CAPTURE_TIMEOUT, |display| display.capture_root()).await??;
    let (width, height) = (image.width, image.height);
    let bytes =
        tokio::task::spawn_blocking(move || encode_rgb_png(image.width, image.height, image.rgb))
            .await
            .context("X11 screenshot encoder task failed")??;
    Ok(RawScreenshotCapture {
        mime_type: "image/png".to_string(),
        bytes,
        source: "x11".to_string(),
        width,
        height,
    })
}

fn encode_rgb_png(width: u32, height: u32, rgb: Vec<u8>) -> Result<Vec<u8>> {
    let buffer = image::RgbImage::from_raw(width, height, rgb)
        .context("X11 root image did not match its dimensions")?;
    let mut out = Vec::new();
    image::DynamicImage::ImageRgb8(buffer)
        .write_to(&mut Cursor::new(&mut out), image::ImageFormat::Png)
        .context("failed to encode X11 screenshot PNG")?;
    Ok(out)
}

pub async fn capture_screenshot() -> Result<ScreenshotCapture> {
    let _pipeline = screenshot_pipeline_permit().await?;
    let raw = capture_screenshot_raw().await?;
    run_image_task(move || prepare_screenshot_payload(raw, ScreenshotPayloadOptions::default()))
        .await
}

pub(crate) async fn screenshot_pipeline_permit() -> Result<tokio::sync::SemaphorePermit<'static>> {
    static CAPTURE_PIPELINE: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(1);
    Ok(CAPTURE_PIPELINE.acquire().await?)
}

pub(crate) async fn run_image_task<T: Send + 'static>(
    work: impl FnOnce() -> Result<T> + Send + 'static,
) -> Result<T> {
    static IMAGE_WORKER: tokio::sync::Semaphore = tokio::sync::Semaphore::const_new(1);
    let permit = IMAGE_WORKER.acquire().await?;
    tokio::task::spawn_blocking(move || {
        let _permit = permit;
        work()
    })
    .await
    .context("screenshot processing task failed")?
}

pub fn prepare_screenshot_payload(
    raw: RawScreenshotCapture,
    options: ScreenshotPayloadOptions,
) -> Result<ScreenshotCapture> {
    if raw.bytes.is_empty() {
        bail!("screenshot file was empty");
    }
    let (coordinate_width, coordinate_height) = png_dimensions(&raw.bytes)?;
    let original_bytes = raw.bytes.len();
    let options = options.resolve();
    let (target_width, target_height) =
        target_dimensions(coordinate_width, coordinate_height, options);

    let (bytes, width, height) = if options.format == ScreenshotOutputFormat::Png
        && target_width == coordinate_width
        && target_height == coordinate_height
        && original_bytes <= options.max_bytes
    {
        (raw.bytes, coordinate_width, coordinate_height)
    } else {
        encode_screenshot_to_fit_bytes(
            &raw.bytes,
            coordinate_width,
            coordinate_height,
            target_width,
            target_height,
            options,
        )?
    };

    let encoded = STANDARD.encode(&bytes);
    let scale = if coordinate_width == 0 {
        1.0
    } else {
        width as f32 / coordinate_width as f32
    };

    Ok(ScreenshotCapture {
        mime_type: options.format.mime_type().to_string(),
        data_url: format!("data:{};base64,{encoded}", options.format.mime_type()),
        source: raw.source,
        width,
        height,
        coordinate_width,
        coordinate_height,
        scale,
        resized: width != coordinate_width || height != coordinate_height,
        bytes: bytes.len(),
        original_bytes,
        max_bytes: options.max_bytes,
        format: options.format,
        quality: (options.format == ScreenshotOutputFormat::Jpeg).then_some(options.quality),
    })
}

fn target_dimensions(
    width: u32,
    height: u32,
    options: ResolvedScreenshotPayloadOptions,
) -> (u32, u32) {
    let width_scale = options.max_width as f64 / width as f64;
    let height_scale = options.max_height as f64 / height as f64;
    let scale = f64::from(options.scale)
        .min(width_scale)
        .min(height_scale)
        .min(1.0);

    let target_width = ((width as f64 * scale).round() as u32).clamp(1, width);
    let target_height = ((height as f64 * scale).round() as u32).clamp(1, height);
    (target_width, target_height)
}

fn encode_screenshot_to_fit_bytes(
    raw: &[u8],
    original_width: u32,
    original_height: u32,
    mut target_width: u32,
    mut target_height: u32,
    options: ResolvedScreenshotPayloadOptions,
) -> Result<(Vec<u8>, u32, u32)> {
    let img = image::load_from_memory_with_format(raw, image::ImageFormat::Png)
        .context("failed to decode screenshot PNG for encoding")?;

    loop {
        let bytes = if options.format == ScreenshotOutputFormat::Png
            && target_width == original_width
            && target_height == original_height
        {
            raw.to_vec()
        } else {
            let output = if target_width == original_width && target_height == original_height {
                img.clone()
            } else {
                img.resize_exact(target_width, target_height, FilterType::Lanczos3)
            };
            encode_image(&output, options)?
        };

        if bytes.len() <= options.max_bytes {
            return Ok((bytes, target_width, target_height));
        }

        if target_width == 1 && target_height == 1 {
            bail!(
                "screenshot payload is {} bytes at 1x1, over max_bytes {}",
                bytes.len(),
                options.max_bytes
            );
        }

        (target_width, target_height) = next_dimensions_for_byte_cap(
            target_width,
            target_height,
            bytes.len(),
            options.max_bytes,
        );
    }
}

fn encode_image(
    img: &image::DynamicImage,
    options: ResolvedScreenshotPayloadOptions,
) -> Result<Vec<u8>> {
    let mut out = Vec::new();
    match options.format {
        ScreenshotOutputFormat::Png => {
            img.write_to(&mut Cursor::new(&mut out), image::ImageFormat::Png)
                .context("failed to encode screenshot PNG")?;
        }
        ScreenshotOutputFormat::Jpeg => {
            let rgb = img.to_rgb8();
            JpegEncoder::new_with_quality(&mut out, options.quality)
                .encode_image(&rgb)
                .context("failed to encode screenshot JPEG")?;
        }
    }
    Ok(out)
}

fn next_dimensions_for_byte_cap(
    width: u32,
    height: u32,
    encoded_bytes: usize,
    max_bytes: usize,
) -> (u32, u32) {
    let shrink = ((max_bytes as f64 / encoded_bytes as f64).sqrt() * 0.9).clamp(0.1, 0.95);
    let mut next_width = ((width as f64 * shrink).floor() as u32).max(1);
    let mut next_height = ((height as f64 * shrink).floor() as u32).max(1);

    if next_width >= width && width > 1 {
        next_width = width - 1;
    }
    if next_height >= height && height > 1 {
        next_height = height - 1;
    }

    (next_width, next_height)
}

fn png_dimensions(bytes: &[u8]) -> Result<(u32, u32)> {
    const PNG_SIGNATURE: &[u8; 8] = b"\x89PNG\r\n\x1a\n";
    if bytes.len() < 24 || &bytes[..8] != PNG_SIGNATURE || &bytes[12..16] != b"IHDR" {
        bail!("screenshot file was not a valid PNG");
    }
    let width = u32::from_be_bytes(bytes[16..20].try_into().unwrap());
    let height = u32::from_be_bytes(bytes[20..24].try_into().unwrap());
    if width == 0 || height == 0 {
        bail!("screenshot PNG had invalid dimensions {width}x{height}");
    }
    let pixels = u64::from(width) * u64::from(height);
    if pixels > MAX_SCREENSHOT_SOURCE_PIXELS {
        bail!(
            "screenshot PNG has {pixels} pixels, over the {MAX_SCREENSHOT_SOURCE_PIXELS}-pixel source limit"
        );
    }
    Ok((width, height))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn valid_png(width: u32, height: u32) -> Vec<u8> {
        let mut png = Vec::new();
        png.extend_from_slice(b"\x89PNG\r\n\x1a\n");
        png.extend_from_slice(&13_u32.to_be_bytes());
        png.extend_from_slice(b"IHDR");
        png.extend_from_slice(&width.to_be_bytes());
        png.extend_from_slice(&height.to_be_bytes());
        png.extend_from_slice(&[8, 6, 0, 0, 0]);
        png
    }

    fn solid_png(width: u32, height: u32) -> Vec<u8> {
        let img = image::RgbaImage::from_pixel(width, height, image::Rgba([24, 96, 160, 255]));
        encode_test_png(img)
    }

    fn noisy_png(width: u32, height: u32) -> Vec<u8> {
        let mut img = image::RgbaImage::new(width, height);
        for (x, y, pixel) in img.enumerate_pixels_mut() {
            let r = ((x * 31 + y * 17) % 256) as u8;
            let g = ((x * 13 + y * 47) % 256) as u8;
            let b = ((x * 97 + y * 7) % 256) as u8;
            *pixel = image::Rgba([r, g, b, 255]);
        }
        encode_test_png(img)
    }

    fn encode_test_png(img: image::RgbaImage) -> Vec<u8> {
        let mut out = Vec::new();
        image::DynamicImage::ImageRgba8(img)
            .write_to(&mut Cursor::new(&mut out), image::ImageFormat::Png)
            .unwrap();
        out
    }

    fn raw_capture(bytes: Vec<u8>) -> RawScreenshotCapture {
        let (width, height) = png_dimensions(&bytes).unwrap();
        RawScreenshotCapture {
            mime_type: "image/png".to_string(),
            bytes,
            source: "test".to_string(),
            width,
            height,
        }
    }

    #[test]
    fn reads_png_dimensions_from_ihdr() {
        let png = valid_png(3840, 1080);

        assert_eq!(png_dimensions(&png).unwrap(), (3840, 1080));
    }

    #[test]
    fn default_payload_downscales_long_edge() {
        let capture =
            prepare_screenshot_payload(raw_capture(solid_png(4000, 1000)), Default::default())
                .unwrap();

        assert_eq!((capture.width, capture.height), (1920, 480));
        assert_eq!(
            (capture.coordinate_width, capture.coordinate_height),
            (4000, 1000)
        );
        assert!(capture.resized);
        assert!(capture.bytes <= DEFAULT_SCREENSHOT_MAX_BYTES);
        assert!(capture.data_url.starts_with("data:image/png;base64,"));
    }

    #[test]
    fn larger_bounded_request_can_keep_more_detail() {
        let capture = prepare_screenshot_payload(
            raw_capture(solid_png(3000, 1000)),
            ScreenshotPayloadOptions {
                max_width: Some(3000),
                max_height: Some(3000),
                max_bytes: Some(DEFAULT_SCREENSHOT_MAX_BYTES),
                ..Default::default()
            },
        )
        .unwrap();

        assert_eq!((capture.width, capture.height), (3000, 1000));
        assert_eq!(
            (capture.coordinate_width, capture.coordinate_height),
            (3000, 1000)
        );
        assert!(!capture.resized);
    }

    #[test]
    fn byte_cap_downscales_until_payload_fits() {
        let capture = prepare_screenshot_payload(
            raw_capture(noisy_png(512, 512)),
            ScreenshotPayloadOptions {
                max_width: Some(512),
                max_height: Some(512),
                max_bytes: Some(20_000),
                ..Default::default()
            },
        )
        .unwrap();

        assert!(capture.bytes <= 20_000);
        assert!(capture.width < 512);
        assert_eq!(
            (capture.coordinate_width, capture.coordinate_height),
            (512, 512)
        );
        assert!(capture.resized);
    }

    #[test]
    fn jpeg_format_compresses_when_requested() {
        let capture = prepare_screenshot_payload(
            raw_capture(noisy_png(512, 512)),
            ScreenshotPayloadOptions {
                max_width: Some(512),
                max_height: Some(512),
                max_bytes: Some(DEFAULT_SCREENSHOT_MAX_BYTES),
                format: Some(ScreenshotOutputFormat::Jpeg),
                quality: Some(60),
                ..Default::default()
            },
        )
        .unwrap();

        assert_eq!(capture.mime_type, "image/jpeg");
        assert_eq!(capture.format, ScreenshotOutputFormat::Jpeg);
        assert_eq!(capture.quality, Some(60));
        assert_eq!((capture.width, capture.height), (512, 512));
        assert_eq!(
            (capture.coordinate_width, capture.coordinate_height),
            (512, 512)
        );
        assert!(capture.bytes < capture.original_bytes);
        assert!(capture.data_url.starts_with("data:image/jpeg;base64,"));
    }

    #[test]
    fn png_dimensions_reject_decompression_bomb_dimensions() {
        let png = valid_png(u32::MAX, u32::MAX);

        assert!(png_dimensions(&png)
            .unwrap_err()
            .to_string()
            .contains("pixel source limit"));
    }
}
