//! Remote filesystem listing types (SFTP-backed).

use std::path::{Path, PathBuf};

/// A single directory entry from the remote (or demo) filesystem.
#[derive(Debug, Clone)]
pub struct RemoteEntry {
    pub name: String,
    pub path: PathBuf,
    pub is_dir: bool,
    pub size: Option<u64>,
}

impl RemoteEntry {
    pub fn display_name(&self) -> String {
        if self.is_dir {
            format!("{}/", self.name)
        } else {
            self.name.clone()
        }
    }
}

/// Result of reading a remote file for the viewer.
#[derive(Debug, Clone)]
pub struct RemoteFileContent {
    pub path: PathBuf,
    pub bytes: Vec<u8>,
    pub truncated: bool,
}

impl RemoteFileContent {
    pub const MAX_BYTES: usize = 512 * 1024;

    pub fn as_text(&self) -> Option<&str> {
        std::str::from_utf8(&self.bytes).ok()
    }

    pub fn looks_binary(&self) -> bool {
        self.as_text().is_none() || self.bytes.iter().take(8000).any(|&b| b == 0)
    }

    pub fn hex_preview(&self, max_bytes: usize) -> String {
        let slice = &self.bytes[..self.bytes.len().min(max_bytes)];
        let mut out = String::new();
        for (i, chunk) in slice.chunks(16).enumerate() {
            let offset = i * 16;
            let hex: Vec<String> = chunk.iter().map(|b| format!("{b:02x}")).collect();
            let ascii: String = chunk
                .iter()
                .map(|&b| {
                    if (32..127).contains(&b) {
                        b as char
                    } else {
                        '.'
                    }
                })
                .collect();
            out.push_str(&format!("{offset:08x}  {:<47}  |{}|\n", hex.join(" "), ascii));
        }
        out
    }
}

/// Join remote POSIX paths.
pub fn join_remote(cwd: &Path, name: &str) -> PathBuf {
    if name == ".." {
        return cwd.parent().map(Path::to_path_buf).unwrap_or_else(|| PathBuf::from("/"));
    }
    if name.starts_with('/') {
        return PathBuf::from(name);
    }
    if cwd.as_os_str().is_empty() || cwd == Path::new("/") {
        PathBuf::from(format!("/{name}"))
    } else {
        cwd.join(name)
    }
}

/// Normalize path string for SFTP (forward slashes).
pub fn remote_path_string(path: &Path) -> String {
    let s = path.to_string_lossy().replace('\\', "/");
    if s.is_empty() {
        "/".into()
    } else {
        s
    }
}
