//! Files browser and Viewer state for the desktop shell.

use std::path::{Path, PathBuf};

use ssh_core::{RemoteEntry, RemoteFileContent, join_remote, remote_path_string};
use ssh_os::{HalfblockPreview, OpenAction, sniff_open_action};

#[derive(Debug, Clone)]
pub struct FilesState {
    pub cwd: PathBuf,
    pub entries: Vec<RemoteEntry>,
    /// Index into `rows()` (includes synthetic `..` when not at root).
    pub selected: usize,
    /// Selected entry indices (into `entries`, not rows).
    pub marked: Vec<usize>,
    pub loading: bool,
    pub error: Option<String>,
    pub online: bool,
    pub search_query: Option<String>,
}

impl Default for FilesState {
    fn default() -> Self {
        Self {
            cwd: PathBuf::from("/"),
            entries: Vec::new(),
            selected: 0,
            marked: Vec::new(),
            loading: false,
            error: None,
            online: false,
            search_query: None,
        }
    }
}

impl FilesState {
    pub fn demo() -> Self {
        let cwd = PathBuf::from("/home/demo");
        let entries = vec![
            RemoteEntry {
                name: "Documents".into(),
                path: cwd.join("Documents"),
                is_dir: true,
                is_symlink: false,
                size: None,
                permissions: Some(0o040755),
                mtime: None,
            },
            RemoteEntry {
                name: "notes.txt".into(),
                path: cwd.join("notes.txt"),
                is_dir: false,
                is_symlink: false,
                size: Some(42),
                permissions: Some(0o100644),
                mtime: None,
            },
            RemoteEntry {
                name: "readme.md".into(),
                path: cwd.join("readme.md"),
                is_dir: false,
                is_symlink: false,
                size: Some(120),
                permissions: Some(0o100644),
                mtime: None,
            },
        ];
        Self {
            cwd,
            entries,
            selected: 0,
            marked: Vec::new(),
            loading: false,
            error: Some("offline · demo listing (connect for live SFTP)".into()),
            online: false,
            search_query: None,
        }
    }

    pub fn set_listing(&mut self, cwd: PathBuf, entries: Vec<RemoteEntry>) {
        self.cwd = cwd;
        self.entries = entries;
        self.selected = 0;
        self.marked.clear();
        self.loading = false;
        self.error = None;
        self.online = true;
        self.search_query = None;
    }

    pub fn rows(&self) -> Vec<FilesRow> {
        let mut rows = Vec::new();
        if self.cwd != Path::new("/") && self.search_query.is_none() {
            rows.push(FilesRow::Parent);
        }
        for (i, entry) in self.entries.iter().enumerate() {
            if let Some(query) = &self.search_query {
                if entry.name.to_lowercase().contains(&query.to_lowercase()) {
                    rows.push(FilesRow::Entry(i));
                }
            } else {
                rows.push(FilesRow::Entry(i));
            }
        }
        rows
    }

    pub fn move_up(&mut self) {
        if self.selected > 0 {
            self.selected -= 1;
        }
    }

    pub fn move_down(&mut self) {
        let len = self.rows().len();
        if len > 0 && self.selected + 1 < len {
            self.selected += 1;
        }
    }

    pub fn selected_row(&self) -> Option<FilesRow> {
        self.rows().get(self.selected).copied()
    }

    pub fn toggle_mark_selected(&mut self) {
        let Some(FilesRow::Entry(i)) = self.selected_row() else {
            return;
        };
        if let Some(pos) = self.marked.iter().position(|m| *m == i) {
            self.marked.remove(pos);
        } else {
            self.marked.push(i);
        }
    }

    pub fn clear_marks(&mut self) {
        self.marked.clear();
    }

    pub fn is_marked(&self, entry_idx: usize) -> bool {
        self.marked.contains(&entry_idx)
    }

    /// Entries for clipboard: marked set, or the single focused entry.
    pub fn clipboard_targets(&self) -> Vec<&RemoteEntry> {
        if !self.marked.is_empty() {
            return self
                .marked
                .iter()
                .filter_map(|i| self.entries.get(*i))
                .collect();
        }
        match self.selected_row() {
            Some(FilesRow::Entry(i)) => self.entries.get(i).into_iter().collect(),
            _ => Vec::new(),
        }
    }

    pub fn cwd_display(&self) -> String {
        remote_path_string(&self.cwd)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FilesRow {
    Parent,
    Entry(usize),
}

#[derive(Debug, Clone)]
pub enum ViewerKind {
    Text,
    Hex,
    Image(HalfblockPreview),
}

impl Default for ViewerKind {
    fn default() -> Self {
        Self::Text
    }
}

#[derive(Debug, Clone, Default)]
pub struct ViewerState {
    pub path: Option<PathBuf>,
    pub title: String,
    pub body: String,
    pub scroll: u16,
    pub binary: bool,
    pub truncated: bool,
    pub kind: ViewerKind,
    /// Raw bytes kept so image previews can re-rasterize on resize.
    pub image_bytes: Option<Vec<u8>>,
}

impl ViewerState {
    pub fn clear(&mut self) {
        *self = Self::default();
    }

    pub fn is_open(&self) -> bool {
        self.path.is_some()
    }

    pub fn from_content_on(
        content: RemoteFileContent,
        preview_cols: u16,
        preview_rows: u16,
        bg_rgb: [u8; 3],
    ) -> Self {
        let path = content.path.clone();
        let title = path
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| remote_path_string(&path));
        let truncated = content.truncated;
        let action = sniff_open_action(&path);

        if matches!(action, OpenAction::PreviewImage) {
            match HalfblockPreview::from_bytes_on(
                &content.bytes,
                preview_cols,
                preview_rows,
                bg_rgb,
            ) {
                Ok(preview) => {
                    let meta = preview.meta.clone();
                    let trunc_note = if truncated {
                        "  [truncated — preview may look wrong]"
                    } else {
                        ""
                    };
                    return Self {
                        path: Some(path),
                        title,
                        body: format!("{meta}{trunc_note}"),
                        scroll: 0,
                        binary: false,
                        truncated,
                        kind: ViewerKind::Image(preview),
                        image_bytes: Some(content.bytes),
                    };
                }
                Err(e) => {
                    return Self {
                        path: Some(path),
                        title,
                        body: format!(
                            "image decode failed: {e}{}\n\n{}",
                            if truncated {
                                " (file truncated on download)"
                            } else {
                                ""
                            },
                            content.hex_preview(2048)
                        ),
                        scroll: 0,
                        binary: true,
                        truncated,
                        kind: ViewerKind::Hex,
                        image_bytes: None,
                    };
                }
            }
        }

        let force_hex = content.looks_binary() || matches!(action, OpenAction::Hex);
        let (mut body, kind, binary) = if force_hex {
            (content.hex_preview(4096), ViewerKind::Hex, true)
        } else {
            (
                content
                    .as_text()
                    .map(|s| s.to_string())
                    .unwrap_or_else(|| content.hex_preview(4096)),
                ViewerKind::Text,
                content.as_text().is_none(),
            )
        };

        if truncated {
            body.push_str("\n\n… truncated at download limit …\n");
        }

        Self {
            path: Some(path),
            title,
            body,
            scroll: 0,
            binary,
            truncated,
            kind,
            image_bytes: None,
        }
    }

    /// Re-rasterize a stored image to a new Viewer pane size.
    pub fn refit_image(&mut self, cols: u16, rows: u16, bg_rgb: [u8; 3]) -> bool {
        let Some(bytes) = self.image_bytes.as_ref() else {
            return false;
        };
        let Ok(preview) = HalfblockPreview::from_bytes_on(bytes, cols, rows, bg_rgb) else {
            return false;
        };
        if let ViewerKind::Image(old) = &self.kind {
            if old.width == preview.width && old.height == preview.height {
                return false;
            }
        }
        let meta = preview.meta.clone();
        let trunc_note = if self.truncated {
            "  [truncated — preview may look wrong]"
        } else {
            ""
        };
        self.body = format!("{meta}{trunc_note}");
        self.kind = ViewerKind::Image(preview);
        self.scroll = 0;
        true
    }

    pub fn demo_file(path: &Path) -> Self {
        let name = path
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "file".into());
        let body = match name.as_str() {
            "notes.txt" => {
                "Demo notes\n\nConnect over SSH to browse and open real remote files via SFTP.\n"
                    .into()
            }
            "readme.md" => {
                "# ssh-desk\n\nRemote OS shell in the terminal.\n\nOpen files from the Files pane.\n"
                    .into()
            }
            _ => format!("(demo) contents of {name}\n"),
        };
        Self {
            path: Some(path.to_path_buf()),
            title: name,
            body,
            scroll: 0,
            binary: false,
            truncated: false,
            kind: ViewerKind::Text,
            image_bytes: None,
        }
    }

    pub fn scroll_by(&mut self, delta: i32) {
        if delta < 0 {
            self.scroll = self.scroll.saturating_sub((-delta) as u16);
        } else {
            self.scroll = self.scroll.saturating_add(delta as u16);
        }
    }
}

pub fn resolve_open_path(
    cwd: &Path,
    row: FilesRow,
    entries: &[RemoteEntry],
) -> Option<(PathBuf, bool)> {
    match row {
        FilesRow::Parent => Some((join_remote(cwd, ".."), true)),
        FilesRow::Entry(i) => entries.get(i).map(|e| (e.path.clone(), e.is_dir)),
    }
}
