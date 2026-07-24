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

    pub fn set_app(&mut self, id: NodeId, app: AppKind) {
        if let Some(PaneNode::Leaf { app: slot, .. }) = self.root.find_mut(id) {
            *slot = app;
        }
    }

    pub fn set_focused_app(&mut self, app: AppKind) {
        self.set_app(self.focused, app);
    }

    /// Focus an existing Viewer pane, or turn a spare pane into one / split.
    pub fn focus_or_open_viewer(&mut self) {
        if let Some((id, _)) = self
            .leaves()
            .into_iter()
            .find(|(_, app)| *app == AppKind::Viewer)
        {
            self.focused = id;
            return;
        }
        if let Some((id, _)) = self
            .leaves()
            .into_iter()
            .find(|(_, app)| *app == AppKind::Processes)
        {
            self.set_app(id, AppKind::Viewer);
            self.focused = id;
            return;
        }
        let _ = self.split_focused(Direction::Vertical, 0.55, AppKind::Viewer);
    }
}
