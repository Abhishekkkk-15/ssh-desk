//! OpenSSH-style known_hosts verification (TOFU accept-new).

use std::path::{Path, PathBuf};

use russh::keys::known_hosts::{check_known_hosts_path, learn_known_hosts_path};
use russh::keys::{Error as KeysError, PublicKey};
use tracing::{info, warn};

use crate::error::CoreError;

/// Default path: `~/.config/ssh-desk/known_hosts`.
pub fn default_known_hosts_path() -> Result<PathBuf, CoreError> {
    let base = dirs::config_dir()
        .ok_or_else(|| CoreError::Message("no config directory for known_hosts".into()))?;
    Ok(base.join("ssh-desk").join("known_hosts"))
}

/// Verify `pubkey` for `host:port`.
///
/// Policy (OpenSSH-like `accept-new`):
/// - known + match → accept
/// - unknown → learn into `path` and accept (TOFU)
/// - known + changed → reject (possible MITM)
pub fn verify_server_key(
    host: &str,
    port: u16,
    pubkey: &PublicKey,
    path: &Path,
) -> Result<bool, CoreError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(CoreError::Io)?;
    }

    match check_known_hosts_path(host, port, pubkey, path) {
        Ok(true) => {
            info!(%host, port, "known_hosts: key matched");
            Ok(true)
        }
        Ok(false) => {
            learn_known_hosts_path(host, port, pubkey, path)
                .map_err(|e| CoreError::Auth(format!("failed to write known_hosts: {e}")))?;
            info!(
                %host,
                port,
                path = %path.display(),
                "known_hosts: learned new host key (TOFU)"
            );
            Ok(true)
        }
        Err(KeysError::KeyChanged { line }) => {
            warn!(%host, port, line, "known_hosts: HOST KEY CHANGED — rejecting");
            Err(CoreError::Auth(format!(
                "REMOTE HOST IDENTIFICATION HAS CHANGED for {host}:{port} (known_hosts line {line}).\n\
                 Someone could be eavesdropping, or the host key was rotated.\n\
                 Remove the old entry from {} and reconnect if you trust the new key.",
                path.display()
            )))
        }
        Err(e) => Err(CoreError::Auth(format!("known_hosts check failed: {e}"))),
    }
}
