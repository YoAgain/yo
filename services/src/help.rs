//! The picture for a user who Yos a bridge and is not connected: how to, with a QR code and the address to read. Drawn
//! once at the start, the same for everyone.

use crate::gatekeeper::action::MAX_PICTURE;
use ab_glyph::{point, Font, FontRef, PxScale, ScaleFont};
use image::{codecs::jpeg::JpegEncoder, imageops, Rgb, RgbImage};
use std::io::Cursor;

const FONT: &[u8] = include_bytes!("../../app/ios/YoApp/Resources/Montserrat-Bold.ttf");
const WIDTH: u32 = 1080;
const HEIGHT: u32 = 1920;
const AMETHYST: Rgb<u8> = Rgb([0x9B, 0x59, 0xB6]);
const WHITE: Rgb<u8> = Rgb([255; 3]);
const LOGO: u32 = 260;

/// `logo`: a PNG, left out if it does not read.
pub fn draw(name: &str, url: &str, logo: Option<&[u8]>) -> Option<Vec<u8>> {
    let font = FontRef::try_from_slice(FONT).ok()?;
    let mut picture = RgbImage::from_pixel(WIDTH, HEIGHT, AMETHYST);
    if let Some(logo) = logo.and_then(read) {
        let logo = imageops::resize(&logo, LOGO, LOGO, imageops::FilterType::Lanczos3);
        for (x, y, pixel) in logo.enumerate_pixels() {
            if rounded(x, y) {
                picture.put_pixel((WIDTH - LOGO) / 2 + x, 200 + y, *pixel);
            }
        }
    }
    text(&mut picture, &font, &format!("Connect {name}"), 620, 116.0);
    text(&mut picture, &font, "first", 750, 116.0);
    code(&mut picture, url)?;
    text(&mut picture, &font, "Open", 1590, 60.0);
    text(&mut picture, &font, url.trim_start_matches("https://"), 1710, 110.0);
    let mut jpeg = Vec::new();
    JpegEncoder::new_with_quality(&mut jpeg, 85).encode_image(&picture).ok()?;
    (jpeg.len() <= MAX_PICTURE).then_some(jpeg)
}

fn read(png: &[u8]) -> Option<RgbImage> {
    let mut reader = image::ImageReader::with_format(Cursor::new(png), image::ImageFormat::Png);
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(2048);
    limits.max_image_height = Some(2048);
    reader.limits(limits);
    Some(reader.decode().ok()?.to_rgb8())
}

fn rounded(x: u32, y: u32) -> bool {
    let radius = LOGO as f32 * 0.23;
    let away = |v: u32| (radius - v.min(LOGO - 1 - v) as f32).max(0.0);
    away(x).hypot(away(y)) <= radius
}

/// Square modules on a white square, in the middle.
fn code(picture: &mut RgbImage, url: &str) -> Option<()> {
    let code = qrcode::QrCode::new(url).ok()?;
    let modules = code.width() as u32;
    let size = 500 / modules;
    let (side, top) = (modules * size, 840 + 4 * size);
    let left = (WIDTH - side) / 2;
    let quiet = 4 * size;
    for y in 0..side + 2 * quiet {
        for x in 0..side + 2 * quiet {
            let inside = (quiet..quiet + side).contains(&x) && (quiet..quiet + side).contains(&y);
            let dark =
                inside && code[(((x - quiet) / size) as usize, ((y - quiet) / size) as usize)] == qrcode::Color::Dark;
            let pixel = if dark { AMETHYST } else { WHITE };
            picture.put_pixel(left - quiet + x, top - quiet + y, pixel);
        }
    }
    Some(())
}

/// One line in the middle, smaller if it would not fit; `y` is its baseline.
fn text(picture: &mut RgbImage, font: &FontRef, text: &str, y: u32, size: f32) {
    let wide = |size: f32| {
        let font = font.as_scaled(PxScale::from(size));
        text.chars().map(|c| font.h_advance(font.glyph_id(c))).sum::<f32>()
    };
    let size = size.min(size * (WIDTH as f32 - 120.0) / wide(size));
    let mut x = (WIDTH as f32 - wide(size)) / 2.0;
    for c in text.chars() {
        let id = font.glyph_id(c);
        let glyph = id.with_scale_and_position(PxScale::from(size), point(x, y as f32));
        x += font.as_scaled(PxScale::from(size)).h_advance(id);
        if let Some(outline) = font.outline_glyph(glyph) {
            let bounds = outline.px_bounds();
            outline.draw(|gx, gy, cover| {
                let (px, py) = (bounds.min.x as i64 + i64::from(gx), bounds.min.y as i64 + i64::from(gy));
                if (0..i64::from(WIDTH)).contains(&px) && (0..i64::from(HEIGHT)).contains(&py) {
                    let pixel = picture.get_pixel_mut(px as u32, py as u32);
                    for channel in &mut pixel.0 {
                        *channel = (f32::from(*channel) * (1.0 - cover) + 255.0 * cover) as u8;
                    }
                }
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_picture_is_drawn_with_a_logo_a_broken_one_or_none() {
        let logo = std::env::var("HELP_LOGO").ok().and_then(|path| std::fs::read(path).ok());
        for logo in [logo.as_deref(), Some(&b"no png"[..]), None] {
            let jpeg = draw("IFTTT", "https://ifttt.com/yo", logo).unwrap();
            assert!(jpeg.starts_with(b"\xff\xd8\xff"));
            if let Ok(path) =
                std::env::var("HELP_SAMPLE").map(|path| format!("{path}-{}.jpg", logo.map_or(0, <[u8]>::len)))
            {
                std::fs::write(path, &jpeg).unwrap();
            }
        }
        assert!(
            draw("A.VERY_LONG_NAME_OF_32_CHARACTERS", &format!("https://a.com/{}", "a".repeat(100)), None).is_some()
        );
    }
}
