//! Color-preserving neural preparation. The model refines luminance; hue and
//! saturation are reconstructed from the palette-expanded source artwork.
use super::SCALE;
use anyhow::{ensure, Result};
use image::{imageops, Rgba, RgbaImage};

fn luma(pixel: &Rgba<u8>) -> u8 {
    ((u32::from(pixel[0]) * 19595
        + u32::from(pixel[1]) * 38470
        + u32::from(pixel[2]) * 7471
        + 32768)
        >> 16) as u8
}

/// Resolve repeated 2-color checker dithering before neural inference. A full
/// 4x4 periodic patch is required; isolated pixel details and alpha edges stay
/// unchanged. Every resolved patch retains the rounded average source color.
pub(super) fn reference(original: &RgbaImage) -> RgbaImage {
    let mut result = original.clone();
    if original.width() < 4 || original.height() < 4 {
        return result;
    }
    for y in 0..=original.height() - 4 {
        for x in 0..=original.width() - 4 {
            let a = *original.get_pixel(x, y);
            let b = *original.get_pixel(x + 1, y);
            if a[3] != 255 || b[3] != 255 || a == b {
                continue;
            }
            if !(0..4).all(|dy| {
                (0..4).all(|dx| {
                    *original.get_pixel(x + dx, y + dy) == if (dx + dy) % 2 == 0 { a } else { b }
                })
            }) {
                continue;
            }
            let mean = Rgba([
                (u16::from(a[0]) + u16::from(b[0])).div_ceil(2) as u8,
                (u16::from(a[1]) + u16::from(b[1])).div_ceil(2) as u8,
                (u16::from(a[2]) + u16::from(b[2])).div_ceil(2) as u8,
                255,
            ]);
            for dy in 0..4 {
                for dx in 0..4 {
                    result.put_pixel(x + dx, y + dy, mean);
                }
            }
        }
    }
    result
}

pub(super) fn model_input(reference: &RgbaImage) -> RgbaImage {
    RgbaImage::from_fn(reference.width(), reference.height(), |x, y| {
        let source = reference.get_pixel(x, y);
        let y = luma(source);
        Rgba([y, y, y, source[3]])
    })
}

/// Reconstruct RGB using nonnegative interpolation weights and one luminance
/// gain per pixel. The neural output never supplies color channels. Limiting
/// gain to the RGB gamut avoids per-channel clipping and the hue shifts that
/// clipping would introduce. Original alpha is restored separately by prepare.
pub(super) fn reconstruct(reference: &RgbaImage, enhanced: &mut RgbaImage) -> Result<()> {
    ensure!(
        enhanced.dimensions() == (reference.width() * SCALE, reference.height() * SCALE),
        "HD luminance dimensions do not match the color reference"
    );
    let color = imageops::resize(
        reference,
        enhanced.width(),
        enhanced.height(),
        imageops::FilterType::Triangle,
    );
    for (output, source) in enhanced.pixels_mut().zip(color.pixels()) {
        let original_luma = luma(source);
        let predicted_luma = luma(output);
        let max = source.0[..3].iter().copied().max().unwrap_or(0);
        let gain = if original_luma == 0 || max == 0 {
            0.
        } else {
            (f32::from(predicted_luma) / f32::from(original_luma)).min(255. / f32::from(max))
        };
        for c in 0..3 {
            output[c] = (f32::from(source[c]) * gain).round() as u8;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn resolves_the_reproduced_teal_checker_before_model_inference() {
        let original = RgbaImage::from_fn(16, 16, |x, y| {
            if (x + y) % 2 == 0 {
                Rgba([23, 27, 139, 255])
            } else {
                Rgba([19, 143, 19, 255])
            }
        });
        let reference = reference(&original);
        assert!(reference.pixels().all(|p| p.0 == [21, 85, 79, 255]));
        assert!(model_input(&reference)
            .pixels()
            .all(|p| p[0] == p[1] && p[1] == p[2]));
    }
    #[test]
    fn keeps_isolated_pixel_details_and_alpha_edges() {
        let original = RgbaImage::from_fn(8, 8, |x, y| {
            Rgba([
                if x == 3 && y == 3 { 255 } else { 20 },
                80,
                120,
                if x < 2 { 0 } else { 255 },
            ])
        });
        assert_eq!(reference(&original), original);
    }
    #[test]
    fn predicted_colors_cannot_change_the_source_hue() {
        let reference = RgbaImage::from_pixel(4, 4, Rgba([40, 80, 120, 255]));
        // Even a completely red prediction can supply only luminance.
        let mut enhanced = RgbaImage::from_pixel(16, 16, Rgba([255, 0, 0, 173]));
        reconstruct(&reference, &mut enhanced).unwrap();
        for p in enhanced.pixels() {
            assert!((2 * i16::from(p[0]) - i16::from(p[1])).abs() <= 1);
            assert!((3 * i16::from(p[0]) - i16::from(p[2])).abs() <= 2);
            assert_eq!(p[3], 173);
        }
    }
    #[test]
    fn bright_predictions_keep_saturated_palette_colors_inside_the_rgb_gamut() {
        let reference = RgbaImage::from_pixel(4, 4, Rgba([0, 255, 0, 255]));
        let mut enhanced = RgbaImage::from_pixel(16, 16, Rgba([255, 255, 255, 255]));
        reconstruct(&reference, &mut enhanced).unwrap();
        assert!(enhanced.pixels().all(|p| p.0 == [0, 255, 0, 255]));
    }
}
