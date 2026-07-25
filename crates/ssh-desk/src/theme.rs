//! Tokyo Night–inspired UI palette (muted blues / soft greens, no neon).
//!
//! Designed for long sessions: soft contrast on slate surfaces instead of
//! bright green/yellow on pitch black.

use ratatui::style::Color;

/// Named theme tokens used across chrome, panes, and accents.
pub struct Theme;

impl Theme {
    // --- Surfaces ---
    /// Deep editor background (not pure black).
    pub const BG: Color = Color::Rgb(0x1a, 0x1b, 0x26);
    /// Title / dock / status chrome band.
    pub const CHROME: Color = Color::Rgb(0x16, 0x16, 0x1e);
    /// Unfocused pane header strip.
    pub const SURFACE: Color = Color::Rgb(0x24, 0x28, 0x3b);
    /// Subtle elevated surface (lists, inactive pills).
    #[allow(dead_code)]
    pub const SURFACE_2: Color = Color::Rgb(0x29, 0x2e, 0x42);

    // --- Text ---
    pub const FG: Color = Color::Rgb(0xc0, 0xca, 0xf5);
    pub const FG_DIM: Color = Color::Rgb(0x56, 0x5f, 0x89);
    pub const FG_MUTED: Color = Color::Rgb(0xa9, 0xb1, 0xd6);
    /// Text on filled accent chips.
    pub const ON_ACCENT: Color = Color::Rgb(0x1a, 0x1b, 0x26);

    // --- Accents (muted) ---
    /// Primary focus / brand (soft blue).
    pub const ACCENT: Color = Color::Rgb(0x7a, 0xa2, 0xf7);
    /// Secondary highlight (soft teal).
    #[allow(dead_code)]
    pub const ACCENT_2: Color = Color::Rgb(0x7d, 0xcf, 0xff);
    /// Success / connected (muted green — not neon).
    pub const OK: Color = Color::Rgb(0x9e, 0xce, 0x6a);
    /// Warning / drop target (warm amber).
    pub const WARN: Color = Color::Rgb(0xe0, 0xaf, 0x68);
    /// Error / destructive.
    pub const ERR: Color = Color::Rgb(0xf7, 0x76, 0x8e);
    /// Info / meta (soft violet used sparingly).
    pub const INFO: Color = Color::Rgb(0xbb, 0x9a, 0xf7);
}
