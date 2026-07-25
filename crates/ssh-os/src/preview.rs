//! Braille image preview for the Viewer pane (~8 pixels per terminal cell).
//!
//! Half-block (▀) only yields 2 samples per cell and looks soft. Unicode
//! braille (⠀-⣿) packs a 2×4 grid into each cell — about 4× the resolution
//! and much closer to what tools like `chafa` / `viu` show.

use image::imageops::FilterType;
use image::{GenericImageView, RgbaImage};
use ratatui::style::Color;

/// One terminal cell: braille glyph + fg/bg colors.
#[derive(Debug, Clone, Copy)]
pub struct HalfCell {
    pub glyph: char,
    pub fg: Color,
    pub bg: Color,
}

#[derive(Debug, Clone)]
pub struct HalfblockPreview {
    pub width: u16,
    pub height: u16,
    pub rows: Vec<Vec<HalfCell>>,
    pub meta: String,
}

/// Braille is a 2×4 pixel grid per cell.
const PX_W: u32 = 2;
const PX_H: u32 = 4;

impl HalfblockPreview {
    /// Decode image bytes and render into at most `max_cols` × `max_rows` cells.
    pub fn from_bytes(bytes: &[u8], max_cols: u16, max_rows: u16) -> Result<Self, String> {
        Self::from_bytes_on(bytes, max_cols, max_rows, [0x1a, 0x1b, 0x26])
    }

    /// Same as [`from_bytes`] but composites transparent pixels onto `bg_rgb`.
    pub fn from_bytes_on(
        bytes: &[u8],
        max_cols: u16,
        max_rows: u16,
        bg_rgb: [u8; 3],
    ) -> Result<Self, String> {
        let img = image::load_from_memory(bytes).map_err(|e| e.to_string())?;
        let (ow, oh) = img.dimensions();
        if ow == 0 || oh == 0 {
            return Err("empty image".into());
        }

        let max_cols = max_cols.max(8) as u32;
        let max_rows = max_rows.max(4) as u32;
        let max_px_w = max_cols.saturating_mul(PX_W);
        let max_px_h = max_rows.saturating_mul(PX_H);

        let scale = (max_px_w as f32 / ow as f32)
            .min(max_px_h as f32 / oh as f32)
            .min(1.0);
        let mut nw = ((ow as f32) * scale).round().max(PX_W as f32) as u32;
        let mut nh = ((oh as f32) * scale).round().max(PX_H as f32) as u32;
        // Snap to braille grid.
        nw = (nw / PX_W * PX_W).clamp(PX_W, max_px_w);
        nh = (nh / PX_H * PX_H).clamp(PX_H, max_px_h);

        let rgba: RgbaImage = img.resize_exact(nw, nh, FilterType::CatmullRom).to_rgba8();

        let cell_cols = (nw / PX_W) as u16;
        let cell_rows = (nh / PX_H) as u16;
        let mut rows = Vec::with_capacity(cell_rows as usize);

        for cy in 0..cell_rows as u32 {
            let mut row = Vec::with_capacity(cell_cols as usize);
            for cx in 0..cell_cols as u32 {
                row.push(braille_cell(&rgba, cx * PX_W, cy * PX_H, bg_rgb));
            }
            rows.push(row);
        }

        Ok(Self {
            width: cell_cols,
            height: cell_rows,
            rows,
            meta: format!("{ow}×{oh} → {cell_cols}×{cell_rows} braille"),
        })
    }
}

/// Map a 2×4 pixel block to a colored braille cell.
fn braille_cell(img: &RgbaImage, x0: u32, y0: u32, canvas_bg: [u8; 3]) -> HalfCell {
    // Dot bit order for U+2800 braille:
    //  (0,0)=0x01 (0,1)=0x02 (0,2)=0x04 (0,3)=0x40
    //  (1,0)=0x08 (1,1)=0x10 (1,2)=0x20 (1,3)=0x80
    const DOT_BITS: [[u8; 4]; 2] = [[0x01, 0x02, 0x04, 0x40], [0x08, 0x10, 0x20, 0x80]];

    let mut samples = [[0u8; 4]; 8];
    let mut i = 0usize;
    for dy in 0..PX_H {
        for dx in 0..PX_W {
            let px = img.get_pixel(x0 + dx, y0 + dy).0;
            samples[i] = composite_rgba(px, canvas_bg);
            i += 1;
        }
    }

    // Luminance threshold = mean of the 8 samples (adapts per cell).
    let mut lum = [0.0f32; 8];
    let mut sum = 0.0f32;
    for (i, s) in samples.iter().enumerate() {
        let l = 0.2126 * s[0] as f32 + 0.7152 * s[1] as f32 + 0.0722 * s[2] as f32;
        lum[i] = l;
        sum += l;
    }
    let mean = sum / 8.0;

    let mut bits: u8 = 0;
    let mut fg_acc = [0u32; 3];
    let mut fg_n = 0u32;
    let mut bg_acc = [0u32; 3];
    let mut bg_n = 0u32;

    i = 0;
    for dy in 0..PX_H as usize {
        for dx in 0..PX_W as usize {
            let on = lum[i] >= mean;
            if on {
                bits |= DOT_BITS[dx][dy];
                fg_acc[0] += samples[i][0] as u32;
                fg_acc[1] += samples[i][1] as u32;
                fg_acc[2] += samples[i][2] as u32;
                fg_n += 1;
            } else {
                bg_acc[0] += samples[i][0] as u32;
                bg_acc[1] += samples[i][1] as u32;
                bg_acc[2] += samples[i][2] as u32;
                bg_n += 1;
            }
            i += 1;
        }
    }

    // Flat cell (all similar): fill with a solid-ish braille + average color.
    if fg_n == 0 || bg_n == 0 {
        let mut avg = [0u32; 3];
        for s in &samples {
            avg[0] += s[0] as u32;
            avg[1] += s[1] as u32;
            avg[2] += s[2] as u32;
        }
        let c = Color::Rgb((avg[0] / 8) as u8, (avg[1] / 8) as u8, (avg[2] / 8) as u8);
        return HalfCell {
            glyph: if fg_n == 8 || (fg_n == 0 && mean > 40.0) {
                '⣿' // full braille
            } else {
                ' '
            },
            fg: c,
            bg: Color::Rgb(canvas_bg[0], canvas_bg[1], canvas_bg[2]),
        };
    }

    let glyph = char::from_u32(0x2800 + bits as u32).unwrap_or('⠀');
    HalfCell {
        glyph,
        fg: Color::Rgb(
            (fg_acc[0] / fg_n) as u8,
            (fg_acc[1] / fg_n) as u8,
            (fg_acc[2] / fg_n) as u8,
        ),
        bg: Color::Rgb(
            (bg_acc[0] / bg_n) as u8,
            (bg_acc[1] / bg_n) as u8,
            (bg_acc[2] / bg_n) as u8,
        ),
    }
}

fn composite_rgba(px: [u8; 4], bg: [u8; 3]) -> [u8; 4] {
    let a = px[3] as f32 / 255.0;
    if a >= 0.999 {
        return [px[0], px[1], px[2], 255];
    }
    if a <= 0.001 {
        return [bg[0], bg[1], bg[2], 255];
    }
    let inv = 1.0 - a;
    [
        (px[0] as f32 * a + bg[0] as f32 * inv).round() as u8,
        (px[1] as f32 * a + bg[1] as f32 * inv).round() as u8,
        (px[2] as f32 * a + bg[2] as f32 * inv).round() as u8,
        255,
    ]
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{ImageBuffer, ImageFormat, Rgba};

    #[test]
    fn decodes_tiny_png() {
        let img = ImageBuffer::from_pixel(8, 8, Rgba([255u8, 0, 0, 255]));
        let mut bytes = Vec::new();
        img.write_to(&mut std::io::Cursor::new(&mut bytes), ImageFormat::Png)
            .expect("encode png");
        let preview = HalfblockPreview::from_bytes(&bytes, 40, 20).expect("decode");
        assert!(preview.width >= 1);
        assert!(preview.height >= 1);
        assert!(!preview.rows.is_empty());
        assert!(preview.rows[0][0].glyph != '▀');
    }

    #[test]
    fn composites_transparent_over_bg() {
        let img = ImageBuffer::from_pixel(4, 4, Rgba([0u8, 0, 0, 0]));
        let mut bytes = Vec::new();
        img.write_to(&mut std::io::Cursor::new(&mut bytes), ImageFormat::Png)
            .expect("encode png");
        let preview =
            HalfblockPreview::from_bytes_on(&bytes, 40, 20, [10, 20, 30]).expect("decode");
        // Fully transparent → flat dark cell on canvas bg.
        let cell = preview.rows[0][0];
        assert_eq!(cell.bg, Color::Rgb(10, 20, 30));
    }
}
