use thiserror::Error;

#[derive(Debug, Error)]
pub enum CoreError {
    #[error("ssh error: {0}")]
    Ssh(String),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("auth failed: {0}")]
    Auth(String),
    #[error("session closed")]
    Closed,
    #[error("pty error: {0}")]
    Pty(String),
    #[error("vault: {0}")]
    Vault(#[from] ssh_vault::VaultError),
    #[error("{0}")]
    Message(String),
}
