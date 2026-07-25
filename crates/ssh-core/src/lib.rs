//! SSH session hub: connect, PTY channels, SFTP, and multi-session/jump support.

mod client;
mod error;
mod fs;
mod known_hosts;
mod pty;
mod transfer;

pub use client::{SessionEvent, SessionHub};
pub use error::CoreError;
pub use fs::{RemoteEntry, RemoteFileContent, join_remote, remote_path_string};
pub use known_hosts::default_known_hosts_path;
pub use pty::{PtyId, PtyOutput, PtySession};
pub use transfer::{
    TransferDirection, TransferId, TransferJob, TransferStatus, format_bytes, format_rate,
};
