//! Transfer queue UI state and local path picker.

use std::fs;
use std::path::{Path, PathBuf};

use ssh_core::{TransferId, TransferJob};

#[derive(Debug, Clone, Default)]
pub struct TransfersUi {
    pub jobs: Vec<TransferJob>,
    pub selected: usize,
}

impl TransfersUi {
    pub fn upsert(&mut self, job: TransferJob) {
        if let Some(existing) = self.jobs.iter_mut().find(|j| j.id == job.id) {
            *existing = job;
        } else {
            self.jobs.push(job);
        }
        if self.selected >= self.jobs.len() && !self.jobs.is_empty() {
            self.selected = self.jobs.len() - 1;
        }
    }

    pub fn selected_id(&self) -> Option<TransferId> {
        self.jobs.get(self.selected).map(|j| j.id)
    }

    pub fn move_up(&mut self) {
        if self.selected > 0 {
            self.selected -= 1;
        }
    }

    pub fn move_down(&mut self) {
        if !self.jobs.is_empty() && self.selected + 1 < self.jobs.len() {
            self.selected += 1;
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PathPromptKind {
    /// Pick a local file to upload into remote cwd.
    Upload,
    /// Pick a local destination path for a remote file download.
    Download,
    /// Pick local file(s) to place on the file clipboard (for paste → upload).
    CopyLocal,
}

#[derive(Debug, Clone)]
pub struct PathPrompt {
    pub kind: PathPromptKind,
    pub title: String,
    pub buffer: String,
    pub remote: PathBuf,
    pub remote_size: Option<u64>,
    pub browse_cwd: PathBuf,
    pub browse_entries: Vec<LocalEntry>,
    pub browse_selected: usize,
    /// true = editing path buffer; false = browsing local list
    pub editing: bool,
    pub error: Option<String>,
}

#[derive(Debug, Clone)]
pub struct LocalEntry {
    pub name: String,
    pub path: PathBuf,
    pub is_dir: bool,
}

impl PathPrompt {
    pub fn upload(remote_dir: PathBuf) -> Self {
        let start = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
        let mut prompt = Self {
            kind: PathPromptKind::Upload,
            title: format!("Upload into {}", remote_dir.display()),
            buffer: String::new(),
            remote: remote_dir,
            remote_size: None,
            browse_cwd: start.clone(),
            browse_entries: Vec::new(),
            browse_selected: 0,
            editing: false,
            error: None,
        };
        prompt.refresh_listing();
        prompt
    }

    pub fn download(remote_file: PathBuf, remote_size: Option<u64>) -> Self {
        let name = remote_file
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "download.bin".into());
        let dir = dirs::download_dir()
            .or_else(dirs::home_dir)
            .unwrap_or_else(|| PathBuf::from("."));
        let dest = dir.join(&name);
        let mut prompt = Self {
            kind: PathPromptKind::Download,
            title: format!("Download {}", remote_file.display()),
            buffer: dest.display().to_string(),
            remote: remote_file,
            remote_size,
            browse_cwd: dir,
            browse_entries: Vec::new(),
            browse_selected: 0,
            editing: true,
            error: None,
        };
        prompt.refresh_listing();
        prompt
    }

    pub fn copy_local() -> Self {
        let start = dirs::home_dir().unwrap_or_else(|| PathBuf::from("."));
        let mut prompt = Self {
            kind: PathPromptKind::CopyLocal,
            title: "Copy local file to clipboard".into(),
            buffer: String::new(),
            remote: PathBuf::new(),
            remote_size: None,
            browse_cwd: start,
            browse_entries: Vec::new(),
            browse_selected: 0,
            editing: false,
            error: None,
        };
        prompt.refresh_listing();
        prompt
    }

    pub fn refresh_listing(&mut self) {
        self.browse_entries.clear();
        if self.browse_cwd != Path::new("/") {
            if let Some(parent) = self.browse_cwd.parent() {
                self.browse_entries.push(LocalEntry {
                    name: "..".into(),
                    path: parent.to_path_buf(),
                    is_dir: true,
                });
            }
        }
        let Ok(rd) = fs::read_dir(&self.browse_cwd) else {
            self.error = Some(format!("cannot read {}", self.browse_cwd.display()));
            return;
        };
        let mut entries: Vec<LocalEntry> = rd
            .filter_map(|e| e.ok())
            .map(|e| {
                let path = e.path();
                let is_dir = path.is_dir();
                let name = e.file_name().to_string_lossy().into_owned();
                LocalEntry { name, path, is_dir }
            })
            .collect();
        entries.sort_by(|a, b| match (a.is_dir, b.is_dir) {
            (true, false) => std::cmp::Ordering::Less,
            (false, true) => std::cmp::Ordering::Greater,
            _ => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
        });
        self.browse_entries.extend(entries);
        self.browse_selected = 0;
        self.error = None;
    }

    pub fn move_up(&mut self) {
        if self.browse_selected > 0 {
            self.browse_selected -= 1;
        }
    }

    pub fn move_down(&mut self) {
        if self.browse_selected + 1 < self.browse_entries.len() {
            self.browse_selected += 1;
        }
    }

    pub fn activate_selected(&mut self) -> Option<PathBuf> {
        let entry = self.browse_entries.get(self.browse_selected)?.clone();
        if entry.is_dir {
            self.browse_cwd = entry.path;
            self.refresh_listing();
            None
        } else {
            self.buffer = entry.path.display().to_string();
            Some(entry.path)
        }
    }

    /// Select the focused entry for submit (file or directory). Skips `..`.
    pub fn select_selected(&mut self) -> Option<PathBuf> {
        let entry = self.browse_entries.get(self.browse_selected)?.clone();
        if entry.name == ".." {
            return None;
        }
        self.buffer = entry.path.display().to_string();
        Some(entry.path)
    }

    /// Destination for download into the current browse directory.
    pub fn download_into_cwd(&self) -> PathBuf {
        let name = self
            .remote
            .file_name()
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "download.bin".into());
        self.browse_cwd.join(name)
    }

    pub fn resolved_path(&self) -> PathBuf {
        let raw = self.buffer.trim();
        if raw.is_empty() {
            return self.browse_cwd.clone();
        }
        let p = PathBuf::from(raw);
        if p.is_absolute() {
            p
        } else {
            self.browse_cwd.join(p)
        }
    }
}
