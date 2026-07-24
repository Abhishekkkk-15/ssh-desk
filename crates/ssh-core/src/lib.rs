//! SSH session hub: connect, PTY channels, and future SFTP/exec.

mod client;
mod error;
mod pty;

pub use client::{SessionEvent, SessionHub};
pub use error::CoreError;
pub use pty::{PtyId, PtyOutput, PtySession};
