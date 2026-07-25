//! Name prompts and delete confirm for the Files pane.

use std::path::PathBuf;

#[derive(Debug, Clone)]
pub enum FilesPrompt {
    Mkdir {
        buffer: String,
        error: Option<String>,
    },
    Rename {
        from: PathBuf,
        buffer: String,
        error: Option<String>,
    },
    Delete {
        /// Display names.
        names: Vec<String>,
        /// Paths to remove (file or dir).
        paths: Vec<PathBuf>,
        /// 0 = Yes, 1 = No
        selected: usize,
    },
}

impl FilesPrompt {
    pub fn mkdir() -> Self {
        Self::Mkdir {
            buffer: String::new(),
            error: None,
        }
    }

    pub fn rename(from: PathBuf, current_name: String) -> Self {
        Self::Rename {
            from,
            buffer: current_name,
            error: None,
        }
    }

    pub fn delete(names: Vec<String>, paths: Vec<PathBuf>) -> Self {
        Self::Delete {
            names,
            paths,
            selected: 1, // default to No for safety
        }
    }

    pub fn title(&self) -> &'static str {
        match self {
            Self::Mkdir { .. } => "New folder",
            Self::Rename { .. } => "Rename",
            Self::Delete { .. } => "Delete",
        }
    }

    pub fn buffer_mut(&mut self) -> Option<&mut String> {
        match self {
            Self::Mkdir { buffer, .. } | Self::Rename { buffer, .. } => Some(buffer),
            Self::Delete { .. } => None,
        }
    }

    pub fn set_error(&mut self, err: impl Into<String>) {
        match self {
            Self::Mkdir { error, .. } | Self::Rename { error, .. } => *error = Some(err.into()),
            Self::Delete { .. } => {}
        }
    }
}
