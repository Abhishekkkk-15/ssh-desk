//! Encrypted host credentials and connection profiles.

mod store;

pub use store::{zeroize_string, AuthMethod, HostProfile, Vault, VaultError, VaultPath};
