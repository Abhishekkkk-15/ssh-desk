//! Editor and process-list state for Phase 7 apps.

use std::path::PathBuf;

use ssh_core::remote_path_string;

#[derive(Debug, Clone, Default)]
pub struct EditorState {
    pub path: Option<PathBuf>,
    pub title: String,
    pub lines: Vec<String>,
    pub cursor_row: usize,
    pub cursor_col: usize,
    pub scroll: u16,
    pub dirty: bool,
    pub online: bool,
    /// After warning on Esc with unsaved changes, next Esc discards.
    pub discard_armed: bool,
}

impl EditorState {
    pub fn clear(&mut self) {
        *self = Self::default();
    }

    pub fn is_open(&self) -> bool {
        self.path.is_some()
    }

    pub fn from_text(path: PathBuf, text: &str, online: bool) -> Self {
        let title = path
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| remote_path_string(&path));
        let mut lines: Vec<String> = text.lines().map(str::to_string).collect();
        if lines.is_empty() {
            lines.push(String::new());
        }
        Self {
            path: Some(path),
            title,
            lines,
            cursor_row: 0,
            cursor_col: 0,
            scroll: 0,
            dirty: false,
            online,
            discard_armed: false,
        }
    }

    pub fn contents(&self) -> String {
        let mut s = self.lines.join("\n");
        if !s.ends_with('\n') && !self.lines.is_empty() {
            // Preserve trailing newline if last line empty? join is fine without forced NL
        }
        s.push('\n');
        s
    }

    pub fn move_left(&mut self) {
        if self.cursor_col > 0 {
            self.cursor_col -= 1;
        } else if self.cursor_row > 0 {
            self.cursor_row -= 1;
            self.cursor_col = self.lines[self.cursor_row].chars().count();
        }
    }

    pub fn move_right(&mut self) {
        let len = self.lines[self.cursor_row].chars().count();
        if self.cursor_col < len {
            self.cursor_col += 1;
        } else if self.cursor_row + 1 < self.lines.len() {
            self.cursor_row += 1;
            self.cursor_col = 0;
        }
    }

    pub fn move_up(&mut self) {
        if self.cursor_row > 0 {
            self.cursor_row -= 1;
            let len = self.lines[self.cursor_row].chars().count();
            self.cursor_col = self.cursor_col.min(len);
            if self.cursor_row < self.scroll as usize {
                self.scroll = self.cursor_row as u16;
            }
        }
    }

    pub fn move_down(&mut self) {
        if self.cursor_row + 1 < self.lines.len() {
            self.cursor_row += 1;
            let len = self.lines[self.cursor_row].chars().count();
            self.cursor_col = self.cursor_col.min(len);
        }
    }

    pub fn insert_char(&mut self, c: char) {
        let line = &mut self.lines[self.cursor_row];
        let idx = byte_index(line, self.cursor_col);
        line.insert(idx, c);
        self.cursor_col += 1;
        self.dirty = true;
        self.discard_armed = false;
    }

    pub fn insert_newline(&mut self) {
        let line = &mut self.lines[self.cursor_row];
        let idx = byte_index(line, self.cursor_col);
        let rest = line.split_off(idx);
        self.cursor_row += 1;
        self.lines.insert(self.cursor_row, rest);
        self.cursor_col = 0;
        self.dirty = true;
        self.discard_armed = false;
    }

    pub fn backspace(&mut self) {
        if self.cursor_col > 0 {
            let line = &mut self.lines[self.cursor_row];
            let idx = byte_index(line, self.cursor_col - 1);
            let end = byte_index(line, self.cursor_col);
            line.replace_range(idx..end, "");
            self.cursor_col -= 1;
            self.dirty = true;
            self.discard_armed = false;
        } else if self.cursor_row > 0 {
            let cur = self.lines.remove(self.cursor_row);
            self.cursor_row -= 1;
            self.cursor_col = self.lines[self.cursor_row].chars().count();
            self.lines[self.cursor_row].push_str(&cur);
            self.dirty = true;
            self.discard_armed = false;
        }
    }

    pub fn ensure_visible(&mut self, height: u16) {
        let h = height.max(1) as usize;
        if self.cursor_row < self.scroll as usize {
            self.scroll = self.cursor_row as u16;
        } else if self.cursor_row >= self.scroll as usize + h {
            self.scroll = (self.cursor_row + 1 - h) as u16;
        }
    }
}

fn byte_index(s: &str, char_idx: usize) -> usize {
    s.char_indices()
        .nth(char_idx)
        .map(|(i, _)| i)
        .unwrap_or(s.len())
}

#[derive(Debug, Clone)]
pub struct ProcessRow {
    pub pid: String,
    pub user: String,
    pub cpu: String,
    pub mem: String,
    pub command: String,
}

#[derive(Debug, Clone, Default)]
pub struct ProcessesState {
    pub rows: Vec<ProcessRow>,
    pub selected: usize,
    pub error: Option<String>,
    pub loading: bool,
    pub online: bool,
}

impl ProcessesState {
    pub fn from_ps(output: &str) -> Self {
        let mut rows = Vec::new();
        for (i, line) in output.lines().enumerate() {
            if i == 0 && line.to_ascii_lowercase().contains("pid") {
                continue;
            }
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            // Prefer: PID USER %CPU %MEM COMMAND…
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() < 5 {
                continue;
            }
            // Heuristic: first token is pid if numeric
            if !parts[0].chars().all(|c| c.is_ascii_digit()) {
                continue;
            }
            let command = if parts.len() > 4 {
                parts[4..].join(" ")
            } else {
                String::new()
            };
            rows.push(ProcessRow {
                pid: parts[0].into(),
                user: parts[1].into(),
                cpu: parts[2].into(),
                mem: parts[3].into(),
                command,
            });
        }
        Self {
            rows,
            selected: 0,
            error: None,
            loading: false,
            online: true,
        }
    }

    pub fn demo() -> Self {
        Self {
            rows: vec![
                ProcessRow {
                    pid: "1".into(),
                    user: "root".into(),
                    cpu: "0.0".into(),
                    mem: "0.1".into(),
                    command: "systemd".into(),
                },
                ProcessRow {
                    pid: "428".into(),
                    user: "www".into(),
                    cpu: "1.2".into(),
                    mem: "2.4".into(),
                    command: "nginx".into(),
                },
            ],
            selected: 0,
            error: Some("offline · demo process list".into()),
            loading: false,
            online: false,
        }
    }

    pub fn move_up(&mut self) {
        if self.selected > 0 {
            self.selected -= 1;
        }
    }

    pub fn move_down(&mut self) {
        if !self.rows.is_empty() && self.selected + 1 < self.rows.len() {
            self.selected += 1;
        }
    }
}
