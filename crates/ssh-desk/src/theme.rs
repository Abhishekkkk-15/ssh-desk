//! Tokyo Night–inspired UI palettes (dark + light).
//!
//! Active theme is process-wide so existing `Theme::*` call sites stay simple.

use std::sync::atomic::{AtomicU8, Ordering};

use ratatui::style::Color;
use serde::{Deserialize, Serialize};

static ACTIVE: AtomicU8 = AtomicU8::new(0);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ThemeId {
    #[default]
    Dark,
    Light,
}

impl ThemeId {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Dark => "dark",
            Self::Light => "light",
        }
    }

    pub fn toggle(self) -> Self {
        match self {
            Self::Dark => Self::Light,
            Self::Light => Self::Dark,
        }
    }

    fn from_u8(v: u8) -> Self {
        match v {
            1 => Self::Light,
            _ => Self::Dark,
        }
    }
}

pub fn set_theme(id: ThemeId) {
    ACTIVE.store(id as u8, Ordering::Relaxed);
}

pub fn current_theme() -> ThemeId {
    ThemeId::from_u8(ACTIVE.load(Ordering::Relaxed))
}

/// Named theme tokens used across chrome, panes, and accents.
pub struct Theme;

impl Theme {
    fn dark() -> bool {
        matches!(current_theme(), ThemeId::Dark)
    }

    // --- Surfaces ---
    pub fn bg() -> Color {
        if Self::dark() {
            Color::Rgb(0x1a, 0x1b, 0x26)
        } else {
            Color::Rgb(0xee, 0xf1, 0xf6)
        }
    }

    pub fn chrome() -> Color {
        if Self::dark() {
            Color::Rgb(0x16, 0x16, 0x1e)
        } else {
            Color::Rgb(0xe2, 0xe6, 0xef)
        }
    }

    pub fn surface() -> Color {
        if Self::dark() {
            Color::Rgb(0x24, 0x28, 0x3b)
        } else {
            Color::Rgb(0xff, 0xff, 0xff)
        }
    }

    #[allow(dead_code)]
    pub fn surface_2() -> Color {
        if Self::dark() {
            Color::Rgb(0x29, 0x2e, 0x42)
        } else {
            Color::Rgb(0xf5, 0xf7, 0xfb)
        }
    }

    // --- Text ---
    pub fn fg() -> Color {
        if Self::dark() {
            Color::Rgb(0xc0, 0xca, 0xf5)
        } else {
            Color::Rgb(0x1a, 0x1b, 0x26)
        }
    }

    pub fn fg_dim() -> Color {
        if Self::dark() {
            Color::Rgb(0x56, 0x5f, 0x89)
        } else {
            Color::Rgb(0x6b, 0x73, 0x8a)
        }
    }

    pub fn fg_muted() -> Color {
        if Self::dark() {
            Color::Rgb(0xa9, 0xb1, 0xd6)
        } else {
            Color::Rgb(0x3d, 0x44, 0x5c)
        }
    }

    pub fn on_accent() -> Color {
        if Self::dark() {
            Color::Rgb(0x1a, 0x1b, 0x26)
        } else {
            Color::Rgb(0xff, 0xff, 0xff)
        }
    }

    // --- Accents ---
    pub fn accent() -> Color {
        if Self::dark() {
            Color::Rgb(0x7a, 0xa2, 0xf7)
        } else {
            Color::Rgb(0x2f, 0x5b, 0xe0)
        }
    }

    #[allow(dead_code)]
    pub fn accent_2() -> Color {
        if Self::dark() {
            Color::Rgb(0x7d, 0xcf, 0xff)
        } else {
            Color::Rgb(0x0e, 0x74, 0x9a)
        }
    }

    pub fn ok() -> Color {
        if Self::dark() {
            Color::Rgb(0x9e, 0xce, 0x6a)
        } else {
            Color::Rgb(0x2f, 0x7a, 0x4a)
        }
    }

    pub fn warn() -> Color {
        if Self::dark() {
            Color::Rgb(0xe0, 0xaf, 0x68)
        } else {
            Color::Rgb(0xb4, 0x53, 0x09)
        }
    }

    pub fn err() -> Color {
        if Self::dark() {
            Color::Rgb(0xf7, 0x76, 0x8e)
        } else {
            Color::Rgb(0xc2, 0x3b, 0x4d)
        }
    }

    pub fn info() -> Color {
        if Self::dark() {
            Color::Rgb(0xbb, 0x9a, 0xf7)
        } else {
            Color::Rgb(0x5b, 0x4d, 0xb8)
        }
    }

    /// Background RGB for compositing transparent image pixels.
    pub fn bg_rgb() -> [u8; 3] {
        if Self::dark() {
            [0x1a, 0x1b, 0x26]
        } else {
            [0xee, 0xf1, 0xf6]
        }
    }
}
