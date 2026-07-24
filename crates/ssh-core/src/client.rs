//! Async SSH client wrapper around russh.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use russh::client::{self, Handle};
use russh::keys::load_secret_key;
use russh::keys::PrivateKeyWithHashAlg;
use russh::ChannelMsg;
use russh_sftp::client::SftpSession;
use ssh_vault::{AuthMethod, HostProfile, Vault};
use tokio::sync::{mpsc, Mutex};
use tracing::{info, warn};

use crate::error::CoreError;
use crate::fs::{remote_path_string, RemoteEntry, RemoteFileContent};
use crate::pty::{PtyId, PtyOutput, PtySession};

/// Events pushed from the SSH task to the TUI.
#[derive(Debug, Clone)]
pub enum SessionEvent {
    Connected { host_id: String },
    Disconnected { host_id: String, reason: String },
    PtyData(PtyOutput),
    Status(String),
    Error(String),
}

enum PtyCommand {
    Write(Vec<u8>),
    Resize { cols: u16, rows: u16 },
}

struct ClientHandler;

impl client::Handler for ClientHandler {
    type Error = russh::Error;

    async fn check_server_key(
        &mut self,
        _server_public_key: &russh::keys::PublicKey,
    ) -> Result<bool, Self::Error> {
        // Phase 1: accept; known_hosts verification lands later.
        Ok(true)
    }
}

/// Live handle for one connected host.
struct SessionHandle {
    host_id: String,
    handle: Handle<ClientHandler>,
    pty: PtySession,
    pty_tx: mpsc::UnboundedSender<PtyCommand>,
    sftp: Option<SftpSession>,
}

/// Manages SSH sessions for the desktop.
pub struct SessionHub {
    sessions: Mutex<Vec<SessionHandle>>,
    events_tx: mpsc::UnboundedSender<SessionEvent>,
}

impl SessionHub {
    pub fn new(events_tx: mpsc::UnboundedSender<SessionEvent>) -> Arc<Self> {
        Arc::new(Self {
            sessions: Mutex::new(Vec::new()),
            events_tx,
        })
    }

    pub fn events_sender(&self) -> mpsc::UnboundedSender<SessionEvent> {
        self.events_tx.clone()
    }

    pub async fn connect(
        self: &Arc<Self>,
        profile: HostProfile,
        vault: &Vault,
        password_passphrase: Option<&str>,
    ) -> Result<PtyId, CoreError> {
        let host_id = profile.id.clone();
        let addr = profile.address();
        let _ = self.events_tx.send(SessionEvent::Status(format!(
            "connecting to {}@{}…",
            profile.user, addr
        )));

        let config = Arc::new(client::Config::default());
        let mut handle = client::connect(config, addr.as_str(), ClientHandler)
            .await
            .map_err(|e| CoreError::Ssh(e.to_string()))?;

        authenticate(&mut handle, &profile, vault, password_passphrase).await?;

        let mut channel = handle
            .channel_open_session()
            .await
            .map_err(|e| CoreError::Ssh(e.to_string()))?;

        let cols = 80u32;
        let rows = 24u32;
        channel
            .request_pty(false, "xterm-256color", cols, rows, 0, 0, &[])
            .await
            .map_err(|e| CoreError::Pty(e.to_string()))?;
        channel
            .request_shell(true)
            .await
            .map_err(|e| CoreError::Pty(e.to_string()))?;

        let mut pty = PtySession::new(profile.name.clone());
        pty.connected = true;
        pty.cols = cols as u16;
        pty.rows = rows as u16;
        let pty_id = pty.id;

        let (pty_tx, mut pty_rx) = mpsc::unbounded_channel::<PtyCommand>();
        let events_tx = self.events_tx.clone();
        let pty_id_reader = pty_id;
        let disconnect_host = host_id.clone();

        tokio::spawn(async move {
            loop {
                tokio::select! {
                    msg = channel.wait() => {
                        match msg {
                            Some(ChannelMsg::Data { ref data }) => {
                                let _ = events_tx.send(SessionEvent::PtyData(PtyOutput {
                                    id: pty_id_reader,
                                    data: data.to_vec(),
                                }));
                            }
                            Some(ChannelMsg::ExtendedData { ref data, .. }) => {
                                let _ = events_tx.send(SessionEvent::PtyData(PtyOutput {
                                    id: pty_id_reader,
                                    data: data.to_vec(),
                                }));
                            }
                            Some(ChannelMsg::Eof) | Some(ChannelMsg::Close) | None => break,
                            _ => {}
                        }
                    }
                    cmd = pty_rx.recv() => {
                        match cmd {
                            Some(PtyCommand::Write(bytes)) => {
                                if channel.data(&bytes[..]).await.is_err() {
                                    break;
                                }
                            }
                            Some(PtyCommand::Resize { cols, rows }) => {
                                let _ = channel.window_change(cols as u32, rows as u32, 0, 0).await;
                            }
                            None => break,
                        }
                    }
                }
            }
            let _ = events_tx.send(SessionEvent::Disconnected {
                host_id: disconnect_host,
                reason: "channel closed".into(),
            });
        });

        let sftp = open_sftp(&handle).await?;

        let session = SessionHandle {
            host_id: profile.id.clone(),
            handle,
            pty,
            pty_tx,
            sftp: Some(sftp),
        };

        self.sessions.lock().await.push(session);
        let _ = self.events_tx.send(SessionEvent::Connected {
            host_id: profile.id,
        });
        info!(host = %addr, "ssh session connected");
        Ok(pty_id)
    }

    pub async fn disconnect(&self, host_id: &str) -> Result<(), CoreError> {
        let mut sessions = self.sessions.lock().await;
        if let Some(pos) = sessions.iter().position(|s| s.host_id == host_id) {
            let session = sessions.remove(pos);
            if let Some(sftp) = &session.sftp {
                let _ = sftp.close().await;
            }
            let _ = session
                .handle
                .disconnect(russh::Disconnect::ByApplication, "", "")
                .await;
            let _ = self.events_tx.send(SessionEvent::Disconnected {
                host_id: host_id.into(),
                reason: "user disconnect".into(),
            });
        }
        Ok(())
    }

    pub async fn write_pty(&self, host_id: &str, data: &[u8]) -> Result<(), CoreError> {
        let sessions = self.sessions.lock().await;
        let session = sessions
            .iter()
            .find(|s| s.host_id == host_id)
            .ok_or(CoreError::Closed)?;
        session
            .pty_tx
            .send(PtyCommand::Write(data.to_vec()))
            .map_err(|_| CoreError::Closed)?;
        Ok(())
    }

    pub async fn resize_pty(&self, host_id: &str, cols: u16, rows: u16) -> Result<(), CoreError> {
        let mut sessions = self.sessions.lock().await;
        let session = sessions
            .iter_mut()
            .find(|s| s.host_id == host_id)
            .ok_or(CoreError::Closed)?;
        session.pty.cols = cols;
        session.pty.rows = rows;
        session
            .pty_tx
            .send(PtyCommand::Resize { cols, rows })
            .map_err(|_| CoreError::Closed)?;
        Ok(())
    }

    pub async fn is_connected(&self, host_id: &str) -> bool {
        self.sessions
            .lock()
            .await
            .iter()
            .any(|s| s.host_id == host_id)
    }

    pub async fn has_sftp(&self, host_id: &str) -> bool {
        self.sessions
            .lock()
            .await
            .iter()
            .any(|s| s.host_id == host_id && s.sftp.is_some())
    }

    /// Resolve `.` / relative paths to an absolute remote path.
    pub async fn canonicalize(&self, host_id: &str, path: &str) -> Result<PathBuf, CoreError> {
        let sessions = self.sessions.lock().await;
        let session = sessions
            .iter()
            .find(|s| s.host_id == host_id)
            .ok_or(CoreError::Closed)?;
        let sftp = session
            .sftp
            .as_ref()
            .ok_or_else(|| CoreError::Sftp("sftp not available".into()))?;
        let abs = sftp
            .canonicalize(path)
            .await
            .map_err(|e| CoreError::Sftp(e.to_string()))?;
        Ok(PathBuf::from(abs))
    }

    pub async fn list_dir(&self, host_id: &str, path: &Path) -> Result<Vec<RemoteEntry>, CoreError> {
        let sessions = self.sessions.lock().await;
        let session = sessions
            .iter()
            .find(|s| s.host_id == host_id)
            .ok_or(CoreError::Closed)?;
        let sftp = session
            .sftp
            .as_ref()
            .ok_or_else(|| CoreError::Sftp("sftp not available".into()))?;

        let path_str = remote_path_string(path);
        let dir = sftp
            .read_dir(&path_str)
            .await
            .map_err(|e| CoreError::Sftp(e.to_string()))?;

        let mut entries: Vec<RemoteEntry> = dir
            .map(|entry| {
                let is_dir = entry.file_type().is_dir();
                let size = entry.metadata().size;
                RemoteEntry {
                    name: entry.file_name(),
                    path: PathBuf::from(entry.path()),
                    is_dir,
                    size,
                }
            })
            .collect();

        entries.sort_by(|a, b| match (a.is_dir, b.is_dir) {
            (true, false) => std::cmp::Ordering::Less,
            (false, true) => std::cmp::Ordering::Greater,
            _ => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
        });

        Ok(entries)
    }

    pub async fn read_file(
        &self,
        host_id: &str,
        path: &Path,
    ) -> Result<RemoteFileContent, CoreError> {
        let sessions = self.sessions.lock().await;
        let session = sessions
            .iter()
            .find(|s| s.host_id == host_id)
            .ok_or(CoreError::Closed)?;
        let sftp = session
            .sftp
            .as_ref()
            .ok_or_else(|| CoreError::Sftp("sftp not available".into()))?;

        let path_str = remote_path_string(path);
        let mut bytes = sftp
            .read(&path_str)
            .await
            .map_err(|e| CoreError::Sftp(e.to_string()))?;

        let truncated = bytes.len() > RemoteFileContent::MAX_BYTES;
        if truncated {
            bytes.truncate(RemoteFileContent::MAX_BYTES);
        }

        Ok(RemoteFileContent {
            path: path.to_path_buf(),
            bytes,
            truncated,
        })
    }
}

async fn open_sftp(handle: &Handle<ClientHandler>) -> Result<SftpSession, CoreError> {
    let channel = handle
        .channel_open_session()
        .await
        .map_err(|e| CoreError::Sftp(e.to_string()))?;
    channel
        .request_subsystem(true, "sftp")
        .await
        .map_err(|e| CoreError::Sftp(e.to_string()))?;
    SftpSession::new(channel.into_stream())
        .await
        .map_err(|e| CoreError::Sftp(e.to_string()))
}

async fn authenticate(
    handle: &mut Handle<ClientHandler>,
    profile: &HostProfile,
    vault: &Vault,
    password_passphrase: Option<&str>,
) -> Result<(), CoreError> {
    match &profile.auth {
        AuthMethod::Agent => {
            let mut agent = russh::keys::agent::client::AgentClient::connect_env()
                .await
                .map_err(|e| CoreError::Auth(format!("ssh-agent: {e}")))?;
            let identities = agent
                .request_identities()
                .await
                .map_err(|e| CoreError::Auth(format!("agent identities: {e}")))?;
            if identities.is_empty() {
                return Err(CoreError::Auth("ssh-agent has no identities".into()));
            }
            let hash_alg = handle
                .best_supported_rsa_hash()
                .await
                .map_err(|e| CoreError::Auth(e.to_string()))?
                .flatten();
            let mut ok = false;
            for key in identities {
                match handle
                    .authenticate_publickey_with(&profile.user, key, hash_alg, &mut agent)
                    .await
                {
                    Ok(result) if result.success() => {
                        ok = true;
                        break;
                    }
                    Ok(_) => continue,
                    Err(e) => warn!("agent key failed: {e}"),
                }
            }
            if !ok {
                return Err(CoreError::Auth("all agent keys rejected".into()));
            }
        }
        AuthMethod::PrivateKey { path, .. } => {
            auth_private_key(handle, &profile.user, path).await?;
        }
        AuthMethod::Password { secret_file } => {
            let passphrase = password_passphrase.ok_or_else(|| {
                CoreError::Auth("vault passphrase required for password auth".into())
            })?;
            let mut password = vault.load_password(secret_file, passphrase)?;
            let result = handle
                .authenticate_password(&profile.user, password.as_str())
                .await
                .map_err(|e| CoreError::Auth(e.to_string()))?;
            ssh_vault::zeroize_string(&mut password);
            if !result.success() {
                return Err(CoreError::Auth(format!("password auth failed: {result:?}")));
            }
        }
    }
    Ok(())
}

async fn auth_private_key(
    handle: &mut Handle<ClientHandler>,
    user: &str,
    path: &PathBuf,
) -> Result<(), CoreError> {
    let key = load_secret_key(path, None).map_err(|e| CoreError::Auth(e.to_string()))?;
    let hash_alg = handle
        .best_supported_rsa_hash()
        .await
        .map_err(|e| CoreError::Auth(e.to_string()))?
        .flatten();
    let result = handle
        .authenticate_publickey(
            user,
            PrivateKeyWithHashAlg::new(Arc::new(key), hash_alg),
        )
        .await
        .map_err(|e| CoreError::Auth(e.to_string()))?;
    if result.success() {
        Ok(())
    } else {
        Err(CoreError::Auth(format!("key auth failed: {result:?}")))
    }
}
