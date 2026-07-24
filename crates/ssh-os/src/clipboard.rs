//! Text + file clipboard for the desktop shell.

use std::path::PathBuf;

use arboard::Clipboard as SystemClipboard;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;

#[derive(Debug, Error)]
pub enum ClipboardError {
    #[error("system clipboard unavailable: {0}")]
    System(String),
}

/// Where a file entry lives.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum FileLocation {
    Local { path: PathBuf },
    Remote { host_id: String, path: PathBuf },
}

/// A file (or directory) on the clipboard.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileEntry {
    pub id: Uuid,
    pub location: FileLocation,
    pub is_dir: bool,
}

impl FileEntry {
    pub fn local(path: PathBuf, is_dir: bool) -> Self {
        Self {
            id: Uuid::new_v4(),
            location: FileLocation::Local { path },
            is_dir,
        }
    }

    pub fn remote(host_id: impl Into<String>, path: PathBuf, is_dir: bool) -> Self {
        Self {
            id: Uuid::new_v4(),
            location: FileLocation::Remote {
                host_id: host_id.into(),
                path,
            },
            is_dir,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileOp {
    Copy,
    Cut,
}

/// Dual clipboard: system text + in-app file entries.
#[derive(Debug, Default)]
pub struct Clipboard {
    text: Option<String>,
    files: Vec<FileEntry>,
    file_op: Option<FileOp>,
}

impl Clipboard {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn set_text(&mut self, text: impl Into<String>) {
        self.text = Some(text.into());
    }

    pub fn text(&self) -> Option<&str> {
        self.text.as_deref()
    }

    pub fn copy_text_to_system(&self) -> Result<(), ClipboardError> {
        let Some(text) = &self.text else {
            return Ok(());
        };
        let mut sys = SystemClipboard::new().map_err(|e| ClipboardError::System(e.to_string()))?;
        sys.set_text(text.clone())
            .map_err(|e| ClipboardError::System(e.to_string()))?;
        Ok(())
    }

    pub fn paste_text_from_system(&mut self) -> Result<Option<String>, ClipboardError> {
        let mut sys = SystemClipboard::new().map_err(|e| ClipboardError::System(e.to_string()))?;
        match sys.get_text() {
            Ok(t) => {
                self.text = Some(t.clone());
                Ok(Some(t))
            }
            Err(_) => Ok(None),
        }
    }

    pub fn set_files(&mut self, files: Vec<FileEntry>, op: FileOp) {
        self.files = files;
        self.file_op = Some(op);
    }

    pub fn files(&self) -> &[FileEntry] {
        &self.files
    }

    pub fn file_op(&self) -> Option<FileOp> {
        self.file_op
    }

    pub fn clear_files(&mut self) {
        self.files.clear();
        self.file_op = None;
    }

    pub fn has_files(&self) -> bool {
        !self.files.is_empty()
    }
}
