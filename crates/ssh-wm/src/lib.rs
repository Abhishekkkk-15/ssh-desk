//! Window manager: binary split tree of panes for the ssh-desk desktop.

mod layout;
mod tree;

pub use layout::{Layout, SavedLayout};
pub use tree::{AppKind, Direction, NodeId, PaneNode, PaneTree, Split};

use serde::{Deserialize, Serialize};

/// Identifier for a desktop session (usually one SSH host).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct DesktopId(pub String);

impl DesktopId {
    pub fn new(name: impl Into<String>) -> Self {
        Self(name.into())
    }
}

/// High-level desktop: a pane tree plus focus and optional title.
#[derive(Debug, Clone)]
pub struct Desktop {
    pub id: DesktopId,
    pub title: String,
    pub tree: PaneTree,
}

impl Desktop {
    pub fn new(id: impl Into<String>, title: impl Into<String>) -> Self {
        let mut tree = PaneTree::new(AppKind::Terminal);
        // Seed a familiar OS-like layout: terminal | files over processes | transfers
        let root = tree.root();
        let right = tree
            .split(root, Direction::Vertical, 0.55, AppKind::Files)
            .expect("split root");
        let _ = tree.split(root, Direction::Horizontal, 0.7, AppKind::Processes);
        let _ = tree.split(right, Direction::Horizontal, 0.65, AppKind::Transfers);

        Self {
            id: DesktopId::new(id),
            title: title.into(),
            tree,
        }
    }

    pub fn focus_next(&mut self) {
        self.tree.focus_next();
    }

    pub fn focus_prev(&mut self) {
        self.tree.focus_prev();
    }

    pub fn focused_app(&self) -> AppKind {
        self.tree.focused_app()
    }
}
