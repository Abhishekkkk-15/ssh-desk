//! Layout hit-testing for mouse focus and in-TUI drag-and-drop.

use std::path::PathBuf;

use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ssh_os::DropTarget;
use ssh_wm::{AppKind, Desktop, Direction as SplitDir, NodeId, PaneNode};

use crate::files::{FilesRow, FilesState};

#[derive(Debug, Clone)]
pub struct PaneHit {
    pub id: NodeId,
    #[allow(dead_code)]
    pub app: AppKind,
    pub area: Rect,
    pub inner: Rect,
}

#[derive(Debug, Clone)]
pub struct FilesRowHit {
    pub row_index: usize,
    pub entry_index: Option<usize>,
    pub is_dir: bool,
    pub path: PathBuf,
    pub area: Rect,
}

#[derive(Debug, Clone, Default)]
pub struct FrameGeo {
    pub panes: Vec<PaneHit>,
    pub files_rows: Vec<FilesRowHit>,
    pub files_pane_inner: Option<Rect>,
    pub transfers_pane: Option<Rect>,
    pub dock: Rect,
    #[allow(dead_code)]
    pub content: Rect,
}

impl FrameGeo {
    pub fn pane_at(&self, x: u16, y: u16) -> Option<&PaneHit> {
        self.panes
            .iter()
            .find(|p| contains(p.area, x, y))
            .or_else(|| self.panes.iter().find(|p| contains(p.inner, x, y)))
    }

    pub fn files_row_at(&self, x: u16, y: u16) -> Option<&FilesRowHit> {
        self.files_rows.iter().find(|r| contains(r.area, x, y))
    }

    pub fn drop_target_at(&self, x: u16, y: u16, files_cwd: &PathBuf) -> DropTarget {
        if contains(self.dock, x, y) {
            return DropTarget::TransferDock;
        }
        if let Some(tr) = self.transfers_pane {
            if contains(tr, x, y) {
                return DropTarget::TransferDock;
            }
        }
        if let Some(row) = self.files_row_at(x, y) {
            if row.is_dir {
                return DropTarget::Folder {
                    pane_hint: "files".into(),
                    path: row.path.clone(),
                };
            }
            // Dropping onto a file → parent directory (cwd of listing)
            return DropTarget::Folder {
                pane_hint: "files".into(),
                path: files_cwd.clone(),
            };
        }
        if let Some(inner) = self.files_pane_inner {
            if contains(inner, x, y) {
                return DropTarget::Folder {
                    pane_hint: "files".into(),
                    path: files_cwd.clone(),
                };
            }
        }
        DropTarget::Ask
    }
}

fn contains(r: Rect, x: u16, y: u16) -> bool {
    x >= r.x && x < r.x.saturating_add(r.width) && y >= r.y && y < r.y.saturating_add(r.height)
}

/// Mirror the desktop draw layout so mouse coords map to panes/rows.
pub fn compute_frame_geo(
    term: Rect,
    desktop: &Desktop,
    files: &FilesState,
    fullscreen_app: Option<AppKind>,
    chrome_hidden: bool,
) -> FrameGeo {
    let (content, dock) = if chrome_hidden {
        (term, Rect::default())
    } else {
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Length(1),
                Constraint::Min(3),
                Constraint::Length(1),
                Constraint::Length(1),
            ])
            .split(term);
        (chunks[1], chunks[2])
    };

    let mut geo = FrameGeo {
        dock,
        content,
        ..FrameGeo::default()
    };

    // F11 pane fullscreen uses the whole content area (draw_desktop does the same).
    if let Some(app) = fullscreen_app {
        let block_inner = inset_border(content);
        let inner = inset_header_strip(block_inner);
        geo.panes.push(PaneHit {
            id: desktop.tree.focused(),
            app,
            area: content,
            inner,
        });
        match app {
            AppKind::Files => {
                geo.files_pane_inner = Some(inner);
                geo.files_rows = files_row_hits(inner, files);
            }
            AppKind::Transfers => geo.transfers_pane = Some(inner),
            _ => {}
        }
        return geo;
    }

    collect_panes(
        content,
        desktop.tree.root_node(),
        &mut geo.panes,
        &mut geo.files_pane_inner,
        &mut geo.transfers_pane,
    );

    if let Some(inner) = geo.files_pane_inner {
        geo.files_rows = files_row_hits(inner, files);
    }

    geo
}

fn collect_panes(
    area: Rect,
    node: &PaneNode,
    out: &mut Vec<PaneHit>,
    files_inner: &mut Option<Rect>,
    transfers: &mut Option<Rect>,
) {
    match node {
        PaneNode::Leaf { id, app } => {
            // Match draw_pane_leaf: border inset, then 1-row header strip, then body.
            let block_inner = inset_border(area);
            let content = inset_header_strip(block_inner);
            out.push(PaneHit {
                id: *id,
                app: *app,
                area,
                inner: content,
            });
            match app {
                AppKind::Files => *files_inner = Some(content),
                AppKind::Transfers => *transfers = Some(content),
                _ => {}
            }
        }
        PaneNode::Split(split) => {
            let (a, b) = ratio_constraints(split.ratio);
            let dir = match split.direction {
                SplitDir::Vertical => Direction::Horizontal,
                SplitDir::Horizontal => Direction::Vertical,
            };
            let chunks = Layout::default()
                .direction(dir)
                .constraints([a, b])
                .split(area);
            collect_panes(chunks[0], &split.first, out, files_inner, transfers);
            collect_panes(chunks[1], &split.second, out, files_inner, transfers);
        }
    }
}

fn ratio_constraints(ratio: f32) -> (Constraint, Constraint) {
    let a = ((ratio * 100.0).round() as u16).clamp(15, 85);
    let b = 100 - a;
    (Constraint::Percentage(a), Constraint::Percentage(b))
}

fn inset_border(area: Rect) -> Rect {
    if area.width < 2 || area.height < 2 {
        return area;
    }
    Rect {
        x: area.x + 1,
        y: area.y + 1,
        width: area.width - 2,
        height: area.height - 2,
    }
}

/// Skip the 1-row pane header strip under the border.
fn inset_header_strip(inner: Rect) -> Rect {
    if inner.height <= 1 {
        return Rect {
            x: inner.x,
            y: inner.y,
            width: inner.width,
            height: 0,
        };
    }
    Rect {
        x: inner.x,
        y: inner.y + 1,
        width: inner.width,
        height: inner.height - 1,
    }
}

fn files_row_hits(area: Rect, files: &FilesState) -> Vec<FilesRowHit> {
    let mut y_off = 0u16;
    if files.error.is_some() {
        y_off = y_off.saturating_add(1);
    }
    if files.loading {
        y_off = y_off.saturating_add(1);
    }
    // Column header: MODE · SIZE · MODIFIED · NAME
    y_off = y_off.saturating_add(1);

    let rows = files.rows();
    let visible = area.height.saturating_sub(y_off) as usize;
    if visible == 0 {
        return Vec::new();
    }
    let start = files.selected.saturating_sub(visible.saturating_sub(1));

    let mut hits = Vec::new();
    for (vis_i, (idx, row)) in rows
        .iter()
        .enumerate()
        .skip(start)
        .take(visible)
        .enumerate()
    {
        let y = area.y + y_off + vis_i as u16;
        if y >= area.y + area.height {
            break;
        }
        let (entry_index, is_dir, path) = match row {
            FilesRow::Parent => (
                None,
                true,
                files
                    .cwd
                    .parent()
                    .map(PathBuf::from)
                    .unwrap_or_else(|| PathBuf::from("/")),
            ),
            FilesRow::Entry(i) => {
                let e = match files.entries.get(*i) {
                    Some(e) => e,
                    None => continue,
                };
                (Some(*i), e.is_dir, e.path.clone())
            }
        };
        hits.push(FilesRowHit {
            row_index: idx,
            entry_index,
            is_dir,
            path,
            area: Rect {
                x: area.x,
                y,
                width: area.width,
                height: 1,
            },
        });
    }
    hits
}
