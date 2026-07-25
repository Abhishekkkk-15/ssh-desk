//! Encrypted host credentials and connection profiles.

mod store;

pub use store::{AuthMethod, HostProfile, Vault, VaultError, VaultPath, zeroize_string};
