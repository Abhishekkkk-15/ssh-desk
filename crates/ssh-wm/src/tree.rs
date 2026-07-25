//! Binary split tree of application panes.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Stable identity for a pane node.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct NodeId(pub Uuid);

impl NodeId {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

impl Default for NodeId {
    fn default() -> Self {
        Self::new()
    }
}

/// Application hosted in a leaf pane.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum AppKind {
    Terminal,
    Files,
    Viewer,
    Editor,
    Transfers,
    Processes,
    Launcher,
}

impl AppKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::Terminal => "shell",
            Self::Files => "files",
            Self::Viewer => "viewer",
            Self::Editor => "editor",
            Self::Transfers => "transfers",
            Self::Processes => "processes",
            Self::Launcher => "hosts",
        }
    }

    pub fn all_dock() -> &'static [AppKind] {
        &[
            Self::Terminal,
            Self::Files,
            Self::Viewer,
            Self::Editor,
            Self::Transfers,
            Self::Processes,
        ]
    }
}

/// Why a pane could not be closed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClosePaneError {
    /// Refusing to close the only remaining pane.
    LastPane,
    NotFound,
}

/// Split axis.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Direction {
    /// Left | Right
    Vertical,
    /// Top / Bottom
    Horizontal,
}

/// A split with ratio for the first child (0.0–1.0).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Split {
    pub direction: Direction,
    /// Fraction of space for the first child.
    pub ratio: f32,
    pub first: Box<PaneNode>,
    pub second: Box<PaneNode>,
}

/// Tree node: leaf app or split.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum PaneNode {
    Leaf { id: NodeId, app: AppKind },
    Split(Split),
}

impl PaneNode {
    pub fn leaf(app: AppKind) -> Self {
        Self::Leaf {
            id: NodeId::new(),
            app,
        }
    }

    pub fn id(&self) -> Option<NodeId> {
        match self {
            Self::Leaf { id, .. } => Some(*id),
            Self::Split(_) => None,
        }
    }

    pub fn leaves(&self) -> Vec<(NodeId, AppKind)> {
        match self {
            Self::Leaf { id, app } => vec![(*id, *app)],
            Self::Split(s) => {
                let mut out = s.first.leaves();
                out.extend(s.second.leaves());
                out
            }
        }
    }

    fn find_mut(&mut self, id: NodeId) -> Option<&mut PaneNode> {
        match self {
            Self::Leaf { id: leaf_id, .. } if *leaf_id == id => Some(self),
            Self::Leaf { .. } => None,
            Self::Split(s) => s
                .first
                .find_mut(id)
                .or_else(|| s.second.find_mut(id)),
        }
    }

    fn replace_leaf_with_split(
        &mut self,
        target: NodeId,
        direction: Direction,
        ratio: f32,
        new_app: AppKind,
    ) -> Option<NodeId> {
        match self {
            Self::Leaf { id, app } if *id == target => {
                let first_app = *app;
                let second_id = NodeId::new();
                *self = Self::Split(Split {
                    direction,
                    ratio: ratio.clamp(0.15, 0.85),
                    first: Box::new(Self::Leaf {
                        id: target,
                        app: first_app,
                    }),
                    second: Box::new(Self::Leaf {
                        id: second_id,
                        app: new_app,
                    }),
                });
                Some(second_id)
            }
            Self::Leaf { .. } => None,
            Self::Split(s) => s
                .first
                .replace_leaf_with_split(target, direction, ratio, new_app)
                .or_else(|| {
                    s.second
                        .replace_leaf_with_split(target, direction, ratio, new_app)
                }),
        }
    }

    /// Remove `target` leaf; promote its sibling. Returns the sibling to focus.
    fn remove_leaf(&mut self, target: NodeId) -> Option<NodeId> {
        match self {
            Self::Leaf { .. } => None,
            Self::Split(s) => {
                let first_hit = matches!(s.first.as_ref(), Self::Leaf { id, .. } if *id == target);
                let second_hit = matches!(s.second.as_ref(), Self::Leaf { id, .. } if *id == target);

                if first_hit {
                    let sibling = std::mem::replace(
                        &mut s.second,
                        Box::new(Self::Leaf {
                            id: NodeId::new(),
                            app: AppKind::Terminal,
                        }),
                    );
                    let focus = sibling.leaves().first().map(|(id, _)| *id);
                    *self = *sibling;
                    return focus;
                }
                if second_hit {
                    let sibling = std::mem::replace(
                        &mut s.first,
                        Box::new(Self::Leaf {
                            id: NodeId::new(),
                            app: AppKind::Terminal,
                        }),
                    );
                    let focus = sibling.leaves().first().map(|(id, _)| *id);
                    *self = *sibling;
                    return focus;
                }

                if let Some(focus) = s.first.remove_leaf(target) {
                    return Some(focus);
                }
                s.second.remove_leaf(target)
            }
        }
    }
}

/// Owned pane tree with focus tracking.
#[derive(Debug, Clone)]
pub struct PaneTree {
    pub(crate) root: PaneNode,
    pub(crate) focused: NodeId,
}

impl PaneTree {
    pub fn new(app: AppKind) -> Self {
        let id = NodeId::new();
        Self {
            root: PaneNode::Leaf { id, app },
            focused: id,
        }
    }

    pub fn root(&self) -> NodeId {
        match &self.root {
            PaneNode::Leaf { id, .. } => *id,
            PaneNode::Split(_) => self.focused,
        }
    }

    pub fn root_node(&self) -> &PaneNode {
        &self.root
    }

    pub fn focused(&self) -> NodeId {
        self.focused
    }

    pub fn focused_app(&self) -> AppKind {
        self.leaves()
            .into_iter()
            .find(|(id, _)| *id == self.focused)
            .map(|(_, app)| app)
            .unwrap_or(AppKind::Terminal)
    }

    pub fn set_focus(&mut self, id: NodeId) {
        if self.leaves().iter().any(|(leaf, _)| *leaf == id) {
            self.focused = id;
        }
    }

    pub fn leaves(&self) -> Vec<(NodeId, AppKind)> {
        self.root.leaves()
    }

    pub fn has_app(&self, app: AppKind) -> bool {
        self.leaves().iter().any(|(_, a)| *a == app)
    }

    pub fn find_app(&self, app: AppKind) -> Option<NodeId> {
        self.leaves()
            .into_iter()
            .find(|(_, a)| *a == app)
            .map(|(id, _)| id)
    }

    pub fn focus_next(&mut self) {
        let leaves = self.leaves();
        if leaves.is_empty() {
            return;
        }
        let idx = leaves
            .iter()
            .position(|(id, _)| *id == self.focused)
            .unwrap_or(0);
        let next = (idx + 1) % leaves.len();
        self.focused = leaves[next].0;
    }

    pub fn focus_prev(&mut self) {
        let leaves = self.leaves();
        if leaves.is_empty() {
            return;
        }
        let idx = leaves
            .iter()
            .position(|(id, _)| *id == self.focused)
            .unwrap_or(0);
        let prev = if idx == 0 { leaves.len() - 1 } else { idx - 1 };
        self.focused = leaves[prev].0;
    }

    /// Split the focused (or given) leaf; returns the new pane id.
    pub fn split(
        &mut self,
        target: NodeId,
        direction: Direction,
        ratio: f32,
        new_app: AppKind,
    ) -> Option<NodeId> {
        let new_id =
            self.root
                .replace_leaf_with_split(target, direction, ratio, new_app)?;
        self.focused = new_id;
        Some(new_id)
    }

    pub fn split_focused(
        &mut self,
        direction: Direction,
        ratio: f32,
        new_app: AppKind,
    ) -> Option<NodeId> {
        self.split(self.focused, direction, ratio, new_app)
    }

    /// Close a leaf pane; sibling expands. Focus moves to the sibling subtree.
    pub fn close(&mut self, id: NodeId) -> Result<NodeId, ClosePaneError> {
        if matches!(&self.root, PaneNode::Leaf { .. }) {
            return Err(ClosePaneError::LastPane);
        }
        let Some(next_focus) = self.root.remove_leaf(id) else {
            return Err(ClosePaneError::NotFound);
        };
        let leaves = self.leaves();
        self.focused = if leaves.iter().any(|(lid, _)| *lid == next_focus) {
            next_focus
        } else {
            leaves
                .first()
                .map(|(lid, _)| *lid)
                .unwrap_or(next_focus)
        };
        Ok(self.focused)
    }

    pub fn close_focused(&mut self) -> Result<NodeId, ClosePaneError> {
        self.close(self.focused)
    }

    pub fn set_app(&mut self, id: NodeId, app: AppKind) {
        if let Some(PaneNode::Leaf { app: slot, .. }) = self.root.find_mut(id) {
            *slot = app;
        }
    }

    pub fn set_focused_app(&mut self, app: AppKind) {
        self.set_app(self.focused, app);
    }

    /// Focus `app` if already open; otherwise open it to the **right** of the focused pane.
    /// Returns `true` when a new pane was created.
    pub fn focus_or_open(&mut self, app: AppKind) -> bool {
        if let Some(id) = self.find_app(app) {
            self.focused = id;
            return false;
        }
        self.split_focused(Direction::Vertical, 0.5, app).is_some()
    }

    /// Focus an existing Viewer pane, or open one beside the focused pane.
    pub fn focus_or_open_viewer(&mut self) {
        let _ = self.focus_or_open(AppKind::Viewer);
    }

    /// Focus an existing Editor pane, or open one beside the focused pane.
    pub fn focus_or_open_editor(&mut self) {
        let _ = self.focus_or_open(AppKind::Editor);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn close_promotes_sibling() {
        let mut tree = PaneTree::new(AppKind::Terminal);
        let root = tree.root();
        let files = tree
            .split(root, Direction::Vertical, 0.5, AppKind::Files)
            .unwrap();
        assert_eq!(tree.leaves().len(), 2);
        tree.close(files).unwrap();
        assert_eq!(tree.leaves().len(), 1);
        assert_eq!(tree.focused_app(), AppKind::Terminal);
    }

    #[test]
    fn cannot_close_last_pane() {
        let mut tree = PaneTree::new(AppKind::Files);
        assert_eq!(tree.close_focused(), Err(ClosePaneError::LastPane));
    }

    #[test]
    fn close_nested_default_layout() {
        // Mirror Desktop::new seeding
        let mut tree = PaneTree::new(AppKind::Terminal);
        let root = tree.root();
        let right = tree
            .split(root, Direction::Vertical, 0.55, AppKind::Files)
            .unwrap();
        let _ = tree.split(root, Direction::Horizontal, 0.7, AppKind::Processes);
        let _ = tree.split(right, Direction::Horizontal, 0.65, AppKind::Transfers);
        assert_eq!(tree.leaves().len(), 4);

        tree.close_focused().unwrap();
        assert_eq!(tree.leaves().len(), 3);
        tree.close_focused().unwrap();
        assert_eq!(tree.leaves().len(), 2);
        tree.close_focused().unwrap();
        assert_eq!(tree.leaves().len(), 1);
        assert_eq!(tree.close_focused(), Err(ClosePaneError::LastPane));
    }

    #[test]
    fn focus_or_open_splits_to_the_right() {
        let mut tree = PaneTree::new(AppKind::Terminal);
        assert!(tree.focus_or_open(AppKind::Files));
        assert_eq!(tree.leaves().len(), 2);
        assert_eq!(tree.focused_app(), AppKind::Files);
        assert!(!tree.focus_or_open(AppKind::Files));
        assert_eq!(tree.leaves().len(), 2);
    }
}
