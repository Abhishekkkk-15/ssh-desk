//! Async SSH client wrapper around russh.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use russh::client::{self, Handle};
use russh::keys::load_secret_key;
use russh::keys::PrivateKeyWithHashAlg;
use russh::ChannelMsg;
use russh_sftp::client::SftpSession;
use russh_sftp::protocol::OpenFlags;
use ssh_vault::{AuthMethod, HostProfile, Vault};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::{mpsc, Mutex};
use tracing::{info, warn};

use crate::error::CoreError;
use crate::fs::{remote_path_string, RemoteEntry, RemoteFileContent};
use crate::pty::{PtyId, PtyOutput, PtySession};
use crate::transfer::{TransferDirection, TransferId, TransferJob, TransferStatus};

/// Events pushed from the SSH task to the TUI.
#[derive(Debug, Clone)]
pub enum SessionEvent {
    Connected { host_id: String },
    Disconnected { host_id: String, reason: String },
    PtyData(PtyOutput),
    TransferUpdate(TransferJob),
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
    sftp: Option<Arc<SftpSession>>,
}

/// Manages SSH sessions for the desktop.
pub struct SessionHub {
    sessions: Mutex<Vec<SessionHandle>>,
    transfers: Mutex<Vec<TransferJob>>,
    events_tx: mpsc::UnboundedSender<SessionEvent>,
}

impl SessionHub {
    pub fn new(events_tx: mpsc::UnboundedSender<SessionEvent>) -> Arc<Self> {
        Arc::new(Self {
            sessions: Mutex::new(Vec::new()),
            transfers: Mutex::new(Vec::new()),
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

        let sftp = Arc::new(open_sftp(&handle).await?);

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

    pub async fn transfers_snapshot(&self) -> Vec<TransferJob> {
        self.transfers.lock().await.clone()
    }

    pub async fn cancel_transfer(&self, id: TransferId) -> bool {
        let mut jobs = self.transfers.lock().await;
        if let Some(job) = jobs.iter_mut().find(|j| j.id == id) {
            if job.status.can_cancel() {
                job.request_cancel();
                if job.status == TransferStatus::Queued {
                    job.status = TransferStatus::Cancelled;
                }
                let snap = job.clone();
                let _ = self.events_tx.send(SessionEvent::TransferUpdate(snap));
                return true;
            }
        }
        false
    }

    /// Queue a local → remote upload into `remote_dir` (file keeps its name).
    pub async fn enqueue_upload(
        self: &Arc<Self>,
        host_id: &str,
        local_path: PathBuf,
        remote_dir: PathBuf,
    ) -> Result<TransferId, CoreError> {
        self.enqueue_upload_ex(host_id, local_path, remote_dir, false)
            .await
    }

    pub async fn enqueue_upload_ex(
        self: &Arc<Self>,
        host_id: &str,
        local_path: PathBuf,
        remote_dir: PathBuf,
        cut: bool,
    ) -> Result<TransferId, CoreError> {
        if !local_path.is_file() {
            return Err(CoreError::Message(format!(
                "not a local file: {}",
                local_path.display()
            )));
        }
        let meta = tokio::fs::metadata(&local_path)
            .await
            .map_err(CoreError::Io)?;
        let name = local_path
            .file_name()
            .ok_or_else(|| CoreError::Message("invalid local path".into()))?;
        let remote_path = remote_dir.join(name);
        let sftp = self.sftp_arc(host_id).await?;

        let mut job = TransferJob::new(
            host_id,
            TransferDirection::Upload,
            local_path.clone(),
            remote_path.clone(),
            Some(meta.len()),
        );
        if cut {
            job = job.with_delete_local();
        }
        let id = job.id;
        let cancel = Arc::clone(&job.cancel);
        self.transfers.lock().await.push(job.clone());
        let _ = self
            .events_tx
            .send(SessionEvent::TransferUpdate(job.clone()));

        let hub = Arc::clone(self);
        tokio::spawn(async move {
            let result = run_upload(sftp, local_path, remote_path, cancel, &hub, id).await;
            hub.finish_transfer(id, result).await;
        });
        Ok(id)
    }

    /// Queue a remote → local download.
    pub async fn enqueue_download(
        self: &Arc<Self>,
        host_id: &str,
        remote_path: PathBuf,
        local_path: PathBuf,
        remote_size: Option<u64>,
    ) -> Result<TransferId, CoreError> {
        self.enqueue_download_ex(host_id, remote_path, local_path, remote_size, false)
            .await
    }

    pub async fn enqueue_download_ex(
        self: &Arc<Self>,
        host_id: &str,
        remote_path: PathBuf,
        local_path: PathBuf,
        remote_size: Option<u64>,
        cut: bool,
    ) -> Result<TransferId, CoreError> {
        let sftp = self.sftp_arc(host_id).await?;
        let bytes_total = if remote_size.is_some() {
            remote_size
        } else {
            sftp.metadata(remote_path_string(&remote_path))
                .await
                .ok()
                .and_then(|m| m.size)
        };

        let mut job = TransferJob::new(
            host_id,
            TransferDirection::Download,
            local_path.clone(),
            remote_path.clone(),
            bytes_total,
        );
        if cut {
            job = job.with_delete_remote();
        }
        let id = job.id;
        let cancel = Arc::clone(&job.cancel);
        self.transfers.lock().await.push(job.clone());
        let _ = self
            .events_tx
            .send(SessionEvent::TransferUpdate(job.clone()));

        let hub = Arc::clone(self);
        tokio::spawn(async move {
            let result = run_download(sftp, remote_path, local_path, cancel, &hub, id).await;
            hub.finish_transfer(id, result).await;
        });
        Ok(id)
    }

    /// Same-host remote file copy (source in `from`, destination file path in `to`).
    pub async fn enqueue_remote_copy(
        self: &Arc<Self>,
        host_id: &str,
        from: PathBuf,
        to: PathBuf,
    ) -> Result<TransferId, CoreError> {
        let sftp = self.sftp_arc(host_id).await?;
        let bytes_total = sftp
            .metadata(remote_path_string(&from))
            .await
            .ok()
            .and_then(|m| m.size);

        // local_path field stores the remote source for RemoteCopy jobs.
        let job = TransferJob::new(
            host_id,
            TransferDirection::RemoteCopy,
            from.clone(),
            to.clone(),
            bytes_total,
        );
        let id = job.id;
        let cancel = Arc::clone(&job.cancel);
        self.transfers.lock().await.push(job.clone());
        let _ = self
            .events_tx
            .send(SessionEvent::TransferUpdate(job.clone()));

        let hub = Arc::clone(self);
        tokio::spawn(async move {
            let result = run_remote_copy(sftp, from, to, cancel, &hub, id).await;
            hub.finish_transfer(id, result).await;
        });
        Ok(id)
    }

    pub async fn remote_rename(
        &self,
        host_id: &str,
        from: &Path,
        to: &Path,
    ) -> Result<(), CoreError> {
        let sftp = self.sftp_arc(host_id).await?;
        sftp.rename(remote_path_string(from), remote_path_string(to))
            .await
            .map_err(|e| CoreError::Sftp(e.to_string()))
    }

    pub async fn remote_remove_file(&self, host_id: &str, path: &Path) -> Result<(), CoreError> {
        let sftp = self.sftp_arc(host_id).await?;
        sftp.remove_file(remote_path_string(path))
            .await
            .map_err(|e| CoreError::Sftp(e.to_string()))
    }

    pub async fn retry_transfer(self: &Arc<Self>, id: TransferId) -> Result<TransferId, CoreError> {
        let (direction, host_id, local, remote, size, del_local, del_remote) = {
            let jobs = self.transfers.lock().await;
            let job = jobs
                .iter()
                .find(|j| j.id == id)
                .ok_or_else(|| CoreError::Message("transfer not found".into()))?;
            if !job.status.can_retry() {
                return Err(CoreError::Message("transfer cannot be retried".into()));
            }
            (
                job.direction,
                job.host_id.clone(),
                job.local_path.clone(),
                job.remote_path.clone(),
                job.bytes_total,
                job.delete_local_after,
                job.delete_remote_after,
            )
        };
        match direction {
            TransferDirection::Upload => {
                let parent = remote
                    .parent()
                    .map(Path::to_path_buf)
                    .unwrap_or_else(|| PathBuf::from("/"));
                self.enqueue_upload_ex(&host_id, local, parent, del_local)
                    .await
            }
            TransferDirection::Download => {
                self.enqueue_download_ex(&host_id, remote, local, size, del_remote)
                    .await
            }
            TransferDirection::RemoteCopy => {
                self.enqueue_remote_copy(&host_id, local, remote).await
            }
        }
    }

    async fn sftp_arc(&self, host_id: &str) -> Result<Arc<SftpSession>, CoreError> {
        let sessions = self.sessions.lock().await;
        let session = sessions
            .iter()
            .find(|s| s.host_id == host_id)
            .ok_or(CoreError::Closed)?;
        session
            .sftp
            .clone()
            .ok_or_else(|| CoreError::Sftp("sftp not available".into()))
    }

    async fn bump_progress(&self, id: TransferId, bytes_done: u64) {
        let mut jobs = self.transfers.lock().await;
        if let Some(job) = jobs.iter_mut().find(|j| j.id == id) {
            if job.is_cancelled() {
                return;
            }
            job.note_progress(bytes_done);
            let _ = self
                .events_tx
                .send(SessionEvent::TransferUpdate(job.clone()));
        }
    }

    async fn finish_transfer(&self, id: TransferId, result: Result<(), CoreError>) {
        let cleanup = {
            let mut jobs = self.transfers.lock().await;
            let Some(job) = jobs.iter_mut().find(|j| j.id == id) else {
                return;
            };
            if job.is_cancelled() {
                job.status = TransferStatus::Cancelled;
                job.bytes_per_sec = 0.0;
                let _ = self
                    .events_tx
                    .send(SessionEvent::TransferUpdate(job.clone()));
                return;
            }
            match result {
                Ok(()) => {
                    job.status = TransferStatus::Done;
                    if let Some(total) = job.bytes_total {
                        job.bytes_done = total;
                    }
                    job.bytes_per_sec = 0.0;
                    job.error = None;
                    Some((
                        job.host_id.clone(),
                        job.local_path.clone(),
                        job.remote_path.clone(),
                        job.delete_local_after,
                        job.delete_remote_after,
                        job.clone(),
                    ))
                }
                Err(e) => {
                    job.status = TransferStatus::Failed;
                    job.error = Some(e.to_string());
                    job.bytes_per_sec = 0.0;
                    let _ = self
                        .events_tx
                        .send(SessionEvent::TransferUpdate(job.clone()));
                    None
                }
            }
        };

        if let Some((host_id, local, remote, del_local, del_remote, snap)) = cleanup {
            let _ = self.events_tx.send(SessionEvent::TransferUpdate(snap));
            if del_local {
                if let Err(e) = tokio::fs::remove_file(&local).await {
                    let _ = self.events_tx.send(SessionEvent::Status(format!(
                        "cut: could not remove local {}: {e}",
                        local.display()
                    )));
                }
            }
            if del_remote {
                if let Err(e) = self.remote_remove_file(&host_id, &remote).await {
                    let _ = self.events_tx.send(SessionEvent::Status(format!(
                        "cut: could not remove remote {}: {e}",
                        remote.display()
                    )));
                }
            }
        }
    }
}

const CHUNK: usize = 64 * 1024;

async fn run_upload(
    sftp: Arc<SftpSession>,
    local_path: PathBuf,
    remote_path: PathBuf,
    cancel: Arc<std::sync::atomic::AtomicBool>,
    hub: &SessionHub,
    id: TransferId,
) -> Result<(), CoreError> {
    use std::sync::atomic::Ordering;

    let mut local = tokio::fs::File::open(&local_path)
        .await
        .map_err(CoreError::Io)?;
    let remote_str = remote_path_string(&remote_path);
    let mut remote = sftp
        .open_with_flags(
            &remote_str,
            OpenFlags::CREATE | OpenFlags::TRUNCATE | OpenFlags::WRITE,
        )
        .await
        .map_err(|e| CoreError::Sftp(e.to_string()))?;

    let mut buf = vec![0u8; CHUNK];
    let mut done = 0u64;
    loop {
        if cancel.load(Ordering::SeqCst) {
            return Err(CoreError::Message("cancelled".into()));
        }
        let n = local.read(&mut buf).await.map_err(CoreError::Io)?;
        if n == 0 {
            break;
        }
        remote
            .write_all(&buf[..n])
            .await
            .map_err(|e| CoreError::Sftp(e.to_string()))?;
        done += n as u64;
        hub.bump_progress(id, done).await;
    }
    let _ = remote.shutdown().await;
    Ok(())
}

async fn run_remote_copy(
    sftp: Arc<SftpSession>,
    from: PathBuf,
    to: PathBuf,
    cancel: Arc<std::sync::atomic::AtomicBool>,
    hub: &SessionHub,
    id: TransferId,
) -> Result<(), CoreError> {
    use std::sync::atomic::Ordering;

    let from_str = remote_path_string(&from);
    let to_str = remote_path_string(&to);
    let mut src = sftp
        .open(&from_str)
        .await
        .map_err(|e| CoreError::Sftp(e.to_string()))?;
    let mut dst = sftp
        .open_with_flags(
            &to_str,
            OpenFlags::CREATE | OpenFlags::TRUNCATE | OpenFlags::WRITE,
        )
        .await
        .map_err(|e| CoreError::Sftp(e.to_string()))?;

    let mut buf = vec![0u8; CHUNK];
    let mut done = 0u64;
    loop {
        if cancel.load(Ordering::SeqCst) {
            return Err(CoreError::Message("cancelled".into()));
        }
        let n = src
            .read(&mut buf)
            .await
            .map_err(|e| CoreError::Sftp(e.to_string()))?;
        if n == 0 {
            break;
        }
        dst.write_all(&buf[..n])
            .await
            .map_err(|e| CoreError::Sftp(e.to_string()))?;
        done += n as u64;
        hub.bump_progress(id, done).await;
    }
    let _ = dst.shutdown().await;
    Ok(())
}

async fn run_download(
    sftp: Arc<SftpSession>,
    remote_path: PathBuf,
    local_path: PathBuf,
    cancel: Arc<std::sync::atomic::AtomicBool>,
    hub: &SessionHub,
    id: TransferId,
) -> Result<(), CoreError> {
    use std::sync::atomic::Ordering;

    if let Some(parent) = local_path.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .map_err(CoreError::Io)?;
    }

    let remote_str = remote_path_string(&remote_path);
    let mut remote = sftp
        .open(&remote_str)
        .await
        .map_err(|e| CoreError::Sftp(e.to_string()))?;
    let mut local = tokio::fs::File::create(&local_path)
        .await
        .map_err(CoreError::Io)?;

    let mut buf = vec![0u8; CHUNK];
    let mut done = 0u64;
    loop {
        if cancel.load(Ordering::SeqCst) {
            return Err(CoreError::Message("cancelled".into()));
        }
        let n = remote
            .read(&mut buf)
            .await
            .map_err(|e| CoreError::Sftp(e.to_string()))?;
        if n == 0 {
            break;
        }
        local.write_all(&buf[..n]).await.map_err(CoreError::Io)?;
        done += n as u64;
        hub.bump_progress(id, done).await;
    }
    local.flush().await.map_err(CoreError::Io)?;
    Ok(())
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
