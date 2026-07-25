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
    pub fn from_bytes(bytes: &[u8], max_cols: u16, max_rows: u16) -> Result<Self, String> {
        let img = image::load_from_memory(bytes).map_err(|e| e.to_string())?;
        let (ow, oh) = img.dimensions();
        let max_cols = max_cols.max(8) as u32;
        // Each row of cells covers 2 source pixels vertically.
        let max_px_h = (max_rows.max(4) as u32).saturating_mul(2);
        let max_px_w = max_cols;

        let scale = (max_px_w as f32 / ow as f32)
            .min(max_px_h as f32 / oh as f32)
            .min(1.0);
        let nw = ((ow as f32) * scale).round().max(1.0) as u32;
        let nh = ((oh as f32) * scale).round().max(2.0) as u32;
        // Ensure even height for pairing.
        let nh = nh + (nh % 2);

        let rgba: RgbaImage = img.resize_exact(nw, nh, FilterType::Triangle).to_rgba8();

        let cell_rows = (nh / 2) as u16;
        let cell_cols = nw as u16;
        let mut rows = Vec::with_capacity(cell_rows as usize);

        for cy in 0..cell_rows {
            let mut row = Vec::with_capacity(cell_cols as usize);
            for cx in 0..cell_cols {
                let top = rgba.get_pixel(cx as u32, cy as u32 * 2).0;
                let bot = rgba.get_pixel(cx as u32, cy as u32 * 2 + 1).0;
                row.push(HalfCell {
                    fg: Color::Rgb(top[0], top[1], top[2]),
                    bg: Color::Rgb(bot[0], bot[1], bot[2]),
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
