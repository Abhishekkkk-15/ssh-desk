//! Persist open host tabs + desktop layouts across app restarts.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use ssh_wm::Layout;
use tracing::{info, warn};

const SESSION_VERSION: u32 = 2;

/// One open host tab snapshot.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PersistedHostSession {
    pub host_id: String,
    pub layout: Layout,
    pub focused_index: usize,
    #[serde(default)]
    pub files_cwd: Option<String>,
}

/// Full desktop session written on quit when at least one host is open.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PersistedSession {
    #[serde(default = "default_version")]
    pub version: u32,
    pub active_host_id: String,
    pub sessions: Vec<PersistedHostSession>,
}

fn default_version() -> u32 {
    SESSION_VERSION
}

/// Legacy single-host format (pre-v2).
#[derive(Debug, Deserialize)]
struct PersistedSessionV1 {
    host_id: String,
    layout: Layout,
    focused_index: usize,
    #[serde(default)]
    files_cwd: Option<String>,
}

impl PersistedSession {
    fn from_v1(v1: PersistedSessionV1) -> Self {
        Self {
            version: SESSION_VERSION,
            active_host_id: v1.host_id.clone(),
            sessions: vec![PersistedHostSession {
                host_id: v1.host_id,
                layout: v1.layout,
                focused_index: v1.focused_index,
                files_cwd: v1.files_cwd,
            }],
        }
    }

    pub fn find_host(&self, host_id: &str) -> Option<&PersistedHostSession> {
        self.sessions.iter().find(|s| s.host_id == host_id)
    }
}

pub fn config_dir() -> Result<PathBuf> {
    let base = dirs::config_dir().context("no config directory")?;
    Ok(base.join("ssh-desk"))
}

pub fn state_dir() -> Result<PathBuf> {
    if let Some(state) = dirs::state_dir() {
        return Ok(state.join("ssh-desk"));
    }
    // Fallback when XDG_STATE_HOME is unavailable (some platforms).
    config_dir()
}

pub fn session_path() -> Result<PathBuf> {
    Ok(config_dir()?.join("session.json"))
}

pub fn log_path() -> Result<PathBuf> {
    Ok(state_dir()?.join("ssh-desk.log"))
}

pub fn load() -> Option<PersistedSession> {
    let path = session_path().ok()?;
    if !path.exists() {
        return None;
    }
    let raw = match fs::read_to_string(&path) {
        Ok(r) => r,
        Err(e) => {
            warn!(error = %e, "failed to read session.json");
            return None;
        }
    };
    match parse_session_json(&raw) {
        Ok(s) if !s.sessions.is_empty() => Some(s),
        Ok(_) => {
            warn!("session.json has no hosts · clearing");
            let _ = clear();
            None
        }
        Err(e) => {
            warn!(error = %e, path = %path.display(), "corrupt session.json · ignoring");
            let _ = clear();
            None
        }
    }
}

fn parse_session_json(raw: &str) -> Result<PersistedSession> {
    if let Ok(v2) = serde_json::from_str::<PersistedSession>(raw) {
        if !v2.sessions.is_empty() {
            return Ok(v2);
        }
    }
    let v1: PersistedSessionV1 =
        serde_json::from_str(raw).context("not a valid v1 or v2 session")?;
    Ok(PersistedSession::from_v1(v1))
}

pub fn save(session: &PersistedSession) -> Result<()> {
    if session.sessions.is_empty() {
        return clear();
    }
    let path = session_path()?;
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)?;
    }
    let raw = serde_json::to_string_pretty(session)?;
    atomic_write(&path, raw.as_bytes())?;
    info!(
        hosts = session.sessions.len(),
        active = %session.active_host_id,
        path = %path.display(),
        "session saved"
    );
    Ok(())
}

pub fn clear() -> Result<()> {
    let path = session_path()?;
    if path.exists() {
        fs::remove_file(&path)?;
        info!(path = %path.display(), "session cleared");
    }
    Ok(())
}

fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
    let parent = path.parent().unwrap_or_else(|| Path::new("."));
    let tmp = tempfile_path(parent)?;
    {
        let mut f = fs::File::create(&tmp).with_context(|| format!("create {}", tmp.display()))?;
        f.write_all(bytes)?;
        f.sync_all()?;
    }
    fs::rename(&tmp, path).with_context(|| {
        let _ = fs::remove_file(&tmp);
        format!("rename {} → {}", tmp.display(), path.display())
    })?;
    Ok(())
}

fn tempfile_path(dir: &Path) -> Result<PathBuf> {
    let name = format!(
        ".session.{}.tmp",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0)
    );
    Ok(dir.join(name))
}

#[cfg(test)]
mod tests {
    use super::*;
    use ssh_wm::AppKind;

    #[test]
    fn migrates_v1_to_v2() {
        let raw = serde_json::json!({
            "host_id": "abc",
            "layout": { "Leaf": "Terminal" },
            "focused_index": 0,
            "files_cwd": "/home/x"
        })
        .to_string();
        let s = parse_session_json(&raw).expect("migrate");
        assert_eq!(s.version, SESSION_VERSION);
        assert_eq!(s.active_host_id, "abc");
        assert_eq!(s.sessions.len(), 1);
        assert_eq!(s.sessions[0].host_id, "abc");
        assert_eq!(s.sessions[0].files_cwd.as_deref(), Some("/home/x"));
        assert!(matches!(
            s.sessions[0].layout,
            Layout::Leaf(AppKind::Terminal)
        ));
    }

    #[test]
    fn parses_v2() {
        let raw = serde_json::json!({
            "version": 2,
            "active_host_id": "b",
            "sessions": [
                {
                    "host_id": "a",
                    "layout": { "Leaf": "Files" },
                    "focused_index": 0
                },
                {
                    "host_id": "b",
                    "layout": { "Leaf": "Terminal" },
                    "focused_index": 0,
                    "files_cwd": "/"
                }
            ]
        })
        .to_string();
        let s = parse_session_json(&raw).unwrap();
        assert_eq!(s.sessions.len(), 2);
        assert_eq!(s.active_host_id, "b");
    }
}
