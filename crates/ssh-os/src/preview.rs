//! Half-block image preview for the Viewer pane.

use image::imageops::FilterType;
use image::{GenericImageView, RgbaImage};
use ratatui::style::Color;

/// One terminal cell: upper pixel = fg, lower pixel = bg, glyph ▀.
#[derive(Debug, Clone, Copy)]
pub struct HalfCell {
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

impl HalfblockPreview {
    /// Decode image bytes and render into at most `max_cols` × `max_rows` cells.
    ///
    /// Each cell is one column wide and covers two source pixels vertically (▀),
    /// which roughly matches typical terminal cell aspect (~1:2).
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
        // Each row of cells covers 2 source pixels vertically; reserve nothing here —
        // caller should already subtract header rows from max_rows.
        let max_px_h = (max_rows.max(4) as u32).saturating_mul(2);
        let max_px_w = max_cols;

        let scale = (max_px_w as f32 / ow as f32)
            .min(max_px_h as f32 / oh as f32)
            .min(1.0);
        let mut nw = ((ow as f32) * scale).round().max(1.0) as u32;
        let mut nh = ((oh as f32) * scale).round().max(2.0) as u32;
        // Ensure even height for pairing upper/lower half-blocks.
        if nh % 2 == 1 {
            nh += 1;
        }
        nw = nw.min(max_px_w).max(1);
        nh = nh.min(max_px_h).max(2);
        if nh % 2 == 1 {
            nh -= 1;
        }

        let rgba: RgbaImage = img.resize_exact(nw, nh, FilterType::CatmullRom).to_rgba8();

        let cell_rows = (nh / 2) as u16;
        let cell_cols = nw as u16;
        let mut rows = Vec::with_capacity(cell_rows as usize);

        for cy in 0..cell_rows {
            let mut row = Vec::with_capacity(cell_cols as usize);
            for cx in 0..cell_cols {
                let top = rgba.get_pixel(cx as u32, cy as u32 * 2).0;
                let bot = rgba.get_pixel(cx as u32, cy as u32 * 2 + 1).0;
                row.push(HalfCell {
                    fg: rgb_over(top, bg_rgb),
                    bg: rgb_over(bot, bg_rgb),
                });
            }
            rows.push(row);
        }

        Ok(Self {
            width: cell_cols,
            height: cell_rows,
            rows,
            meta: format!("{ow}×{oh} → {cell_cols}×{cell_rows} cells"),
        })
    }
}

fn rgb_over(px: [u8; 4], bg: [u8; 3]) -> Color {
    let a = px[3] as f32 / 255.0;
    if a >= 0.999 {
        return Color::Rgb(px[0], px[1], px[2]);
    }
    if a <= 0.001 {
        return Color::Rgb(bg[0], bg[1], bg[2]);
    }
    let inv = 1.0 - a;
    Color::Rgb(
        (px[0] as f32 * a + bg[0] as f32 * inv).round() as u8,
        (px[1] as f32 * a + bg[1] as f32 * inv).round() as u8,
        (px[2] as f32 * a + bg[2] as f32 * inv).round() as u8,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{ImageBuffer, ImageFormat, Rgba};

    #[test]
    fn decodes_tiny_png() {
        let img = ImageBuffer::from_pixel(4, 4, Rgba([255u8, 0, 0, 255]));
        let mut bytes = Vec::new();
        img.write_to(&mut std::io::Cursor::new(&mut bytes), ImageFormat::Png)
            .expect("encode png");
        let preview = HalfblockPreview::from_bytes(&bytes, 40, 20).expect("decode");
        assert!(preview.width >= 1);
        assert!(preview.height >= 1);
        assert!(!preview.rows.is_empty());
    }

    #[test]
    fn composites_transparent_over_bg() {
        let img = ImageBuffer::from_pixel(2, 2, Rgba([0u8, 0, 0, 0]));
        let mut bytes = Vec::new();
        img.write_to(&mut std::io::Cursor::new(&mut bytes), ImageFormat::Png)
            .expect("encode png");
        let preview =
            HalfblockPreview::from_bytes_on(&bytes, 40, 20, [10, 20, 30]).expect("decode");
        let cell = preview.rows[0][0];
        assert_eq!(cell.fg, Color::Rgb(10, 20, 30));
        assert_eq!(cell.bg, Color::Rgb(10, 20, 30));
    }
}
