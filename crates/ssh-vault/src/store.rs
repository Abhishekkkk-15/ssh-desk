//! On-disk vault for SSH host profiles.
//!
//! Host metadata is stored as TOML. Optional passwords are encrypted with `age`
//! using a passphrase-derived identity. Prefer keys / agent in the UI.

use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use age::secrecy::SecretString;
use age::{Decryptor, Encryptor};
use serde::{Deserialize, Serialize};
use thiserror::Error;
use uuid::Uuid;
use zeroize::Zeroize;

#[derive(Debug, Error)]
pub enum VaultError {
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("config parse error: {0}")]
    Parse(#[from] toml::de::Error),
    #[error("config serialize error: {0}")]
    Serialize(#[from] toml::ser::Error),
    #[error("encryption error: {0}")]
    Crypto(String),
    #[error("host not found: {0}")]
    NotFound(String),
}

/// How to authenticate to a host.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AuthMethod {
    Agent,
    PrivateKey {
        path: PathBuf,
        #[serde(default)]
        passphrase_protected: bool,
    },
    /// Password is never stored in plaintext in the main file.
    Password {
        /// Relative path under vault dir to the age-encrypted secret.
        secret_file: String,
    },
}

/// A saved SSH host.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HostProfile {
    pub id: String,
    pub name: String,
    pub host: String,
    #[serde(default = "default_port")]
    pub port: u16,
    pub user: String,
    pub auth: AuthMethod,
    #[serde(default)]
    pub jump_via: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
}

fn default_port() -> u16 {
    22
}

impl HostProfile {
    pub fn new(name: impl Into<String>, host: impl Into<String>, user: impl Into<String>) -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            name: name.into(),
            host: host.into(),
            port: 22,
            user: user.into(),
            auth: AuthMethod::Agent,
            jump_via: None,
            tags: Vec::new(),
        }
    }

    pub fn address(&self) -> String {
        format!("{}:{}", self.host, self.port)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
struct VaultFile {
    hosts: Vec<HostProfile>,
}

/// Resolved vault directory (`~/.config/ssh-desk` by default).
#[derive(Debug, Clone)]
pub struct VaultPath(pub PathBuf);

impl VaultPath {
    pub fn default_dir() -> Result<Self, VaultError> {
        let base = dirs::config_dir().ok_or_else(|| {
            VaultError::Io(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                "no config directory",
            ))
        })?;
        Ok(Self(base.join("ssh-desk")))
    }

    pub fn hosts_file(&self) -> PathBuf {
        self.0.join("hosts.toml")
    }

    pub fn secrets_dir(&self) -> PathBuf {
        self.0.join("secrets")
    }
}

/// Host vault: load/save profiles and encrypted password secrets.
#[derive(Debug, Clone)]
pub struct Vault {
    path: VaultPath,
    data: VaultFile,
}

impl Vault {
    pub fn open_default() -> Result<Self, VaultError> {
        Self::open(VaultPath::default_dir()?)
    }

    pub fn open(path: VaultPath) -> Result<Self, VaultError> {
        fs::create_dir_all(&path.0)?;
        fs::create_dir_all(path.secrets_dir())?;
        let hosts_file = path.hosts_file();
        let data = if hosts_file.exists() {
            let raw = fs::read_to_string(&hosts_file)?;
            toml::from_str(&raw)?
        } else {
            VaultFile::default()
        };
        Ok(Self { path, data })
    }

    pub fn path(&self) -> &Path {
        &self.path.0
    }

    pub fn hosts(&self) -> &[HostProfile] {
        &self.data.hosts
    }

    pub fn get(&self, id: &str) -> Option<&HostProfile> {
        self.data.hosts.iter().find(|h| h.id == id || h.name == id)
    }

    pub fn upsert(&mut self, profile: HostProfile) -> Result<(), VaultError> {
        if let Some(existing) = self.data.hosts.iter_mut().find(|h| h.id == profile.id) {
            *existing = profile;
        } else {
            self.data.hosts.push(profile);
        }
        self.save()
    }

    pub fn remove(&mut self, id: &str) -> Result<(), VaultError> {
        let before = self.data.hosts.len();
        self.data.hosts.retain(|h| h.id != id && h.name != id);
        if self.data.hosts.len() == before {
            return Err(VaultError::NotFound(id.into()));
        }
        self.save()
    }

    pub fn save(&self) -> Result<(), VaultError> {
        let raw = toml::to_string_pretty(&self.data)?;
        fs::write(self.path.hosts_file(), raw)?;
        Ok(())
    }

    /// Encrypt a password into the secrets dir; returns relative secret file name.
    pub fn store_password(
        &self,
        host_id: &str,
        password: &str,
        passphrase: &str,
    ) -> Result<String, VaultError> {
        let file_name = format!("{host_id}.age");
        let dest = self.path.secrets_dir().join(&file_name);

        let encryptor = Encryptor::with_user_passphrase(SecretString::from(passphrase.to_owned()));
        let mut encrypted = Vec::new();
        {
            let mut writer = encryptor
                .wrap_output(&mut encrypted)
                .map_err(|e| VaultError::Crypto(e.to_string()))?;
            writer
                .write_all(password.as_bytes())
                .map_err(|e| VaultError::Crypto(e.to_string()))?;
            writer
                .finish()
                .map_err(|e| VaultError::Crypto(e.to_string()))?;
        }
        fs::write(dest, encrypted)?;
        Ok(file_name)
    }

    /// Decrypt a stored password (caller should zeroize when done).
    pub fn load_password(&self, secret_file: &str, passphrase: &str) -> Result<String, VaultError> {
        let path = self.path.secrets_dir().join(secret_file);
        let bytes = fs::read(path)?;
        let decryptor =
            Decryptor::new(&bytes[..]).map_err(|e| VaultError::Crypto(e.to_string()))?;
        let mut reader = decryptor
            .decrypt(std::iter::once(
                &age::scrypt::Identity::new(SecretString::from(passphrase.to_owned()))
                    as &dyn age::Identity,
            ))
            .map_err(|e| VaultError::Crypto(e.to_string()))?;
        let mut out = String::new();
        reader
            .read_to_string(&mut out)
            .map_err(|e| VaultError::Crypto(e.to_string()))?;
        Ok(out)
    }

    /// Seed demo hosts for first-run launcher (no secrets).
    pub fn ensure_examples(&mut self) -> Result<(), VaultError> {
        if !self.data.hosts.is_empty() {
            return Ok(());
        }
        let mut local = HostProfile::new("localhost", "127.0.0.1", whoami_user());
        local.tags = vec!["local".into(), "demo".into()];
        self.upsert(local)?;
        Ok(())
    }
}

fn whoami_user() -> String {
    std::env::var("USER")
        .or_else(|_| std::env::var("USERNAME"))
        .unwrap_or_else(|_| "user".into())
}

/// Helper so callers can wipe passwords after use.
pub fn zeroize_string(s: &mut String) {
    s.zeroize();
}
