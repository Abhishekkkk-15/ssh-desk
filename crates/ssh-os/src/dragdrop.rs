//! In-TUI and OS path-drop scaffolding.

use std::path::PathBuf;

use uuid::Uuid;

use crate::clipboard::FileEntry;

/// What is being dragged.
#[derive(Debug, Clone)]
pub enum DragPayload {
    Files(Vec<FileEntry>),
    /// Paths dropped from the host OS / terminal emulator.
    OsPaths(Vec<PathBuf>),
}

/// Active drag session (mouse gesture inside the TUI).
#[derive(Debug, Clone)]
pub struct DragSession {
    pub id: Uuid,
    pub payload: DragPayload,
    /// Cursor cell when drag started.
    pub origin: (u16, u16),
    pub current: (u16, u16),
}

impl DragSession {
    pub fn start(payload: DragPayload, origin: (u16, u16)) -> Self {
        Self {
            id: Uuid::new_v4(),
            payload,
            origin,
            current: origin,
        }
    }

    pub fn move_to(&mut self, pos: (u16, u16)) {
        self.current = pos;
    }
}

/// Where a drop can land.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DropTarget {
    /// Remote or local folder in the Files app.
    Folder { pane_hint: String, path: PathBuf },
    TransferDock,
    /// Unknown / ask the user.
    Ask,
}

impl DropTarget {
    /// Resolve OS path drops into an upload/copy intent description.
    pub fn describe(&self) -> String {
        match self {
            Self::Folder { path, .. } => format!("drop → {}", path.display()),
            Self::TransferDock => "drop → transfer queue".into(),
            Self::Ask => "drop → choose destination".into(),
        }
    }
}
