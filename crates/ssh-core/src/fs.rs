//! Remote filesystem listing types (SFTP-backed).

use std::path::{Path, PathBuf};
use std::time::{Duration, UNIX_EPOCH};

/// A single directory entry from the remote (or demo) filesystem.
#[derive(Debug, Clone)]
pub struct RemoteEntry {
    pub name: String,
    pub path: PathBuf,
    pub is_dir: bool,
    pub is_symlink: bool,
    pub size: Option<u64>,
    /// Unix mode bits when provided by the server (includes type nibble).
    pub permissions: Option<u32>,
    /// Modification time (seconds since epoch).
    pub mtime: Option<u32>,
}

impl RemoteEntry {
    pub fn display_name(&self) -> String {
        if self.is_symlink {
            format!("{}@", self.name)
        } else if self.is_dir {
            format!("{}/", self.name)
        } else {
            self.name.clone()
        }
    }

    /// `drwxr-xr-x` / `lrwxrwxrwx` / `-rw-r--r--` style mode string.
    pub fn mode_string(&self) -> String {
        let kind = if self.is_symlink {
            'l'
        } else if self.is_dir {
            'd'
        } else {
            '-'
        };
        let perms = match self.permissions {
            Some(mode) => format_rwx(mode),
            None => "---------".into(),
        };
        format!("{kind}{perms}")
    }

    pub fn size_label(&self) -> String {
        if self.is_dir && !self.is_symlink {
            return "—".into();
        }
        match self.size {
            Some(n) => crate::transfer::format_bytes(n),
            None => "—".into(),
        }
    }

    pub fn mtime_label(&self) -> String {
        let Some(secs) = self.mtime else {
            return "—".into();
        };
        let Some(dt) = UNIX_EPOCH.checked_add(Duration::from_secs(secs as u64)) else {
            return "—".into();
        };
        // Local formatting without chrono dep: YYYY-MM-DD HH:MM via simple UTC math.
        format_unix_mtime(
            dt.duration_since(UNIX_EPOCH)
                .map(|d| d.as_secs())
                .unwrap_or(0),
        )
    }
}

fn format_rwx(mode: u32) -> String {
    let bits = mode & 0o777;
    let mut s = String::with_capacity(9);
    for shift in [6, 3, 0] {
        let n = (bits >> shift) & 0o7;
        s.push(if n & 0o4 != 0 { 'r' } else { '-' });
        s.push(if n & 0o2 != 0 { 'w' } else { '-' });
        s.push(if n & 0o1 != 0 { 'x' } else { '-' });
    }
    s
}

fn format_unix_mtime(secs: u64) -> String {
    // Civil date from Unix days (Howard Hinnant algorithm, UTC).
    let z = (secs / 86_400) as i64 + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    let tod = secs % 86_400;
    let hh = tod / 3600;
    let mm = (tod % 3600) / 60;
    format!("{y:04}-{m:02}-{d:02} {hh:02}:{mm:02}")
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
            out.push_str(&format!(
                "{offset:08x}  {:<47}  |{}|\n",
                hex.join(" "),
                ascii
            ));
        }
        out
    }
}

/// Join remote POSIX paths.
pub fn join_remote(cwd: &Path, name: &str) -> PathBuf {
    if name == ".." {
        return cwd
            .parent()
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from("/"));
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
    if s.is_empty() { "/".into() } else { s }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mode_and_mtime_labels() {
        let e = RemoteEntry {
            name: "a".into(),
            path: PathBuf::from("/a"),
            is_dir: false,
            is_symlink: false,
            size: Some(2048),
            permissions: Some(0o100644),
            mtime: Some(0),
        };
        assert_eq!(e.mode_string(), "-rw-r--r--");
        assert_eq!(e.size_label(), "2 KB");
        assert!(e.mtime_label().starts_with("1970-01-01"));
    }
}
