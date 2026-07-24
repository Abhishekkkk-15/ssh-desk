//! Serializable layout snapshots (per-host).

use serde::{Deserialize, Serialize};

use crate::tree::{AppKind, Direction, NodeId, PaneNode, PaneTree, Split};

/// Persisted layout for a host desktop.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SavedLayout {
    pub host_id: String,
    pub root: Layout,
    pub focused_index: usize,
}

/// Serializable layout tree (apps only; ids regenerated on load).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Layout {
    Leaf(AppKind),
    Split {
        direction: Direction,
        ratio: f32,
        first: Box<Layout>,
        second: Box<Layout>,
    },
}

impl Layout {
    pub fn from_node(node: &PaneNode) -> Self {
        match node {
            PaneNode::Leaf { app, .. } => Self::Leaf(*app),
            PaneNode::Split(s) => Self::Split {
                direction: s.direction,
                ratio: s.ratio,
                first: Box::new(Self::from_node(&s.first)),
                second: Box::new(Self::from_node(&s.second)),
            },
        }
    }

    pub fn into_tree(self, focused_index: usize) -> PaneTree {
        let (root, leaves) = self.into_node();
        let focused = leaves
            .get(focused_index)
            .copied()
            .or_else(|| leaves.first().copied())
            .unwrap_or_else(NodeId::new);
        PaneTree {
            root,
            focused,
        }
    }

    fn into_node(self) -> (PaneNode, Vec<NodeId>) {
        match self {
            Self::Leaf(app) => {
                let id = NodeId::new();
                (PaneNode::Leaf { id, app }, vec![id])
            }
            Self::Split {
                direction,
                ratio,
                first,
                second,
            } => {
                let (first_node, mut leaves) = first.into_node();
                let (second_node, second_leaves) = second.into_node();
                leaves.extend(second_leaves);
                (
                    PaneNode::Split(Split {
                        direction,
                        ratio,
                        first: Box::new(first_node),
                        second: Box::new(second_node),
                    }),
                    leaves,
                )
            }
        }
    }
}

// Allow constructing PaneTree from layout without exposing private fields elsewhere.
impl PaneTree {
    pub fn from_layout(layout: Layout, focused_index: usize) -> Self {
        layout.into_tree(focused_index)
    }

    pub fn to_layout(&self) -> Layout {
        Layout::from_node(self.root_node())
    }

    pub fn focused_index(&self) -> usize {
        self.leaves()
            .iter()
            .position(|(id, _)| *id == self.focused())
            .unwrap_or(0)
    }
}
