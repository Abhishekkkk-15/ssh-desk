//! Interactive PTY channel state.

use uuid::Uuid;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PtyId(pub Uuid);

impl PtyId {
    pub fn new() -> Self {
        Self(Uuid::new_v4())
    }
}

impl Default for PtyId {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, Clone)]
pub struct PtyOutput {
    pub id: PtyId,
    pub data: Vec<u8>,
}

/// Local mirror of a remote PTY for the Terminal app.
#[derive(Debug, Clone)]
pub struct PtySession {
    pub id: PtyId,
    pub cols: u16,
    pub rows: u16,
    pub scrollback: Vec<u8>,
    pub connected: bool,
    pub title: String,
}

impl PtySession {
    pub fn new(title: impl Into<String>) -> Self {
        Self {
            id: PtyId::new(),
            cols: 80,
            rows: 24,
            scrollback: Vec::new(),
            connected: false,
            title: title.into(),
        }
    }

    pub fn push_output(&mut self, data: &[u8]) {
        self.scrollback.extend_from_slice(data);
        // Cap scrollback ~256 KiB for Phase 1.
        const MAX: usize = 256 * 1024;
        if self.scrollback.len() > MAX {
            let drop = self.scrollback.len() - MAX;
            self.scrollback.drain(..drop);
        }
    }

    pub fn clear(&mut self) {
        self.scrollback.clear();
    }
}
