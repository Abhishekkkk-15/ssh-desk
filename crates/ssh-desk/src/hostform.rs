//! Interactive “add host” form for the launcher.

use std::path::PathBuf;

use ssh_vault::{AuthMethod, HostProfile, Vault, VaultError};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthChoice {
    Agent,
    PrivateKey,
    Password,
}

impl AuthChoice {
    pub fn label(self) -> &'static str {
        match self {
            Self::Agent => "ssh-agent",
            Self::PrivateKey => "private key",
            Self::Password => "password",
        }
    }

    pub fn cycle(self) -> Self {
        match self {
            Self::Agent => Self::PrivateKey,
            Self::PrivateKey => Self::Password,
            Self::Password => Self::Agent,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostField {
    Name,
    Host,
    Port,
    User,
    Auth,
    KeyPath,
    Password,
    VaultPass,
}

impl HostField {
    pub fn all_for(auth: AuthChoice) -> &'static [HostField] {
        match auth {
            AuthChoice::Agent => &[
                HostField::Name,
                HostField::Host,
                HostField::Port,
                HostField::User,
                HostField::Auth,
            ],
            AuthChoice::PrivateKey => &[
                HostField::Name,
                HostField::Host,
                HostField::Port,
                HostField::User,
                HostField::Auth,
                HostField::KeyPath,
            ],
            AuthChoice::Password => &[
                HostField::Name,
                HostField::Host,
                HostField::Port,
                HostField::User,
                HostField::Auth,
                HostField::Password,
                HostField::VaultPass,
            ],
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Name => "Name",
            Self::Host => "Host",
            Self::Port => "Port",
            Self::User => "User",
            Self::Auth => "Auth",
            Self::KeyPath => "Key path",
            Self::Password => "Password",
            Self::VaultPass => "Vault pass",
        }
    }
}

#[derive(Debug, Clone)]
pub struct HostForm {
    pub name: String,
    pub host: String,
    pub port: String,
    pub user: String,
    pub auth: AuthChoice,
    pub key_path: String,
    pub password: String,
    pub vault_pass: String,
    pub focus: HostField,
    pub error: Option<String>,
}

impl HostForm {
    pub fn new() -> Self {
        let user = std::env::var("USER")
            .or_else(|_| std::env::var("USERNAME"))
            .unwrap_or_else(|_| "user".into());
        let key_path = dirs::home_dir()
            .map(|h| h.join(".ssh").join("id_ed25519").display().to_string())
            .unwrap_or_else(|| "~/.ssh/id_ed25519".into());
        Self {
            name: String::new(),
            host: String::new(),
            port: "22".into(),
            user,
            auth: AuthChoice::Agent,
            key_path,
            password: String::new(),
            vault_pass: String::new(),
            focus: HostField::Name,
            error: None,
        }
    }

    pub fn active_fields(&self) -> &'static [HostField] {
        HostField::all_for(self.auth)
    }

    pub fn focus_next(&mut self) {
        let fields = self.active_fields();
        let i = fields.iter().position(|f| *f == self.focus).unwrap_or(0);
        self.focus = fields[(i + 1) % fields.len()];
    }

    pub fn focus_prev(&mut self) {
        let fields = self.active_fields();
        let i = fields.iter().position(|f| *f == self.focus).unwrap_or(0);
        self.focus = fields[(i + fields.len() - 1) % fields.len()];
    }

    pub fn buffer_mut(&mut self) -> Option<&mut String> {
        match self.focus {
            HostField::Name => Some(&mut self.name),
            HostField::Host => Some(&mut self.host),
            HostField::Port => Some(&mut self.port),
            HostField::User => Some(&mut self.user),
            HostField::KeyPath => Some(&mut self.key_path),
            HostField::Password => Some(&mut self.password),
            HostField::VaultPass => Some(&mut self.vault_pass),
            HostField::Auth => None,
        }
    }

    pub fn insert_char(&mut self, c: char) {
        if self.focus == HostField::Auth {
            return;
        }
        if self.focus == HostField::Port && !c.is_ascii_digit() {
            return;
        }
        if let Some(buf) = self.buffer_mut() {
            buf.push(c);
        }
        self.error = None;
    }

    pub fn backspace(&mut self) {
        if let Some(buf) = self.buffer_mut() {
            buf.pop();
        }
        self.error = None;
    }

    pub fn cycle_auth(&mut self) {
        self.auth = self.auth.cycle();
        let fields = self.active_fields();
        if !fields.contains(&self.focus) {
            self.focus = HostField::Auth;
        }
        self.error = None;
    }

    /// Validate and persist into the vault. Returns the new profile on success.
    pub fn save(&self, vault: &mut Vault) -> Result<HostProfile, String> {
        let name = self.name.trim();
        let host = self.host.trim();
        let user = self.user.trim();
        if name.is_empty() {
            return Err("name is required".into());
        }
        if host.is_empty() {
            return Err("host is required".into());
        }
        if user.is_empty() {
            return Err("user is required".into());
        }
        let port: u16 = self
            .port
            .trim()
            .parse()
            .map_err(|_| "port must be a number (1–65535)".to_string())?;
        if port == 0 {
            return Err("port must be 1–65535".into());
        }

        let mut profile = HostProfile::new(name, host, user);
        profile.port = port;

        match self.auth {
            AuthChoice::Agent => {
                profile.auth = AuthMethod::Agent;
            }
            AuthChoice::PrivateKey => {
                let path = expand_tilde(self.key_path.trim());
                if path.as_os_str().is_empty() {
                    return Err("key path is required".into());
                }
                if !path.is_file() {
                    return Err(format!("key file not found: {}", path.display()));
                }
                profile.auth = AuthMethod::PrivateKey {
                    path,
                    passphrase_protected: false,
                };
            }
            AuthChoice::Password => {
                if self.password.is_empty() {
                    return Err("password is required".into());
                }
                if self.vault_pass.is_empty() {
                    return Err("vault passphrase is required to encrypt the password".into());
                }
                let secret_file = vault
                    .store_password(&profile.id, &self.password, &self.vault_pass)
                    .map_err(vault_err)?;
                profile.auth = AuthMethod::Password { secret_file };
            }
        }

        vault.upsert(profile.clone()).map_err(vault_err)?;
        Ok(profile)
    }
}

/// Prompt for the age vault passphrase when connecting with password auth.
#[derive(Debug, Clone)]
pub struct VaultUnlockPrompt {
    pub host_name: String,
    pub buffer: String,
    pub error: Option<String>,
    pub connecting: bool,
}

impl VaultUnlockPrompt {
    pub fn new(host_name: impl Into<String>) -> Self {
        Self {
            host_name: host_name.into(),
            buffer: String::new(),
            error: None,
            connecting: false,
        }
    }
}

fn expand_tilde(s: &str) -> PathBuf {
    if let Some(rest) = s.strip_prefix("~/") {
        if let Some(home) = dirs::home_dir() {
            return home.join(rest);
        }
    }
    if s == "~" {
        if let Some(home) = dirs::home_dir() {
            return home;
        }
    }
    PathBuf::from(s)
}

fn vault_err(e: VaultError) -> String {
    e.to_string()
}
