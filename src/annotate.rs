//! Annotated screenshots: a box and the element id `[n]` drawn over each
//! actionable element, so a model reading the image can act by id on what
//! it sees. Drawn on the raw capture, whose pixels are the coordinate space
//! element bounds use, before any crop or resize.

use crate::a11y::{AccessibilityNode, Bounds};
use anyhow::{Context, Result};
use image::{Rgba, RgbaImage};
use std::io::Cursor;

const OUTLINE: Rgba<u8> = Rgba([236, 72, 153, 255]);
const LABEL: Rgba<u8> = Rgba([236, 72, 153, 235]);
const DIGIT: Rgba<u8> = Rgba([255, 255, 255, 255]);
/// Digit cell scale: each font pixel is SCALE x SCALE image pixels.
const SCALE: u32 = 2;

/// 3x5 bitmap digits, one row per byte (3 low bits, left to right).
const DIGITS: [[u8; 5]; 10] = [
    [0b111, 0b101, 0b101, 0b101, 0b111],
    [0b010, 0b110, 0b010, 0b010, 0b111],
    [0b111, 0b001, 0b111, 0b100, 0b111],
    [0b111, 0b001, 0b111, 0b001, 0b111],
    [0b101, 0b101, 0b111, 0b001, 0b001],
    [0b111, 0b100, 0b111, 0b001, 0b111],
    [0b111, 0b100, 0b111, 0b101, 0b111],
    [0b111, 0b001, 0b010, 0b010, 0b010],
    [0b111, 0b101, 0b111, 0b101, 0b111],
    [0b111, 0b101, 0b111, 0b001, 0b111],
];

/// Elements worth a label: on screen, with an action or editable.
pub(crate) fn actionable(nodes: &[AccessibilityNode]) -> Vec<(u32, Bounds)> {
    nodes
        .iter()
        // A window's own `raise` would box the whole screenshot.
        .filter(|node| !matches!(node.role.as_str(), "frame" | "window" | "application"))
        .filter(|node| {
            node.supports_editable_text
                || node.actions.iter().any(crate::outline::is_specific_action)
        })
        .filter_map(|node| {
            let bounds = node.bounds.clone()?;
            (bounds.width >= 4 && bounds.height >= 4).then_some((node.index, bounds))
        })
        .collect()
}

/// Draws the boxes and ids on a PNG; returns the new PNG.
pub(crate) fn annotate_png(png: &[u8], elements: &[(u32, Bounds)]) -> Result<Vec<u8>> {
    let mut image = image::load_from_memory(png)
        .context("screenshot PNG is invalid")?
        .to_rgba8();
    for (id, bounds) in elements {
        draw_box(&mut image, bounds);
        draw_label(&mut image, *id, bounds.x, bounds.y);
    }
    let mut out = Vec::new();
    image::DynamicImage::ImageRgba8(image)
        .write_to(&mut Cursor::new(&mut out), image::ImageFormat::Png)
        .context("failed to encode annotated screenshot")?;
    Ok(out)
}

fn put(image: &mut RgbaImage, x: i64, y: i64, color: Rgba<u8>) {
    if x >= 0 && y >= 0 && (x as u32) < image.width() && (y as u32) < image.height() {
        image.put_pixel(x as u32, y as u32, color);
    }
}

fn draw_box(image: &mut RgbaImage, bounds: &Bounds) {
    let (x0, y0) = (i64::from(bounds.x), i64::from(bounds.y));
    let (x1, y1) = (
        x0 + i64::from(bounds.width) - 1,
        y0 + i64::from(bounds.height) - 1,
    );
    for thickness in 0..2 {
        for x in x0..=x1 {
            put(image, x, y0 + thickness, OUTLINE);
            put(image, x, y1 - thickness, OUTLINE);
        }
        for y in y0..=y1 {
            put(image, x0 + thickness, y, OUTLINE);
            put(image, x1 - thickness, y, OUTLINE);
        }
    }
}

/// The id in white on a filled tag at the element's top-left corner.
fn draw_label(image: &mut RgbaImage, id: u32, x: i32, y: i32) {
    let digits: Vec<usize> = id
        .to_string()
        .bytes()
        .map(|byte| usize::from(byte - b'0'))
        .collect();
    let cell = 4 * SCALE as i64; // 3 columns + 1 gap
    let (width, height) = (digits.len() as i64 * cell + SCALE as i64, 7 * SCALE as i64);
    let (x, y) = (i64::from(x), i64::from(y));
    for dy in 0..height {
        for dx in 0..width {
            put(image, x + dx, y + dy, LABEL);
        }
    }
    for (at, digit) in digits.iter().enumerate() {
        for (row, bits) in DIGITS[*digit].iter().enumerate() {
            for column in 0..3 {
                if bits & (0b100 >> column) == 0 {
                    continue;
                }
                for sy in 0..SCALE as i64 {
                    for sx in 0..SCALE as i64 {
                        put(
                            image,
                            x + SCALE as i64 + at as i64 * cell + column * SCALE as i64 + sx,
                            y + SCALE as i64 + row as i64 * SCALE as i64 + sy,
                            DIGIT,
                        );
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn boxes_and_ids_are_drawn_where_the_element_is() {
        let mut blank = Vec::new();
        image::DynamicImage::ImageRgba8(RgbaImage::new(120, 60))
            .write_to(&mut Cursor::new(&mut blank), image::ImageFormat::Png)
            .unwrap();
        let bounds = Bounds {
            x: 10,
            y: 10,
            width: 80,
            height: 30,
        };
        let png = annotate_png(&blank, &[(7, bounds)]).unwrap();
        let image = image::load_from_memory(&png).unwrap().to_rgba8();
        assert_eq!(
            *image.get_pixel(89, 39),
            OUTLINE,
            "bottom-right corner of the box"
        );
        assert_eq!(
            *image.get_pixel(50, 25),
            Rgba([0, 0, 0, 0]),
            "inside stays clear"
        );
        // Digit 7's top row: three lit cells after the tag padding.
        assert_eq!(*image.get_pixel(10 + 2, 10 + 2), DIGIT);
        assert_eq!(
            *image.get_pixel(5, 5),
            Rgba([0, 0, 0, 0]),
            "outside untouched"
        );
    }
}
