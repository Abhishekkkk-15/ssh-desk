//! Parse OS / terminal file-drop payloads into local paths.
//!
//! Terminal emulators (WezTerm, iTerm2, Kitty, Windows Terminal, …) typically
//! deliver dropped files as bracketed paste containing:
//! - `file://` URIs (RFC 8089 / text/uri-list)
//! - absolute paths, one per line
//! - quoted paths with spaces

use std::path::{Path, PathBuf};

/// Result of inspecting pasted / dropped text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PasteKind {
    /// One or more filesystem paths (drop or path-list paste).
    Paths(Vec<PathBuf>),
    /// Ordinary text (not a path list).
    Text(String),
}

/// Classify pasted text: path list vs plain text.
pub fn classify_paste(raw: &str) -> PasteKind {
    let paths = parse_os_paths(raw);
    if !paths.is_empty() {
        PasteKind::Paths(paths)
    } else {
        PasteKind::Text(raw.to_string())
    }
}

/// Extract local filesystem paths from a drop/paste payload.
///
/// Existing files are preferred; non-existent paths are still returned when they
/// look like absolute paths (caller may filter).
pub fn parse_os_paths(raw: &str) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for token in tokenize_payload(raw) {
        if let Some(path) = token_to_path(&token) {
            if !out.iter().any(|p| p == &path) {
                out.push(path);
            }
        }
    }
    out
}

/// Keep only paths that currently exist as files (directories skipped for upload).
pub fn existing_files(paths: &[PathBuf]) -> Vec<PathBuf> {
    paths
        .iter()
        .filter(|p| p.is_file())
        .cloned()
        .collect()
}

fn tokenize_payload(raw: &str) -> Vec<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Vec::new();
    }

    // Null-separated (rare but used by some tools)
    if trimmed.contains('\0') {
        return trimmed
            .split('\0')
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_string)
            .collect();
    }

    // text/uri-list style or newline-separated paths
    if trimmed.contains('\n') || trimmed.contains('\r') {
        return trimmed
            .lines()
            .map(|l| l.trim().trim_end_matches('\r'))
            .filter(|l| !l.is_empty() && !l.starts_with('#'))
            .map(unquote)
            .collect();
    }

    // Single token, possibly quoted
    vec![unquote(trimmed)]
}

fn unquote(s: &str) -> String {
    let s = s.trim();
    if s.len() >= 2 {
        let bytes = s.as_bytes();
        if (bytes[0] == b'\'' && bytes[s.len() - 1] == b'\'')
            || (bytes[0] == b'"' && bytes[s.len() - 1] == b'"')
        {
            return s[1..s.len() - 1].to_string();
        }
    }
    s.to_string()
}

fn token_to_path(token: &str) -> Option<PathBuf> {
    let token = token.trim();
    if token.is_empty() {
        return None;
    }

    if let Some(path) = file_uri_to_path(token) {
        return Some(path);
    }

    // Absolute POSIX or Windows path
    let path = PathBuf::from(token);
    if path.is_absolute() {
        return Some(path);
    }

    // ~/foo
    if let Some(rest) = token.strip_prefix("~/") {
        if let Some(home) = home_dir() {
            return Some(home.join(rest));
        }
    }

    None
}

fn file_uri_to_path(uri: &str) -> Option<PathBuf> {
    let uri = uri.trim();
    let rest = uri.strip_prefix("file://")?;

    // file:///path  or  file://localhost/path  or  file://hostname/path
    let path_part = if let Some(stripped) = rest.strip_prefix("//") {
        // file:////server/share — unusual; treat as /server/share
        format!("/{stripped}")
    } else if rest.starts_with('/') {
        // file:///home/... → /home/...
        rest.to_string()
    } else if let Some(idx) = rest.find('/') {
        // file://localhost/home/... or file://host/path
        let host = &rest[..idx];
        let path = &rest[idx..];
        if host.is_empty() || host.eq_ignore_ascii_case("localhost") {
            path.to_string()
        } else {
            // UNC-style: keep host in path for Windows share approximation
            format!("/{host}{path}")
        }
    } else {
        return None;
    };

    let decoded = percent_decode(&path_part);

    // Windows: /C:/Users/... → C:/Users/...
    let decoded = if decoded.len() >= 3 {
        let bytes = decoded.as_bytes();
        if bytes[0] == b'/' && bytes[1].is_ascii_alphabetic() && bytes[2] == b':' {
            decoded[1..].to_string()
        } else {
            decoded
        }
    } else {
        decoded
    };

    Some(PathBuf::from(decoded))
}

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' && i + 2 < bytes.len() {
            if let (Some(h), Some(l)) = (from_hex(bytes[i + 1]), from_hex(bytes[i + 2])) {
                out.push((h << 4) | l);
                i += 3;
                continue;
            }
        }
        out.push(bytes[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn from_hex(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
}

/// Suggest a drop destination description for the UI.
pub fn describe_upload(paths: &[PathBuf], dest: &Path) -> String {
    let n = paths.len();
    let first = paths
        .first()
        .and_then(|p| p.file_name())
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "file".into());
    if n == 1 {
        format!("upload {first} → {}", dest.display())
    } else {
        format!("upload {n} files ({first}, …) → {}", dest.display())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_file_uri() {
        let paths = parse_os_paths("file:///tmp/hello%20world.txt");
        assert_eq!(paths, vec![PathBuf::from("/tmp/hello world.txt")]);
    }

    #[test]
    fn parses_uri_list() {
        let raw = "file:///tmp/a.txt\nfile:///tmp/b.txt\n# comment\n";
        let paths = parse_os_paths(raw);
        assert_eq!(
            paths,
            vec![PathBuf::from("/tmp/a.txt"), PathBuf::from("/tmp/b.txt")]
        );
    }

    #[test]
    fn parses_quoted_path() {
        let paths = parse_os_paths("'/tmp/my file.txt'");
        assert_eq!(paths, vec![PathBuf::from("/tmp/my file.txt")]);
    }

    #[test]
    fn parses_windows_file_uri() {
        let paths = parse_os_paths("file:///C:/Users/me/file.txt");
        assert_eq!(paths, vec![PathBuf::from("C:/Users/me/file.txt")]);
    }

    #[test]
    fn plain_text_is_not_paths() {
        assert_eq!(
            classify_paste("hello world"),
            PasteKind::Text("hello world".into())
        );
    }

    #[test]
    fn absolute_path_line() {
        let paths = parse_os_paths("/etc/hosts\n");
        assert_eq!(paths, vec![PathBuf::from("/etc/hosts")]);
    }
}
