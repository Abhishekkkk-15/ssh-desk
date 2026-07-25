//! In-app diagnostics / error log for the F9 viewer.

use std::time::{SystemTime, UNIX_EPOCH};

const MAX_ENTRIES: usize = 200;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiagLevel {
    Info,
    Warn,
    Error,
}

impl DiagLevel {
    pub fn tag(self) -> &'static str {
        match self {
            Self::Info => "info",
            Self::Warn => "warn",
            Self::Error => "error",
        }
    }
}

#[derive(Debug, Clone)]
pub struct DiagEntry {
    pub level: DiagLevel,
    pub message: String,
    /// Local clock seconds since epoch (for display only).
    pub ts_secs: u64,
}

#[derive(Debug, Clone)]
pub struct DiagnosticsState {
    pub entries: Vec<DiagEntry>,
    pub open: bool,
    /// Scroll offset from the bottom (0 = follow latest).
    pub scroll_from_bottom: u16,
}

impl Default for DiagnosticsState {
    fn default() -> Self {
        let mut s = Self {
            entries: Vec::new(),
            open: false,
            scroll_from_bottom: 0,
        };
        s.push(DiagLevel::Info, "diagnostics ready · F9 toggle · Esc close");
        s
    }
}

impl DiagnosticsState {
    pub fn push(&mut self, level: DiagLevel, message: impl Into<String>) {
        let ts_secs = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        self.entries.push(DiagEntry {
            level,
            message: message.into(),
            ts_secs,
        });
        if self.entries.len() > MAX_ENTRIES {
            let drop_n = self.entries.len() - MAX_ENTRIES;
            self.entries.drain(0..drop_n);
        }
        // New entries: stick to bottom unless user scrolled up.
        if self.scroll_from_bottom == 0 {
            // stay pinned
        }
    }

    pub fn toggle(&mut self) {
        self.open = !self.open;
        if self.open {
            self.scroll_from_bottom = 0;
        }
    }

    pub fn close(&mut self) {
        self.open = false;
    }

    pub fn clear(&mut self) {
        self.entries.clear();
        self.scroll_from_bottom = 0;
        self.push(DiagLevel::Info, "log cleared");
    }

    pub fn scroll_up(&mut self) {
        self.scroll_from_bottom = self.scroll_from_bottom.saturating_add(1);
        let max = self.entries.len().saturating_sub(1) as u16;
        if self.scroll_from_bottom > max {
            self.scroll_from_bottom = max;
        }
    }

    pub fn scroll_down(&mut self) {
        self.scroll_from_bottom = self.scroll_from_bottom.saturating_sub(1);
    }

    pub fn scroll_home(&mut self) {
        self.scroll_from_bottom = self.entries.len().saturating_sub(1) as u16;
    }

    pub fn scroll_end(&mut self) {
        self.scroll_from_bottom = 0;
    }
}
