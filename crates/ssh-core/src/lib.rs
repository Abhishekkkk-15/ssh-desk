//! SSH session hub: connect, PTY channels, SFTP, and multi-session/jump support.

mod client;
mod error;
mod fs;
mod known_hosts;
mod pty;
mod transfer;

pub use client::{SessionEvent, SessionHub};
pub use error::CoreError;
pub use fs::{join_remote, remote_path_string, RemoteEntry, RemoteFileContent};
pub use known_hosts::default_known_hosts_path;
pub use pty::{PtyId, PtyOutput, PtySession};
pub use transfer::{
    format_bytes, format_rate, TransferDirection, TransferId, TransferJob, TransferStatus,
};
