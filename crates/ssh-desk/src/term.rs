//! VT100 terminal emulator state for the Terminal pane.

use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use vt100::{Color as VtColor, Parser};

/// Scrollback-backed VT100 emulator fed by remote PTY bytes.
pub struct TermEmulator {
    parser: Parser,
    cols: u16,
    rows: u16,
}

impl Default for TermEmulator {
    fn default() -> Self {
        Self::new(24, 80)
    }
}

impl std::fmt::Debug for TermEmulator {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TermEmulator")
            .field("cols", &self.cols)
            .field("rows", &self.rows)
            .finish()
    }
}

impl TermEmulator {
    pub fn new(rows: u16, cols: u16) -> Self {
        let rows = rows.max(1);
        let cols = cols.max(1);
        Self {
            parser: Parser::new(rows, cols, 2_000),
            cols,
            rows,
        }
    }

    pub fn process(&mut self, bytes: &[u8]) {
        self.parser.process(bytes);
    }

    pub fn write_str(&mut self, s: &str) {
        self.process(s.as_bytes());
    }

    pub fn resize(&mut self, rows: u16, cols: u16) {
        let rows = rows.max(1);
        let cols = cols.max(1);
        if rows == self.rows && cols == self.cols {
            return;
        }
        self.parser.set_size(rows, cols);
        self.rows = rows;
        self.cols = cols;
    }

    pub fn size(&self) -> (u16, u16) {
        (self.rows, self.cols)
    }

    /// Render visible screen into ratatui lines (one Line per row).
    pub fn lines(&self) -> Vec<Line<'static>> {
        let screen = self.parser.screen();
        let (rows, cols) = screen.size();
        let (cy, cx) = screen.cursor_position();
        let mut out = Vec::with_capacity(rows as usize);

        for row in 0..rows {
            let mut spans: Vec<Span<'static>> = Vec::new();
            let mut col = 0u16;
            while col < cols {
                let Some(cell) = screen.cell(row, col) else {
                    col += 1;
                    continue;
                };
                if cell.is_wide_continuation() {
                    col += 1;
                    continue;
                }
                let mut contents = cell.contents();
                if contents.is_empty() {
                    contents = " ".into();
                }
                let width = if cell.is_wide() { 2u16 } else { 1u16 };

                let mut style = Style::default()
                    .fg(map_color(cell.fgcolor()))
                    .bg(map_color(cell.bgcolor()));
                let mut mods = Modifier::empty();
                if cell.bold() {
                    mods |= Modifier::BOLD;
                }
                if cell.italic() {
                    mods |= Modifier::ITALIC;
                }
                if cell.underline() {
                    mods |= Modifier::UNDERLINED;
                }
                if cell.inverse() {
                    mods |= Modifier::REVERSED;
                }
                style = style.add_modifier(mods);

                if row == cy && col == cx {
                    style = style.add_modifier(Modifier::REVERSED);
                }

                spans.push(Span::styled(contents, style));
                col = col.saturating_add(width);
            }
            out.push(Line::from(spans));
        }
        out
    }
}

fn map_color(c: VtColor) -> Color {
    match c {
        VtColor::Default => Color::Reset,
        VtColor::Idx(i) => Color::Indexed(i),
        VtColor::Rgb(r, g, b) => Color::Rgb(r, g, b),
    }
}
