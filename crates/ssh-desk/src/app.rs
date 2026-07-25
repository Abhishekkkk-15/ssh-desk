//! Application state machine: launcher ↔ desktop session.

use std::collections::VecDeque;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use crossterm::event::{
    self, DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture,
    Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseEvent, MouseEventKind,
};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::DefaultTerminal;
use ssh_core::{SessionEvent, SessionHub, join_remote};
use ssh_os::{
    Clipboard, DragPayload, DragSession, DropTarget, FileEntry, FileLocation, FileOp, OsDropOffer,
    PasteKind, classify_paste, existing_files,
};
use ssh_vault::{AuthMethod, HostProfile, Vault};
use ssh_wm::{AppKind, Desktop, Direction, PaneTree};
use tokio::sync::{mpsc, oneshot};

use crate::apps::{EditorState, ProcessesState};
use crate::diagnostics::{DiagLevel, DiagnosticsState};
use crate::files::{FilesRow, FilesState, ViewerState, resolve_open_path};
use crate::files_prompt::FilesPrompt;
use crate::hit::{self, FrameGeo};
use crate::hostform::{HostForm, VaultUnlockPrompt};
use crate::session::{self, PersistedHostSession, PersistedSession};
use crate::term::TermEmulator;
use crate::transfers::{PathPrompt, PathPromptKind, TransfersUi};
use crate::ui::{self, UiFrame};
use ssh_os::OpenAction;
use ssh_os::sniff_open_action;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Screen {
    Launcher,
    Desktop,
}

/// Per-session desktop state.
struct SessionSlot {
    host_id: String,
    host_name: String,
    desktop: Desktop,
    files: FilesState,
    viewer: ViewerState,
    editor: EditorState,
    processes: ProcessesState,
    transfers: TransfersUi,
    term: TermEmulator,
    pending_files_refresh: bool,
    fullscreen_app: Option<AppKind>,
    cached_passphrase: Option<String>,
}

impl SessionSlot {
    fn new(host_id: String, host_name: String, online: bool) -> Self {
        let mut term = TermEmulator::new(24, 80);
        if !online {
            term.write_str(&format!(
                "Desktop for '{host_name}' — not connected.\n\
                 Fix auth and reconnect (Esc → launcher → Enter).\n"
            ));
        }
        SessionSlot {
            host_id,
            host_name,
            desktop: Desktop::new(String::new(), String::new()), // filled below
            files: if online {
                FilesState::default()
            } else {
                FilesState::demo()
            },
            viewer: ViewerState::default(),
            editor: EditorState::default(),
            processes: if online {
                ProcessesState::default()
            } else {
                ProcessesState::demo()
            },
            transfers: TransfersUi::default(),
            term,
            pending_files_refresh: false,
            fullscreen_app: None,
            cached_passphrase: None,
        }
    }
}

pub struct App {
    screen: Screen,
    vault: Vault,
    hosts: Vec<HostProfile>,
    selected_host: usize,
    /// All open sessions (tabs).
    sessions: Vec<SessionSlot>,
    /// Index into `sessions` of the currently-viewed session.
    active_idx: usize,
    status: String,
    hub: Arc<SessionHub>,
    events_rx: mpsc::UnboundedReceiver<SessionEvent>,
    clipboard: Clipboard,
    path_prompt: Option<PathPrompt>,
    host_form: Option<HostForm>,
    vault_unlock: Option<VaultUnlockPrompt>,
    /// Last computed layout for mouse hit-testing.
    last_geo: Option<FrameGeo>,
    /// Mouse press awaiting drag threshold.
    mouse_press: Option<MousePress>,
    drag: Option<DragSession>,
    drop_target: Option<DropTarget>,
    pending_drop: Option<PendingDrop>,
    pending_open_parent: bool,
    os_drop: Option<OsDropOffer>,
    /// Show the session-switcher overlay.
    show_session_switcher: bool,
    full_screen: bool,
    files_search: Option<String>,
    should_quit: bool,
    connect_in_flight: bool,
    /// Host name shown while a background connect is running.
    connecting_host: Option<String>,
    connect_spinner: u8,
    pending_connect: Option<oneshot::Receiver<ConnectOutcome>>,
    overwrite_prompt: Option<OverwritePrompt>,
    files_prompt: Option<FilesPrompt>,
    diagnostics: DiagnosticsState,
    /// Multi-host restore in progress after startup.
    restore: Option<RestoreState>,
}

/// Sequential reconnect of tabs saved on last quit.
struct RestoreState {
    snapshot: PersistedSession,
    queue: VecDeque<String>,
    passphrase: Option<String>,
    total: usize,
    completed: usize,
}

#[derive(Debug)]
struct ConnectOutcome {
    profile: HostProfile,
    password_passphrase: Option<String>,
    result: Result<(), String>,
}

#[derive(Debug, Clone)]
pub struct OverwritePrompt {
    pub title: String,
    pub files: Vec<String>,
    pub selected: usize, // 0 = Yes, 1 = No
    // Action details to run if confirmed:
    pub dest_dir: PathBuf,
    pub entries: Vec<FileEntry>,
    pub op: FileOp,
}

#[derive(Debug, Clone)]
struct MousePress {
    origin: (u16, u16),
    /// Entry indices to drag (into files.entries).
    entry_indices: Vec<usize>,
}

impl App {
    fn new(
        vault: Vault,
        hub: Arc<SessionHub>,
        events_rx: mpsc::UnboundedReceiver<SessionEvent>,
    ) -> Self {
        let hosts = vault.hosts().to_vec();
        Self {
            screen: Screen::Launcher,
            vault,
            hosts,
            selected_host: 0,
            sessions: Vec::new(),
            active_idx: 0,
            status: "ssh-desk · select a host and press Enter to connect".into(),
            hub,
            events_rx,
            clipboard: Clipboard::new(),
            path_prompt: None,
            host_form: None,
            vault_unlock: None,
            last_geo: None,
            mouse_press: None,
            drag: None,
            drop_target: None,
            pending_drop: None,
            pending_open_parent: false,
            os_drop: None,
            show_session_switcher: false,
            full_screen: false,
            files_search: None,
            should_quit: false,
            connect_in_flight: false,
            connecting_host: None,
            connect_spinner: 0,
            pending_connect: None,
            overwrite_prompt: None,
            files_prompt: None,
            diagnostics: DiagnosticsState::default(),
            restore: None,
        }
    }

    fn note(&mut self, level: DiagLevel, msg: impl Into<String>) {
        let msg = msg.into();
        self.diagnostics.push(level, msg.clone());
        match level {
            DiagLevel::Error => self.status = format!("error: {msg} · F9 log"),
            DiagLevel::Warn => self.status = format!("{msg} · F9 log"),
            DiagLevel::Info => self.status = msg,
        }
    }

    fn note_status(&mut self, level: DiagLevel, status: impl Into<String>, log: impl Into<String>) {
        let status = status.into();
        self.diagnostics.push(level, log.into());
        self.status = status;
    }

    // ── per-session accessors ──────────────────────────────────────────────────

    fn slot(&self) -> Option<&SessionSlot> {
        self.sessions.get(self.active_idx)
    }

    fn slot_mut(&mut self) -> Option<&mut SessionSlot> {
        self.sessions.get_mut(self.active_idx)
    }

    fn active_host_id(&self) -> Option<&str> {
        self.slot().map(|s| s.host_id.as_str())
    }

    fn desktop(&self) -> Option<&Desktop> {
        self.slot().map(|s| &s.desktop)
    }

    fn desktop_mut(&mut self) -> Option<&mut Desktop> {
        self.slot_mut().map(|s| &mut s.desktop)
    }

    fn selected_profile(&self) -> Option<&HostProfile> {
        self.hosts.get(self.selected_host)
    }

    async fn connect_selected(&mut self) -> Result<()> {
        let Some(profile) = self.selected_profile().cloned() else {
            self.status = "no hosts in vault — press a to add one".into();
            return Ok(());
        };
        // Already connected → switch to that tab instead of reconnecting.
        if let Some(pos) = self.sessions.iter().position(|s| s.host_id == profile.id) {
            self.active_idx = pos;
            self.screen = Screen::Desktop;
            self.status = format!(
                "session [{}/{}] · {}",
                pos + 1,
                self.sessions.len(),
                profile.name
            );
            return Ok(());
        }
        if matches!(profile.auth, AuthMethod::Password { .. }) {
            self.vault_unlock = Some(VaultUnlockPrompt::new(profile.name.clone()));
            self.status = format!("unlock vault for {} · Enter", profile.name);
            return Ok(());
        }
        self.connect_profile(profile, None).await
    }

    async fn connect_profile(
        &mut self,
        profile: HostProfile,
        password_passphrase: Option<String>,
    ) -> Result<()> {
        self.begin_connect(profile, password_passphrase);
        Ok(())
    }

    /// Start SSH connect in the background so the UI can animate a spinner.
    fn begin_connect(&mut self, profile: HostProfile, password_passphrase: Option<String>) {
        if self.connect_in_flight {
            return;
        }
        self.connect_in_flight = true;
        self.connect_spinner = 0;
        self.connecting_host = Some(profile.name.clone());
        self.status = format!("connecting to {}…", profile.name);
        if let Some(prompt) = self.vault_unlock.as_mut() {
            prompt.connecting = true;
            prompt.spinner_frame = 0;
        }

        let hub = Arc::clone(&self.hub);
        let vault = self.vault.clone();
        let profile_c = profile.clone();
        let pass = password_passphrase.clone();
        let (tx, rx) = oneshot::channel();
        tokio::spawn(async move {
            let result = hub
                .connect(profile_c.clone(), &vault, pass.as_deref())
                .await
                .map(|_| ())
                .map_err(|e| e.to_string());
            let _ = tx.send(ConnectOutcome {
                profile: profile_c,
                password_passphrase: pass,
                result,
            });
        });
        self.pending_connect = Some(rx);
    }

    fn tick_connect_spinner(&mut self) {
        if !self.connect_in_flight {
            return;
        }
        self.connect_spinner = self.connect_spinner.wrapping_add(1);
        if let Some(prompt) = self.vault_unlock.as_mut() {
            if prompt.connecting {
                prompt.spinner_frame = self.connect_spinner;
            }
        }
        if let Some(name) = &self.connecting_host {
            let spin = Self::connect_spinner_glyph(self.connect_spinner);
            self.status = format!("{spin} connecting to {name}…");
        }
    }

    fn poll_connect_outcome(&mut self) -> Option<ConnectOutcome> {
        let rx = self.pending_connect.as_mut()?;
        match rx.try_recv() {
            Ok(outcome) => {
                self.pending_connect = None;
                Some(outcome)
            }
            Err(oneshot::error::TryRecvError::Empty) => None,
            Err(oneshot::error::TryRecvError::Closed) => {
                self.pending_connect = None;
                let name = self
                    .connecting_host
                    .clone()
                    .unwrap_or_else(|| "host".into());
                Some(ConnectOutcome {
                    profile: HostProfile::new(name, "", ""),
                    password_passphrase: None,
                    result: Err("connection task ended unexpectedly".into()),
                })
            }
        }
    }

    async fn apply_connect_outcome(&mut self, outcome: ConnectOutcome) -> Result<()> {
        let ConnectOutcome {
            profile,
            password_passphrase,
            result,
        } = outcome;

        match result {
            Err(e) => {
                self.connect_in_flight = false;
                self.connecting_host = None;
                if self.restore.is_some() {
                    self.vault_unlock = None;
                    self.note_status(
                        DiagLevel::Error,
                        format!("restore failed · {} · {e}", profile.name),
                        format!("restore failed · {} · {e}", profile.name),
                    );
                    self.advance_restore();
                    return Ok(());
                }
                if let Some(prompt) = self.vault_unlock.as_mut() {
                    prompt.connecting = false;
                    prompt.error = Some(e.clone());
                    prompt.buffer.clear();
                } else {
                    self.vault_unlock = None;
                }
                self.note_status(
                    DiagLevel::Error,
                    format!("connection failed · {e}"),
                    format!("connection failed · {e}"),
                );
                return Ok(());
            }
            Ok(()) => {
                self.status = format!("connected · {}", profile.name);
            }
        }

        self.vault_unlock = None;
        self.connecting_host = None;
        let online = true;

        let host_snap: Option<PersistedHostSession> = self
            .restore
            .as_ref()
            .and_then(|r| r.snapshot.find_host(&profile.id).cloned());
        let restored = host_snap.is_some();
        let restore_cwd = host_snap.as_ref().and_then(|s| s.files_cwd.clone());
        let desktop = match &host_snap {
            Some(saved) => Desktop::with_tree(
                profile.id.clone(),
                profile.name.clone(),
                PaneTree::from_layout(saved.layout.clone(), saved.focused_index),
            ),
            None => Desktop::new(profile.id.clone(), profile.name.clone()),
        };

        // Find or create a slot for this host.
        let slot_idx = if let Some(pos) = self.sessions.iter().position(|s| s.host_id == profile.id)
        {
            let slot = &mut self.sessions[pos];
            slot.cached_passphrase = password_passphrase;
            slot.files = FilesState::default();
            slot.viewer.clear();
            slot.editor.clear();
            slot.processes = ProcessesState::default();
            slot.term = TermEmulator::new(24, 80);
            slot.term
                .write_str(&format!("Connected to '{}'.\r\n", profile.name));
            slot.desktop = desktop;
            pos
        } else {
            let mut slot = SessionSlot::new(profile.id.clone(), profile.name.clone(), online);
            slot.cached_passphrase = password_passphrase;
            slot.desktop = desktop;
            slot.term
                .write_str(&format!("Connected to '{}'.\r\n", profile.name));
            self.sessions.push(slot);
            self.sessions.len() - 1
        };
        // During multi-restore keep focus on the latest connected tab until finish.
        self.active_idx = slot_idx;
        self.screen = Screen::Desktop;
        self.connect_in_flight = false;

        if online {
            if let Some(cwd) = restore_cwd {
                let path = PathBuf::from(&cwd);
                if self.load_dir(path).await.is_err() {
                    let _ = self.refresh_files_home().await;
                }
            } else if let Err(e) = self.refresh_files_home().await {
                if let Some(s) = self.sessions.get_mut(slot_idx) {
                    s.files = FilesState::demo();
                }
                self.status = format!("connected · files: {e}");
            }
            if restored {
                if let Some(r) = self.restore.as_mut() {
                    r.completed += 1;
                    self.status =
                        format!("restored {}/{} · {}", r.completed, r.total, profile.name);
                } else {
                    self.status = format!("restored session · {}", profile.name);
                }
            }
            let _ = self.refresh_processes().await;
        }

        if self.restore.is_some() {
            self.advance_restore();
            return Ok(());
        }

        let session_count = self.sessions.len();
        if session_count > 1 {
            self.status = format!(
                "{} sessions · Ctrl+Tab switch · F8 picker · {}",
                session_count, self.status
            );
        }
        Ok(())
    }

    fn connect_spinner_glyph(frame: u8) -> &'static str {
        const FRAMES: &[&str] = &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
        FRAMES[(frame as usize) % FRAMES.len()]
    }

    /// Persist all open host tabs + decks (or clear) and quit.
    fn request_quit(&mut self) {
        if self.sessions.is_empty() {
            if let Err(e) = session::clear() {
                tracing::warn!(error = %e, "failed to clear session on quit");
            }
        } else {
            let active_host_id = self
                .sessions
                .get(self.active_idx)
                .map(|s| s.host_id.clone())
                .unwrap_or_default();
            let snap = PersistedSession {
                version: 2,
                active_host_id,
                sessions: self
                    .sessions
                    .iter()
                    .map(|slot| PersistedHostSession {
                        host_id: slot.host_id.clone(),
                        layout: slot.desktop.tree.to_layout(),
                        focused_index: slot.desktop.tree.focused_index(),
                        files_cwd: Some(slot.files.cwd_display()),
                    })
                    .collect(),
            };
            if let Err(e) = session::save(&snap) {
                tracing::warn!(error = %e, "failed to save session on quit");
            }
        }
        self.should_quit = true;
    }

    /// Resume every host tab that was open on last Ctrl+Q, if any.
    fn try_restore_session(&mut self) {
        let Some(saved) = session::load() else {
            return;
        };
        let mut queue = VecDeque::new();
        for host in &saved.sessions {
            if self.hosts.iter().any(|h| h.id == host.host_id) {
                queue.push_back(host.host_id.clone());
            } else {
                tracing::warn!(host = %host.host_id, "saved host missing from vault · skipped");
            }
        }
        if queue.is_empty() {
            let _ = session::clear();
            self.status = "saved sessions missing · pick a host".into();
            return;
        }
        let total = queue.len();
        let needs_password = queue.iter().any(|id| {
            self.hosts
                .iter()
                .any(|h| h.id == *id && matches!(h.auth, AuthMethod::Password { .. }))
        });
        self.restore = Some(RestoreState {
            snapshot: saved,
            queue,
            passphrase: None,
            total,
            completed: 0,
        });
        if needs_password {
            let label = if total == 1 {
                self.hosts
                    .iter()
                    .find(|h| {
                        self.restore
                            .as_ref()
                            .and_then(|r| r.queue.front())
                            .is_some_and(|id| h.id == *id)
                    })
                    .map(|h| h.name.clone())
                    .unwrap_or_else(|| "hosts".into())
            } else {
                format!("{total} hosts")
            };
            self.vault_unlock = Some(VaultUnlockPrompt::new(label));
            self.status = format!("restore · unlock vault for {total} session(s)");
        } else {
            self.status = format!("restoring {total} session(s)…");
            self.advance_restore();
        }
    }

    /// Start the next queued restore connect, or finish when the queue is empty.
    fn advance_restore(&mut self) {
        if self.restore.is_none() {
            return;
        }
        while let Some(host_id) = self.restore.as_mut().and_then(|rs| rs.queue.pop_front()) {
            let Some(profile) = self.hosts.iter().find(|h| h.id == host_id).cloned() else {
                continue;
            };
            let pass = if matches!(profile.auth, AuthMethod::Password { .. }) {
                self.restore.as_ref().and_then(|rs| rs.passphrase.clone())
            } else {
                None
            };
            if matches!(profile.auth, AuthMethod::Password { .. }) && pass.is_none() {
                // Should not happen after unlock — re-queue and wait.
                if let Some(rs) = self.restore.as_mut() {
                    rs.queue.push_front(host_id);
                }
                return;
            }
            let (step, total) = self
                .restore
                .as_ref()
                .map(|rs| (rs.completed + 1, rs.total))
                .unwrap_or((1, 1));
            self.status = format!("restoring {step}/{total} · {}…", profile.name);
            if let Some(idx) = self.hosts.iter().position(|h| h.id == profile.id) {
                self.selected_host = idx;
            }
            self.begin_connect(profile, pass);
            return;
        }
        self.finish_restore();
    }

    fn finish_restore(&mut self) {
        let active = self
            .restore
            .as_ref()
            .map(|r| r.snapshot.active_host_id.clone());
        let total = self.restore.as_ref().map(|r| r.total).unwrap_or(0);
        let completed = self.restore.as_ref().map(|r| r.completed).unwrap_or(0);
        self.restore = None;
        self.vault_unlock = None;
        if let Some(id) = active {
            if let Some(idx) = self.sessions.iter().position(|s| s.host_id == id) {
                self.active_idx = idx;
            }
        }
        if self.sessions.is_empty() {
            self.screen = Screen::Launcher;
            self.status = "restore finished · no sessions connected".into();
            let _ = session::clear();
        } else {
            self.screen = Screen::Desktop;
            self.status = format!("restored {completed}/{total} session(s)");
        }
    }

    /// Switch to the next session (Ctrl+Tab).
    fn session_next(&mut self) {
        if self.sessions.is_empty() {
            return;
        }
        self.active_idx = (self.active_idx + 1) % self.sessions.len();
        let name = self.sessions[self.active_idx].host_name.clone();
        self.status = format!(
            "session [{}/{}] · {}",
            self.active_idx + 1,
            self.sessions.len(),
            name
        );
    }

    /// Switch to the previous session (Ctrl+Shift+Tab).
    fn session_prev(&mut self) {
        if self.sessions.is_empty() {
            return;
        }
        let n = self.sessions.len();
        self.active_idx = (self.active_idx + n - 1) % n;
        let name = self.sessions[self.active_idx].host_name.clone();
        self.status = format!(
            "session [{}/{}] · {}",
            self.active_idx + 1,
            self.sessions.len(),
            name
        );
    }

    /// Open the host launcher without disconnecting any sessions.
    fn open_launcher(&mut self) {
        self.show_session_switcher = false;
        self.screen = Screen::Launcher;
        let n = self.sessions.len();
        self.status = if n == 0 {
            "launcher · select a host and press Enter".into()
        } else {
            format!("launcher · {n} session(s) still open · Enter connect · Esc back to desktop")
        };
    }

    /// Return to the active desktop session from the launcher (sessions stay open).
    fn resume_desktop(&mut self) -> bool {
        if self.sessions.is_empty() {
            return false;
        }
        self.screen = Screen::Desktop;
        let name = self.sessions[self.active_idx].host_name.clone();
        self.status = format!(
            "session [{}/{}] · {}",
            self.active_idx + 1,
            self.sessions.len(),
            name
        );
        true
    }

    /// Disconnect and close the current session tab.
    async fn close_current_session(&mut self) -> Result<()> {
        if self.sessions.is_empty() {
            self.screen = Screen::Launcher;
            return Ok(());
        }
        let host_id = self.sessions[self.active_idx].host_id.clone();
        let _ = self.hub.disconnect(&host_id).await;
        self.sessions.remove(self.active_idx);
        if self.sessions.is_empty() {
            self.active_idx = 0;
            self.screen = Screen::Launcher;
            self.status = "all sessions closed · back to launcher".into();
        } else {
            self.active_idx = self.active_idx.min(self.sessions.len() - 1);
            let name = self.sessions[self.active_idx].host_name.clone();
            self.status = format!("closed · now on {name}");
        }
        Ok(())
    }

    async fn refresh_files_home(&mut self) -> Result<(), ssh_core::CoreError> {
        let host_id = match self.active_host_id() {
            Some(id) => id.to_owned(),
            None => return Ok(()),
        };
        if !self.hub.has_sftp(&host_id).await {
            if let Some(s) = self.slot_mut() {
                s.files = FilesState::demo();
            }
            return Ok(());
        }
        let home = self.hub.canonicalize(&host_id, ".").await?;
        self.load_dir(home).await
    }

    async fn load_dir(&mut self, path: PathBuf) -> Result<(), ssh_core::CoreError> {
        let host_id = match self.active_host_id() {
            Some(id) => id.to_owned(),
            None => return Ok(()),
        };
        if let Some(s) = self.slot_mut() {
            s.files.loading = true;
            s.files.error = None;
        }
        match self.hub.list_dir(&host_id, &path).await {
            Ok(entries) => {
                if let Some(s) = self.slot_mut() {
                    s.files.set_listing(path.clone(), entries);
                }
                let cwd_display = self
                    .slot()
                    .map(|s| s.files.cwd_display())
                    .unwrap_or_default();
                self.status = format!("files · {}", cwd_display);
                Ok(())
            }
            Err(e) => {
                if let Some(s) = self.slot_mut() {
                    s.files.loading = false;
                    s.files.error = Some(e.to_string());
                }
                self.note_status(
                    DiagLevel::Error,
                    format!("files error · {e}"),
                    format!("files list · {e}"),
                );
                Err(e)
            }
        }
    }

    async fn open_selected_file(&mut self) -> Result<()> {
        let (cwd, row, entries, online) = match self.slot() {
            Some(s) => (
                s.files.cwd.clone(),
                s.files.selected_row(),
                s.files.entries.clone(),
                s.files.online,
            ),
            None => return Ok(()),
        };
        let Some(row) = row else {
            return Ok(());
        };
        let Some((path, is_dir)) = resolve_open_path(&cwd, row, &entries) else {
            return Ok(());
        };

        if is_dir {
            if online {
                if let Err(e) = self.load_dir(path).await {
                    self.note_status(
                        DiagLevel::Error,
                        format!("cd failed · {e}"),
                        format!("cd failed · {e}"),
                    );
                }
            } else if path.ends_with("Documents") {
                if let Some(s) = self.slot_mut() {
                    s.files.cwd = path;
                    s.files.entries = vec![];
                    s.files.selected = 0;
                }
                self.status = "files · /home/demo/Documents (demo empty)".into();
            } else {
                if let Some(s) = self.slot_mut() {
                    s.files = FilesState::demo();
                }
                self.status = "files · demo root".into();
            }
            return Ok(());
        }

        self.open_path(path, false).await
    }

    async fn open_path(&mut self, path: PathBuf, force_editor: bool) -> Result<()> {
        let action = sniff_open_action(&path);
        let into_editor = force_editor || matches!(action, OpenAction::EditText);
        let online = self.slot().map(|s| s.files.online).unwrap_or(false);

        if online {
            let host_id = match self.active_host_id() {
                Some(id) => id.to_owned(),
                None => return Ok(()),
            };
            match self.hub.read_file(&host_id, &path).await {
                Ok(content) => {
                    if into_editor && !content.looks_binary() {
                        if let Some(text) = content.as_text() {
                            let title = if let Some(s) = self.slot_mut() {
                                s.editor = EditorState::from_text(path, text, true);
                                s.desktop.tree.focus_or_open_editor();
                                s.editor.title.clone()
                            } else {
                                String::new()
                            };
                            self.status = format!("editor · {}", title);
                            return Ok(());
                        }
                    }
                    let (cols, rows) = self
                        .last_geo
                        .as_ref()
                        .and_then(|g| g.files_pane_inner)
                        .map(|r| {
                            (
                                r.width.saturating_sub(2).max(20),
                                r.height.saturating_sub(2).max(8),
                            )
                        })
                        .unwrap_or((60, 20));
                    let title = if let Some(s) = self.slot_mut() {
                        s.viewer = ViewerState::from_content(content, cols, rows);
                        s.desktop.tree.focus_or_open_viewer();
                        s.viewer.title.clone()
                    } else {
                        String::new()
                    };
                    self.status = format!("viewer · {}", title);
                }
                Err(e) => self.note_status(
                    DiagLevel::Error,
                    format!("open failed · {e}"),
                    format!("open failed · {e}"),
                ),
            }
        } else if into_editor {
            let title = if let Some(s) = self.slot_mut() {
                s.editor = EditorState::from_text(
                    path.clone(),
                    &ViewerState::demo_file(&path).body,
                    false,
                );
                s.desktop.tree.focus_or_open_editor();
                s.editor.title.clone()
            } else {
                String::new()
            };
            self.status = format!("editor · {} (demo)", title);
        } else {
            let title = if let Some(s) = self.slot_mut() {
                s.viewer = ViewerState::demo_file(&path);
                s.desktop.tree.focus_or_open_viewer();
                s.viewer.title.clone()
            } else {
                String::new()
            };
            self.status = format!("viewer · {} (demo)", title);
        }
        Ok(())
    }

    async fn refresh_processes(&mut self) -> Result<()> {
        let host_id = match self.active_host_id() {
            Some(id) => id.to_owned(),
            None => {
                if let Some(s) = self.slot_mut() {
                    s.processes = ProcessesState::demo();
                }
                return Ok(());
            }
        };
        if !self.hub.is_connected(&host_id).await {
            if let Some(s) = self.slot_mut() {
                s.processes = ProcessesState::demo();
            }
            return Ok(());
        }
        if let Some(s) = self.slot_mut() {
            s.processes.loading = true;
        }
        let cmd = "ps -eo pid,user,pcpu,pmem,comm --sort=-pcpu 2>/dev/null | head -n 50 || ps aux 2>/dev/null | head -n 50";
        match self.hub.exec_capture(&host_id, cmd).await {
            Ok(out) => {
                let row_count = if let Some(s) = self.slot_mut() {
                    s.processes = ProcessesState::from_ps(&out);
                    if s.processes.rows.is_empty() {
                        s.processes.error = Some("no process rows parsed".into());
                    }
                    s.processes.rows.len()
                } else {
                    0
                };
                self.status = format!("processes · {} rows", row_count);
            }
            Err(e) => {
                if let Some(s) = self.slot_mut() {
                    s.processes.loading = false;
                    s.processes.error = Some(e.to_string());
                    s.processes.online = false;
                }
                self.note_status(
                    DiagLevel::Error,
                    format!("processes · {e}"),
                    format!("processes · {e}"),
                );
            }
        }
        Ok(())
    }

    async fn save_editor(&mut self) -> Result<()> {
        let (path, online) = match self.slot() {
            Some(s) => (s.editor.path.clone(), s.editor.online),
            None => return Ok(()),
        };
        let Some(path) = path else {
            return Ok(());
        };
        if !online {
            self.status = "editor · offline demo (cannot save)".into();
            return Ok(());
        }
        let host_id = match self.active_host_id() {
            Some(id) => id.to_owned(),
            None => return Ok(()),
        };
        let data = self.slot().map(|s| s.editor.contents()).unwrap_or_default();
        match self.hub.write_file(&host_id, &path, data.as_bytes()).await {
            Ok(()) => {
                if let Some(s) = self.slot_mut() {
                    s.editor.dirty = false;
                }
                self.status = format!("saved · {}", path.display());
            }
            Err(e) => self.note_status(
                DiagLevel::Error,
                format!("save failed · {e}"),
                format!("save failed · {} · {e}", path.display()),
            ),
        }
        Ok(())
    }

    fn drain_events(&mut self) {
        while let Ok(ev) = self.events_rx.try_recv() {
            match ev {
                SessionEvent::Connected { host_id } => {
                    self.status = format!("connected · {host_id}");
                }
                SessionEvent::Disconnected { host_id, reason } => {
                    self.note_status(
                        DiagLevel::Warn,
                        format!(
                            "disconnected · {host_id}: {reason} · attempting auto-reconnect..."
                        ),
                        format!("disconnected · {host_id}: {reason}"),
                    );
                    // Trigger silent auto-reconnect attempt
                    if let Some(pos) = self.sessions.iter().position(|s| s.host_id == host_id) {
                        let slot = &self.sessions[pos];
                        let pass = slot.cached_passphrase.clone();
                        if let Some(profile) = self.hosts.iter().find(|h| h.id == host_id).cloned()
                        {
                            let hub = Arc::clone(&self.hub);
                            let vault = self.vault.clone();
                            tokio::spawn(async move {
                                let p_pass = pass.as_deref();
                                let _ = hub.connect(profile, &vault, p_pass).await;
                            });
                        }
                    }
                }
                SessionEvent::PtyData(out) => {
                    // Feed raw PTY bytes into the VT100 emulator (active session).
                    if let Some(slot) = self.slot_mut() {
                        slot.term.process(&out.data);
                    }
                }
                SessionEvent::TransferUpdate(job) => {
                    let done_upload = job.status == ssh_core::TransferStatus::Done
                        && job.direction == ssh_core::TransferDirection::Upload;
                    let name = job.display_name();
                    let status = job.status;
                    let job_host = job.host_id.clone();
                    if let Some(slot) = self.sessions.iter_mut().find(|s| s.host_id == job_host) {
                        slot.transfers.upsert(job);
                        if done_upload && slot.files.online {
                            slot.pending_files_refresh = true;
                        }
                    }
                    self.status = format!("transfer · {} · {}", name, status.label());
                }
                SessionEvent::Status(msg) => {
                    self.diagnostics
                        .push(DiagLevel::Info, format!("status · {msg}"));
                    self.status = msg;
                }
                SessionEvent::Error(msg) => {
                    self.note(DiagLevel::Error, msg);
                }
            }
        }
    }

    async fn handle_key(&mut self, key: KeyEvent) -> Result<()> {
        // Windows reports Press + Release for every key; ignore non-press to avoid double input.
        if key.kind != KeyEventKind::Press {
            return Ok(());
        }
        if key.code == KeyCode::F(9) {
            self.diagnostics.toggle();
            self.status = if self.diagnostics.open {
                "diagnostics · j/k scroll · c clear · Esc/F9 close".into()
            } else {
                "diagnostics closed".into()
            };
            return Ok(());
        }
        if self.diagnostics.open {
            match key.code {
                KeyCode::Esc => {
                    self.diagnostics.close();
                    self.status = "diagnostics closed".into();
                }
                KeyCode::Up | KeyCode::Char('k') => self.diagnostics.scroll_up(),
                KeyCode::Down | KeyCode::Char('j') => self.diagnostics.scroll_down(),
                KeyCode::PageUp => {
                    for _ in 0..10 {
                        self.diagnostics.scroll_up();
                    }
                }
                KeyCode::PageDown => {
                    for _ in 0..10 {
                        self.diagnostics.scroll_down();
                    }
                }
                KeyCode::Home => self.diagnostics.scroll_home(),
                KeyCode::End => self.diagnostics.scroll_end(),
                KeyCode::Char('c') => self.diagnostics.clear(),
                _ => {}
            }
            return Ok(());
        }
        if self.os_drop.is_some() {
            self.handle_os_drop_key(key).await?;
            return Ok(());
        }
        if self.vault_unlock.is_some() {
            if key.modifiers.contains(KeyModifiers::CONTROL)
                && matches!(key.code, KeyCode::Char('q') | KeyCode::Char('Q'))
            {
                self.request_quit();
                return Ok(());
            }
            self.handle_vault_unlock_key(key).await?;
            return Ok(());
        }
        if self.host_form.is_some() {
            self.handle_host_form_key(key)?;
            return Ok(());
        }
        if self.overwrite_prompt.is_some() {
            self.handle_overwrite_prompt_key(key).await?;
            return Ok(());
        }
        if self.files_prompt.is_some() {
            self.handle_files_prompt_key(key).await?;
            return Ok(());
        }
        if self.path_prompt.is_some() {
            self.handle_path_prompt_key(key).await?;
            return Ok(());
        }
        match self.screen {
            Screen::Launcher => self.handle_launcher_key(key).await?,
            Screen::Desktop => self.handle_desktop_key(key).await?,
        }
        Ok(())
    }

    async fn handle_paste(&mut self, data: String) -> Result<()> {
        match classify_paste(&data) {
            PasteKind::Paths(paths) => {
                let files = existing_files(&paths);
                if files.is_empty() {
                    self.status = format!(
                        "drop · {} path(s) parsed, none are local files",
                        paths.len()
                    );
                    return Ok(());
                }
                self.offer_os_upload(files);
            }
            PasteKind::Text(text) => {
                // Forward plain text to the PTY when the terminal pane is focused.
                if self.screen == Screen::Desktop {
                    let focused = self
                        .desktop()
                        .map(|d| d.focused_app())
                        .unwrap_or(AppKind::Terminal);
                    if focused == AppKind::Terminal {
                        if let Some(host_id) = self.active_host_id().map(str::to_owned) {
                            if self.hub.is_connected(&host_id).await {
                                let _ = self.hub.write_pty(&host_id, text.as_bytes()).await;
                                return Ok(());
                            }
                        }
                        if let Some(s) = self.slot_mut() {
                            s.term.write_str(&text);
                        }
                        return Ok(());
                    }
                }
                self.status = "paste · not a file path list (drop files from OS, or Ctrl+L)".into();
            }
        }
        Ok(())
    }

    fn offer_os_upload(&mut self, paths: Vec<PathBuf>) {
        if !self.slot().map(|s| s.files.online).unwrap_or(false) {
            self.status = "OS drop · connect a session to upload".into();
            // Still park paths on the file clipboard for later paste.
            let entries: Vec<FileEntry> = paths
                .into_iter()
                .map(|p| {
                    let is_dir = p.is_dir();
                    FileEntry::local(p, is_dir)
                })
                .collect();
            let n = entries.len();
            self.clipboard.set_files(entries, FileOp::Copy);
            self.status =
                format!("OS drop · {n} file(s) on clipboard · connect then Ctrl+V to upload");
            return;
        }
        let dest = self.slot().map(|s| s.files.cwd.clone()).unwrap_or_default();
        let n = paths.len();
        self.os_drop = Some(OsDropOffer::new(paths, dest));
        self.status = format!("OS drop · confirm upload of {n} file(s) · Enter/y · Esc");
    }

    async fn handle_overwrite_prompt_key(&mut self, key: KeyEvent) -> Result<()> {
        let Some(prompt) = self.overwrite_prompt.as_mut() else {
            return Ok(());
        };
        match key.code {
            KeyCode::Esc | KeyCode::Char('n') | KeyCode::Char('N') => {
                self.overwrite_prompt = None;
                self.status = "paste cancelled".into();
            }
            KeyCode::Left | KeyCode::Char('h') => prompt.selected = 0,
            KeyCode::Right | KeyCode::Char('l') => prompt.selected = 1,
            KeyCode::Tab => prompt.selected = 1 - prompt.selected,
            KeyCode::Enter | KeyCode::Char('y') | KeyCode::Char('Y') => {
                if prompt.selected == 1 {
                    self.overwrite_prompt = None;
                    self.status = "paste cancelled".into();
                } else {
                    let prompt = self.overwrite_prompt.take().unwrap();
                    self.execute_paste_clipboard_into(prompt.dest_dir, prompt.entries, prompt.op)
                        .await?;
                }
            }
            _ => {}
        }
        Ok(())
    }

    fn begin_rename_prompt(&mut self) {
        if !self.slot().map(|s| s.files.online).unwrap_or(false) {
            self.status = "rename needs a live SFTP session".into();
            return;
        }
        let Some(row) = self.slot().and_then(|s| s.files.selected_row()) else {
            return;
        };
        let FilesRow::Entry(i) = row else {
            self.status = "cannot rename ..".into();
            return;
        };
        let Some(entry) = self.slot().and_then(|s| s.files.entries.get(i).cloned()) else {
            return;
        };
        self.files_prompt = Some(FilesPrompt::rename(entry.path.clone(), entry.name.clone()));
        self.status = "rename · edit name · Enter · Esc".into();
    }

    fn begin_delete_prompt(&mut self) {
        if !self.slot().map(|s| s.files.online).unwrap_or(false) {
            self.status = "delete needs a live SFTP session".into();
            return;
        }
        let targets: Vec<(PathBuf, bool, String)> = self
            .slot()
            .map(|s| {
                s.files
                    .clipboard_targets()
                    .into_iter()
                    .map(|e| (e.path.clone(), e.is_dir, e.display_name()))
                    .collect()
            })
            .unwrap_or_default();
        if targets.is_empty() {
            self.status = "nothing to delete".into();
            return;
        }
        let names: Vec<String> = targets.iter().map(|(_, _, n)| n.clone()).collect();
        let paths: Vec<PathBuf> = targets.into_iter().map(|(p, _, _)| p).collect();
        self.files_prompt = Some(FilesPrompt::delete(names, paths));
        self.status = "delete · confirm · Enter · Esc".into();
    }

    async fn handle_files_prompt_key(&mut self, key: KeyEvent) -> Result<()> {
        if self.files_prompt.is_none() {
            return Ok(());
        }
        match key.code {
            KeyCode::Esc => {
                self.files_prompt = None;
                self.status = "cancelled".into();
                return Ok(());
            }
            _ => {}
        }

        let is_delete = matches!(self.files_prompt, Some(FilesPrompt::Delete { .. }));
        if is_delete {
            let Some(FilesPrompt::Delete { selected, .. }) = self.files_prompt.as_mut() else {
                return Ok(());
            };
            match key.code {
                KeyCode::Left | KeyCode::Char('h') => *selected = 0,
                KeyCode::Right | KeyCode::Char('l') => *selected = 1,
                KeyCode::Tab => *selected = 1 - *selected,
                KeyCode::Char('y') | KeyCode::Char('Y') => {
                    *selected = 0;
                    self.submit_files_prompt().await?;
                }
                KeyCode::Char('n') | KeyCode::Char('N') => {
                    self.files_prompt = None;
                    self.status = "delete cancelled".into();
                }
                KeyCode::Enter => self.submit_files_prompt().await?,
                _ => {}
            }
            return Ok(());
        }

        match key.code {
            KeyCode::Enter => self.submit_files_prompt().await?,
            KeyCode::Backspace => {
                if let Some(buf) = self.files_prompt.as_mut().and_then(|p| p.buffer_mut()) {
                    buf.pop();
                }
            }
            KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                if c == '/' || c == '\0' {
                    return Ok(());
                }
                if let Some(buf) = self.files_prompt.as_mut().and_then(|p| p.buffer_mut()) {
                    buf.push(c);
                }
            }
            _ => {}
        }
        Ok(())
    }

    async fn submit_files_prompt(&mut self) -> Result<()> {
        let Some(prompt) = self.files_prompt.take() else {
            return Ok(());
        };
        let Some(host_id) = self.active_host_id().map(str::to_owned) else {
            self.status = "no session".into();
            return Ok(());
        };
        let cwd = self
            .slot()
            .map(|s| s.files.cwd.clone())
            .unwrap_or_else(|| PathBuf::from("/"));

        match prompt {
            FilesPrompt::Mkdir { buffer, .. } => {
                let name = buffer.trim();
                if name.is_empty() || name.contains('/') || name == "." || name == ".." {
                    let mut p = FilesPrompt::mkdir();
                    if let FilesPrompt::Mkdir { buffer: b, .. } = &mut p {
                        *b = buffer;
                    }
                    p.set_error("invalid folder name");
                    self.files_prompt = Some(p);
                    self.status = "mkdir · invalid name".into();
                    return Ok(());
                }
                let path = join_remote(&cwd, name);
                match self.hub.remote_mkdir(&host_id, &path).await {
                    Ok(()) => {
                        self.status = format!("created · {name}/");
                        let _ = self.load_dir(cwd).await;
                    }
                    Err(e) => {
                        let mut p = FilesPrompt::mkdir();
                        if let FilesPrompt::Mkdir { buffer: b, .. } = &mut p {
                            *b = buffer;
                        }
                        p.set_error(e.to_string());
                        self.files_prompt = Some(p);
                        self.note_status(
                            DiagLevel::Error,
                            format!("mkdir failed · {e}"),
                            format!("mkdir failed · {e}"),
                        );
                    }
                }
            }
            FilesPrompt::Rename { from, buffer, .. } => {
                let name = buffer.trim();
                if name.is_empty() || name.contains('/') || name == "." || name == ".." {
                    let mut p = FilesPrompt::rename(from, buffer);
                    p.set_error("invalid name");
                    self.files_prompt = Some(p);
                    return Ok(());
                }
                let parent = from.parent().unwrap_or(Path::new("/"));
                let to = join_remote(parent, name);
                match self.hub.remote_rename(&host_id, &from, &to).await {
                    Ok(()) => {
                        self.status = format!("renamed · {name}");
                        let _ = self.load_dir(cwd).await;
                    }
                    Err(e) => {
                        let mut p = FilesPrompt::rename(from, buffer);
                        p.set_error(e.to_string());
                        self.files_prompt = Some(p);
                        self.note_status(
                            DiagLevel::Error,
                            format!("rename failed · {e}"),
                            format!("rename failed · {e}"),
                        );
                    }
                }
            }
            FilesPrompt::Delete {
                names,
                paths,
                selected,
            } => {
                if selected != 0 {
                    self.status = "delete cancelled".into();
                    return Ok(());
                }
                let mut ok = 0usize;
                let mut errors = Vec::new();
                for path in paths {
                    match self.hub.remote_remove(&host_id, &path).await {
                        Ok(()) => ok += 1,
                        Err(e) => errors.push(format!("{}: {e}", path.display())),
                    }
                }
                if errors.is_empty() {
                    self.status = format!("deleted · {ok} item(s)");
                } else {
                    for e in &errors {
                        self.diagnostics
                            .push(DiagLevel::Error, format!("delete · {e}"));
                    }
                    self.status = format!(
                        "delete incomplete · {ok} ok · {} errors · F9 log",
                        errors.len()
                    );
                }
                let _ = names;
                let _ = self.load_dir(cwd).await;
            }
        }
        Ok(())
    }

    async fn handle_os_drop_key(&mut self, key: KeyEvent) -> Result<()> {
        let Some(offer) = self.os_drop.as_mut() else {
            return Ok(());
        };
        match key.code {
            KeyCode::Esc | KeyCode::Char('n') | KeyCode::Char('N') => {
                self.os_drop = None;
                self.status = "OS drop cancelled".into();
            }
            KeyCode::Left | KeyCode::Char('h') => offer.selected = 0,
            KeyCode::Right | KeyCode::Char('l') => offer.selected = 1,
            KeyCode::Tab => offer.selected = 1 - offer.selected,
            KeyCode::Enter | KeyCode::Char('y') | KeyCode::Char('Y') => {
                if offer.selected == 1 {
                    self.os_drop = None;
                    self.status = "OS drop cancelled".into();
                } else {
                    let offer = self.os_drop.take().unwrap();
                    self.queue_os_uploads(offer.paths, offer.dest).await?;
                }
            }
            KeyCode::Char('u') | KeyCode::Char('U') => {
                let offer = self.os_drop.take().unwrap();
                self.queue_os_uploads(offer.paths, offer.dest).await?;
            }
            _ => {}
        }
        Ok(())
    }

    async fn queue_os_uploads(&mut self, paths: Vec<PathBuf>, dest: PathBuf) -> Result<()> {
        let Some(host_id) = self.active_host_id().map(str::to_owned) else {
            self.status = "no session".into();
            return Ok(());
        };
        let mut queued = 0usize;
        for path in paths {
            match self
                .hub
                .enqueue_upload(&host_id, path.clone(), dest.clone())
                .await
            {
                Ok(_) => queued += 1,
                Err(e) => {
                    self.status = format!("upload failed · {}: {e}", path.display());
                }
            }
        }
        if let Some(s) = self.slot_mut() {
            s.pending_files_refresh = true;
        }
        self.status = format!("queued {queued} upload(s) → {}", dest.display());
        if let Some(desktop) = self.desktop_mut() {
            if let Some((tid, _)) = desktop
                .tree
                .leaves()
                .into_iter()
                .find(|(_, a)| *a == AppKind::Transfers)
            {
                desktop.tree.set_focus(tid);
            }
        }
        Ok(())
    }

    async fn handle_launcher_key(&mut self, key: KeyEvent) -> Result<()> {
        match key.code {
            KeyCode::Char('q') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                self.request_quit();
            }
            KeyCode::Char('q') => self.request_quit(),
            KeyCode::Esc => {
                if !self.resume_desktop() {
                    self.status = "no open sessions · pick a host and press Enter".into();
                }
            }
            KeyCode::Up | KeyCode::Char('k') => {
                if self.selected_host > 0 {
                    self.selected_host -= 1;
                }
            }
            KeyCode::Down | KeyCode::Char('j') => {
                if self.selected_host + 1 < self.hosts.len() {
                    self.selected_host += 1;
                }
            }
            KeyCode::Enter => self.connect_selected().await?,
            KeyCode::Char('a') | KeyCode::Char('n') => {
                self.host_form = Some(HostForm::new());
                self.status = "add host · Tab fields · Space cycle auth · Ctrl+S save · Esc".into();
            }
            KeyCode::Char('d') | KeyCode::Delete => {
                self.delete_selected_host()?;
            }
            KeyCode::Char('r') => {
                self.vault = Vault::open_default()?;
                self.hosts = self.vault.hosts().to_vec();
                if self.selected_host >= self.hosts.len() && !self.hosts.is_empty() {
                    self.selected_host = self.hosts.len() - 1;
                }
                self.status = "vault reloaded".into();
            }
            _ => {}
        }
        Ok(())
    }

    fn delete_selected_host(&mut self) -> Result<()> {
        let Some(profile) = self.selected_profile().cloned() else {
            self.status = "no host to delete".into();
            return Ok(());
        };
        match self.vault.remove(&profile.id) {
            Ok(()) => {
                self.hosts = self.vault.hosts().to_vec();
                if self.selected_host >= self.hosts.len() && !self.hosts.is_empty() {
                    self.selected_host = self.hosts.len() - 1;
                }
                self.status = format!("deleted · {}", profile.name);
            }
            Err(e) => self.status = format!("delete failed · {e}"),
        }
        Ok(())
    }

    fn handle_host_form_key(&mut self, key: KeyEvent) -> Result<()> {
        if self.host_form.is_none() {
            return Ok(());
        }

        if key.code == KeyCode::Esc {
            self.host_form = None;
            self.status = "add host cancelled".into();
            return Ok(());
        }

        let should_save = matches!(
            (key.modifiers, key.code),
            (KeyModifiers::CONTROL, KeyCode::Char('s'))
        ) || {
            let form = self.host_form.as_ref().unwrap();
            key.code == KeyCode::Enter && form.active_fields().last().copied() == Some(form.focus)
        };

        if should_save {
            return self.submit_host_form();
        }

        let form = self.host_form.as_mut().unwrap();
        match (key.modifiers, key.code) {
            (_, KeyCode::Tab) | (_, KeyCode::Enter) => {
                if key.modifiers.contains(KeyModifiers::SHIFT) {
                    form.focus_prev();
                } else {
                    form.focus_next();
                }
            }
            (KeyModifiers::SHIFT, KeyCode::BackTab) => form.focus_prev(),
            (_, KeyCode::Backspace) => form.backspace(),
            (_, KeyCode::Char(' ')) if form.focus == crate::hostform::HostField::Auth => {
                form.cycle_auth();
            }
            (_, KeyCode::Left | KeyCode::Right | KeyCode::Char('h') | KeyCode::Char('l'))
                if form.focus == crate::hostform::HostField::Auth =>
            {
                form.cycle_auth();
            }
            (_, KeyCode::Char(c)) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                form.insert_char(c);
            }
            _ => {}
        }
        Ok(())
    }

    fn submit_host_form(&mut self) -> Result<()> {
        let Some(form) = self.host_form.take() else {
            return Ok(());
        };
        match form.save(&mut self.vault) {
            Ok(profile) => {
                self.hosts = self.vault.hosts().to_vec();
                if let Some(idx) = self.hosts.iter().position(|h| h.id == profile.id) {
                    self.selected_host = idx;
                }
                self.status = format!(
                    "added · {} ({}@{}:{})",
                    profile.name, profile.user, profile.host, profile.port
                );
            }
            Err(e) => {
                let mut form = form;
                form.error = Some(e.clone());
                self.host_form = Some(form);
                self.status = format!("add host · {e}");
            }
        }
        Ok(())
    }

    async fn handle_vault_unlock_key(&mut self, key: KeyEvent) -> Result<()> {
        if self.vault_unlock.is_none() {
            return Ok(());
        }
        match key.code {
            KeyCode::Esc => {
                if self.vault_unlock.as_ref().is_some_and(|p| p.connecting) {
                    return Ok(());
                }
                self.vault_unlock = None;
                if self.restore.take().is_some() {
                    let _ = session::clear();
                    self.status = "restore cancelled".into();
                } else {
                    self.status = "connect cancelled".into();
                }
            }
            KeyCode::Backspace => {
                if self.vault_unlock.as_ref().is_some_and(|p| p.connecting) {
                    return Ok(());
                }
                if let Some(prompt) = self.vault_unlock.as_mut() {
                    prompt.buffer.pop();
                    prompt.error = None;
                }
            }
            KeyCode::Enter => {
                if self.vault_unlock.as_ref().is_some_and(|p| p.connecting) {
                    return Ok(());
                }
                let passphrase = self
                    .vault_unlock
                    .as_ref()
                    .map(|p| p.buffer.clone())
                    .unwrap_or_default();
                if passphrase.is_empty() {
                    if let Some(prompt) = self.vault_unlock.as_mut() {
                        prompt.error = Some("passphrase required".into());
                    }
                    return Ok(());
                }
                if self.restore.is_some() {
                    if let Some(rs) = self.restore.as_mut() {
                        rs.passphrase = Some(passphrase);
                    }
                    if let Some(prompt) = self.vault_unlock.as_mut() {
                        prompt.connecting = true;
                        prompt.error = None;
                    }
                    self.advance_restore();
                    return Ok(());
                }
                let profile = match self.selected_profile().cloned() {
                    Some(p) => p,
                    None => {
                        self.vault_unlock = None;
                        return Ok(());
                    }
                };
                if let Some(prompt) = self.vault_unlock.as_mut() {
                    prompt.connecting = true;
                    prompt.error = None;
                }
                self.begin_connect(profile, Some(passphrase));
            }
            KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                if self.vault_unlock.as_ref().is_some_and(|p| p.connecting) {
                    return Ok(());
                }
                if let Some(prompt) = self.vault_unlock.as_mut() {
                    prompt.buffer.push(c);
                    prompt.error = None;
                }
            }
            _ => {}
        }
        Ok(())
    }

    async fn handle_desktop_key(&mut self, key: KeyEvent) -> Result<()> {
        // ── Session management keys ─────────────────────────────────────
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let shift = key.modifiers.contains(KeyModifiers::SHIFT);
        if ctrl && key.code == KeyCode::Tab && !shift {
            self.session_next();
            return Ok(());
        }
        if ctrl && key.code == KeyCode::BackTab {
            self.session_prev();
            return Ok(());
        }
        if key.code == KeyCode::F(8) {
            self.show_session_switcher = !self.show_session_switcher;
            return Ok(());
        }
        // Hosts launcher without disconnecting (open another session).
        if ctrl && matches!(key.code, KeyCode::Char('n') | KeyCode::Char('N')) {
            self.open_launcher();
            return Ok(());
        }
        // Quit app (saves open host + deck, or clears if launcher-only).
        if ctrl && matches!(key.code, KeyCode::Char('q') | KeyCode::Char('Q')) {
            self.request_quit();
            return Ok(());
        }

        // ── Files Search Mode ───────────────────────────────────────────
        if self.files_search.is_some() {
            match key.code {
                KeyCode::Esc => {
                    self.files_search = None;
                    if let Some(s) = self.slot_mut() {
                        s.files.search_query = None;
                        s.files.selected = 0;
                    }
                    self.status = "search cancelled".into();
                }
                KeyCode::Enter => {
                    self.files_search = None;
                    self.status = "search query locked".into();
                }
                KeyCode::Backspace => {
                    if let Some(q) = self.files_search.as_mut() {
                        q.pop();
                    }
                    let q = self.files_search.clone();
                    if let Some(s) = self.slot_mut() {
                        s.files.search_query = q.clone();
                        s.files.selected = 0;
                    }
                    self.status = format!("searching · {}", q.unwrap_or_default());
                }
                KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                    if let Some(q) = self.files_search.as_mut() {
                        q.push(c);
                    }
                    let q = self.files_search.clone();
                    if let Some(s) = self.slot_mut() {
                        s.files.search_query = q.clone();
                        s.files.selected = 0;
                    }
                    self.status = format!("searching · {}", q.unwrap_or_default());
                }
                _ => {}
            }
            return Ok(());
        }
        if key.code == KeyCode::F(11) {
            let next_mode = if let Some(s) = self.slot_mut() {
                if s.fullscreen_app.is_some() {
                    s.fullscreen_app = None;
                    Some("disabled")
                } else {
                    let focused = s.desktop.focused_app();
                    s.fullscreen_app = Some(focused);
                    Some("enabled")
                }
            } else {
                None
            };
            if let Some(mode) = next_mode {
                self.status = format!("pane full-screen {mode} · press F11 to toggle back");
            }
            return Ok(());
        }
        if ctrl && key.code == KeyCode::Char('f') {
            self.full_screen = !self.full_screen;
            let mode = if self.full_screen {
                "decorations hidden"
            } else {
                "decorations visible"
            };
            self.status = format!("full-screen mode {mode} · press Ctrl+F to toggle");
            return Ok(());
        }
        if self.show_session_switcher {
            match key.code {
                KeyCode::Up | KeyCode::Char('k') => self.session_prev(),
                KeyCode::Down | KeyCode::Char('j') => self.session_next(),
                KeyCode::Char(n) if n.is_ascii_digit() => {
                    let idx = n.to_digit(10).unwrap_or(0) as usize;
                    let idx = if idx == 0 { 9 } else { idx - 1 };
                    if idx < self.sessions.len() {
                        self.active_idx = idx;
                        let name = self.sessions[idx].host_name.clone();
                        self.status =
                            format!("session {}/{} · {}", idx + 1, self.sessions.len(), name);
                    }
                }
                KeyCode::Enter | KeyCode::Esc | KeyCode::F(8) => self.show_session_switcher = false,
                _ => {}
            }
            return Ok(());
        }
        let focused = self
            .desktop()
            .map(|d| d.focused_app())
            .unwrap_or(AppKind::Terminal);

        // Esc: cancel OS drop → cancel drag → clear marks → close viewer → launcher
        if key.code == KeyCode::Esc && key.modifiers.is_empty() {
            if self.os_drop.is_some() {
                self.os_drop = None;
                self.status = "OS drop cancelled".into();
                return Ok(());
            }
            if self.drag.is_some() {
                self.drag = None;
                self.drop_target = None;
                self.mouse_press = None;
                self.status = "drag cancelled".into();
                return Ok(());
            }
            if focused == AppKind::Files
                && self
                    .slot()
                    .map(|s| !s.files.marked.is_empty())
                    .unwrap_or(false)
            {
                if let Some(s) = self.slot_mut() {
                    s.files.clear_marks();
                }
                self.status = "selection cleared".into();
                return Ok(());
            }
            if focused == AppKind::Editor
                && self.slot().map(|s| s.editor.is_open()).unwrap_or(false)
            {
                if self
                    .slot()
                    .map(|s| s.editor.dirty && !s.editor.discard_armed)
                    .unwrap_or(false)
                {
                    if let Some(s) = self.slot_mut() {
                        s.editor.discard_armed = true;
                    }
                    self.status = "unsaved changes · Ctrl+S save · Esc again discards".into();
                    return Ok(());
                }
                if let Some(s) = self.slot_mut() {
                    s.editor.clear();
                    if let Some((id, _)) = s
                        .desktop
                        .tree
                        .leaves()
                        .into_iter()
                        .find(|(_, app)| *app == AppKind::Files)
                    {
                        s.desktop.tree.set_focus(id);
                    }
                }
                self.status = "editor closed".into();
                return Ok(());
            }
            if focused == AppKind::Viewer
                && self.slot().map(|s| s.viewer.is_open()).unwrap_or(false)
            {
                if let Some(s) = self.slot_mut() {
                    s.viewer.clear();
                    if let Some((id, _)) = s
                        .desktop
                        .tree
                        .leaves()
                        .into_iter()
                        .find(|(_, app)| *app == AppKind::Files)
                    {
                        s.desktop.tree.set_focus(id);
                    }
                }
                self.status = "viewer closed".into();
                return Ok(());
            }
            // Esc from desktop → close current session or return to launcher
            self.close_current_session().await?;
            return Ok(());
        }

        // Layout / dock keys — short borrow so status can update freely.
        let (layout_status, refresh_procs) = {
            let Some(desktop) = self.desktop_mut() else {
                return Ok(());
            };
            match (key.modifiers, key.code) {
                // Pane cycle — always available (including while Shell has focus).
                (KeyModifiers::CONTROL, KeyCode::Char(' ')) if !shift => {
                    desktop.focus_next();
                    return Ok(());
                }
                (mods, KeyCode::Char(' '))
                    if mods.contains(KeyModifiers::CONTROL)
                        && mods.contains(KeyModifiers::SHIFT) =>
                {
                    desktop.focus_prev();
                    return Ok(());
                }
                // Tab cycles panes only outside Shell so bash can use Tab to complete.
                (_, KeyCode::Tab) if focused != AppKind::Terminal => {
                    desktop.focus_next();
                    return Ok(());
                }
                (KeyModifiers::SHIFT, KeyCode::BackTab) if focused != AppKind::Terminal => {
                    desktop.focus_prev();
                    return Ok(());
                }
                (_, KeyCode::F(2)) => {
                    let opened = desktop.tree.focus_or_open(AppKind::Files);
                    (
                        Some(if opened {
                            "opened files · right of focus".into()
                        } else {
                            "files".into()
                        }),
                        false,
                    )
                }
                (_, KeyCode::F(3)) => {
                    let opened = desktop.tree.focus_or_open(AppKind::Terminal);
                    (
                        Some(if opened {
                            "opened shell · right of focus".into()
                        } else {
                            "shell".into()
                        }),
                        false,
                    )
                }
                (_, KeyCode::F(4)) => {
                    let opened = desktop.tree.focus_or_open(AppKind::Processes);
                    (
                        Some(if opened {
                            "opened processes · right of focus".into()
                        } else {
                            "processes".into()
                        }),
                        true,
                    )
                }
                (_, KeyCode::F(5)) => {
                    let opened = desktop.tree.focus_or_open(AppKind::Transfers);
                    (
                        Some(if opened {
                            "opened transfers · right of focus".into()
                        } else {
                            "transfers".into()
                        }),
                        false,
                    )
                }
                (_, KeyCode::F(6)) => {
                    let opened = desktop.tree.focus_or_open(AppKind::Viewer);
                    (
                        Some(if opened {
                            "opened viewer · right of focus".into()
                        } else {
                            "viewer".into()
                        }),
                        false,
                    )
                }
                (_, KeyCode::F(7)) => {
                    let opened = desktop.tree.focus_or_open(AppKind::Editor);
                    (
                        Some(if opened {
                            "opened editor · right of focus".into()
                        } else {
                            "editor".into()
                        }),
                        false,
                    )
                }
                (mods, KeyCode::Char(c))
                    if mods.contains(KeyModifiers::CONTROL)
                        && matches!(c, 'w' | 'W')
                        && !mods.contains(KeyModifiers::ALT) =>
                {
                    // Ctrl+W closes the focused pane. (Ctrl+Shift+W is often
                    // swallowed by the host terminal; plain Ctrl+W is reliable.)
                    let app = desktop.focused_app();
                    (
                        Some(match desktop.tree.close_focused() {
                            Ok(_) => format!("closed {} · sibling expanded", app.label()),
                            Err(ssh_wm::ClosePaneError::LastPane) => {
                                "cannot close the last pane".into()
                            }
                            Err(ssh_wm::ClosePaneError::NotFound) => "pane not found".into(),
                        }),
                        false,
                    )
                }
                (_, KeyCode::F(10)) => {
                    let app = desktop.focused_app();
                    (
                        Some(match desktop.tree.close_focused() {
                            Ok(_) => format!("closed {} · sibling expanded", app.label()),
                            Err(ssh_wm::ClosePaneError::LastPane) => {
                                "cannot close the last pane".into()
                            }
                            Err(ssh_wm::ClosePaneError::NotFound) => "pane not found".into(),
                        }),
                        false,
                    )
                }
                (KeyModifiers::CONTROL, KeyCode::Char('h')) => {
                    let _ = desktop
                        .tree
                        .split_focused(Direction::Vertical, 0.5, AppKind::Files);
                    (Some("split · files to the right".into()), false)
                }
                (KeyModifiers::CONTROL, KeyCode::Char('v')) => {
                    let _ =
                        desktop
                            .tree
                            .split_focused(Direction::Horizontal, 0.5, AppKind::Terminal);
                    (Some("split · shell below".into()), false)
                }
                _ => (None, false),
            }
        };
        if let Some(msg) = layout_status {
            self.status = msg;
            if refresh_procs {
                let _ = self.refresh_processes().await;
            }
            return Ok(());
        }

        let Some(desktop) = self.desktop_mut() else {
            return Ok(());
        };

        let focused = desktop.focused_app();
        match focused {
            AppKind::Terminal => {
                if let Some(host_id) = self.active_host_id().map(str::to_owned) {
                    if self.hub.is_connected(&host_id).await {
                        let bytes = key_to_bytes(key);
                        if !bytes.is_empty() {
                            let _ = self.hub.write_pty(&host_id, &bytes).await;
                        }
                        return Ok(());
                    }
                }
                match key.code {
                    KeyCode::Char(c) => {
                        if let Some(s) = self.slot_mut() {
                            s.term.write_str(&c.to_string());
                        }
                    }
                    KeyCode::Enter => {
                        if let Some(s) = self.slot_mut() {
                            s.term.write_str("\r\n");
                        }
                    }
                    KeyCode::Backspace => {
                        if let Some(s) = self.slot_mut() {
                            s.term.process(&[0x08, b' ', 0x08]);
                        }
                    }
                    _ => {}
                }
            }
            AppKind::Files => self.handle_files_key(key).await?,
            AppKind::Viewer => self.handle_viewer_key(key).await?,
            AppKind::Transfers => self.handle_transfers_key(key).await?,
            AppKind::Processes => self.handle_processes_key(key).await?,
            AppKind::Editor => self.handle_editor_key(key).await?,
            AppKind::Launcher => {}
        }
        Ok(())
    }

    async fn handle_files_key(&mut self, key: KeyEvent) -> Result<()> {
        match (key.modifiers, key.code) {
            (KeyModifiers::CONTROL, KeyCode::Char('u')) => {
                self.begin_upload_prompt();
                return Ok(());
            }
            (KeyModifiers::CONTROL, KeyCode::Char('d')) => {
                self.begin_download_prompt();
                return Ok(());
            }
            (KeyModifiers::CONTROL, KeyCode::Char('c')) => {
                self.clipboard_copy_remote();
                return Ok(());
            }
            (KeyModifiers::CONTROL, KeyCode::Char('x')) => {
                self.clipboard_cut_remote();
                return Ok(());
            }
            (KeyModifiers::CONTROL, KeyCode::Char('v')) => {
                self.clipboard_paste_into_remote().await?;
                return Ok(());
            }
            (mods, KeyCode::Char('v'))
                if mods.contains(KeyModifiers::CONTROL) && mods.contains(KeyModifiers::SHIFT) =>
            {
                self.clipboard_paste_to_local().await?;
                return Ok(());
            }
            (KeyModifiers::CONTROL, KeyCode::Char('l')) => {
                self.path_prompt = Some(PathPrompt::copy_local());
                self.status = "copy local · pick a file for the clipboard".into();
                return Ok(());
            }
            _ => {}
        }
        match key.code {
            KeyCode::Up | KeyCode::Char('k') => {
                if let Some(s) = self.slot_mut() {
                    s.files.move_up();
                }
            }
            KeyCode::Down | KeyCode::Char('j') => {
                if let Some(s) = self.slot_mut() {
                    s.files.move_down();
                }
            }
            KeyCode::Char(' ') => {
                if let Some(s) = self.slot_mut() {
                    s.files.toggle_mark_selected();
                    s.files.move_down();
                }
            }
            KeyCode::Enter | KeyCode::Right => {
                self.open_selected_file().await?;
            }
            // Keep `l` for open; local clipboard uses Ctrl+L
            KeyCode::Char('l') => {
                self.open_selected_file().await?;
            }
            KeyCode::Backspace | KeyCode::Left | KeyCode::Char('h') => {
                let (cwd, online) = self
                    .slot()
                    .map(|s| (s.files.cwd.clone(), s.files.online))
                    .unwrap_or_default();
                if cwd != PathBuf::from("/") {
                    let parent = join_remote(&cwd, "..");
                    if online {
                        let _ = self.load_dir(parent).await;
                    } else {
                        if let Some(s) = self.slot_mut() {
                            s.files = FilesState::demo();
                        }
                    }
                }
            }
            KeyCode::Char('r') => {
                let (online, cwd) = self
                    .slot()
                    .map(|s| (s.files.online, s.files.cwd.clone()))
                    .unwrap_or((false, PathBuf::new()));
                if online {
                    let _ = self.load_dir(cwd).await;
                } else {
                    self.status = "files · offline demo (connect for SFTP refresh)".into();
                }
            }
            KeyCode::Char('a') | KeyCode::Char('n') => {
                if self.slot().map(|s| s.files.online).unwrap_or(false) {
                    self.files_prompt = Some(FilesPrompt::mkdir());
                    self.status = "new folder · type name · Enter create · Esc".into();
                } else {
                    self.status = "mkdir needs a live SFTP session".into();
                }
            }
            KeyCode::Char('R') => {
                self.begin_rename_prompt();
            }
            KeyCode::Char('d') | KeyCode::Delete => {
                self.begin_delete_prompt();
            }
            KeyCode::Char('e') => {
                let (row, cwd, entries) = self
                    .slot()
                    .map(|s| {
                        (
                            s.files.selected_row(),
                            s.files.cwd.clone(),
                            s.files.entries.clone(),
                        )
                    })
                    .unwrap_or_default();
                if let Some(row) = row {
                    if let Some((path, is_dir)) = resolve_open_path(&cwd, row, &entries) {
                        if !is_dir {
                            self.open_path(path, true).await?;
                        }
                    }
                }
            }
            KeyCode::Home => {
                if let Some(s) = self.slot_mut() {
                    s.files.selected = 0;
                }
            }
            KeyCode::End => {
                let len = self.slot().map(|s| s.files.rows().len()).unwrap_or(0);
                if len > 0 {
                    if let Some(s) = self.slot_mut() {
                        s.files.selected = len - 1;
                    }
                }
            }
            KeyCode::Char('/') => {
                self.files_search = Some(String::new());
                self.status = "searching · type search query...".into();
            }
            _ => {}
        }
        Ok(())
    }

    fn clipboard_copy_remote(&mut self) {
        let Some(host_id) = self.active_host_id().map(str::to_owned) else {
            self.status = "no session".into();
            return;
        };
        let targets = self
            .slot()
            .map(|s| s.files.clipboard_targets())
            .unwrap_or_default();
        if targets.is_empty() {
            self.status = "nothing to copy".into();
            return;
        }
        let files: Vec<FileEntry> = targets
            .iter()
            .map(|e| FileEntry::remote(&host_id, e.path.clone(), e.is_dir))
            .collect();
        let n = files.len();
        self.clipboard.set_files(files, FileOp::Copy);
        self.status = format!("copied {n} item(s) · Ctrl+V paste here · Ctrl+Shift+V to local");
    }

    fn clipboard_cut_remote(&mut self) {
        let Some(host_id) = self.active_host_id().map(str::to_owned) else {
            self.status = "no session".into();
            return;
        };
        let targets = self
            .slot()
            .map(|s| s.files.clipboard_targets())
            .unwrap_or_default();
        if targets.is_empty() {
            self.status = "nothing to cut".into();
            return;
        }
        let files: Vec<FileEntry> = targets
            .iter()
            .map(|e| FileEntry::remote(&host_id, e.path.clone(), e.is_dir))
            .collect();
        let n = files.len();
        self.clipboard.set_files(files, FileOp::Cut);
        self.status = format!("cut {n} item(s) · navigate and Ctrl+V to move");
    }

    async fn clipboard_paste_into_remote(&mut self) -> Result<()> {
        let dest = self.slot().map(|s| s.files.cwd.clone()).unwrap_or_default();
        self.paste_clipboard_into(dest, false).await
    }

    async fn paste_clipboard_into(&mut self, dest_dir: PathBuf, force_move: bool) -> Result<()> {
        if !self.slot().map(|s| s.files.online).unwrap_or(false) {
            self.status = "paste needs a live SFTP session".into();
            return Ok(());
        }
        let Some(_host_id) = self.active_host_id().map(str::to_owned) else {
            self.status = "no session".into();
            return Ok(());
        };
        let mut op = self.clipboard.file_op().unwrap_or(FileOp::Copy);
        if force_move {
            op = FileOp::Cut;
        }
        let entries = self.clipboard.files().to_vec();
        if entries.is_empty() {
            self.status = "clipboard empty · Ctrl+C / Ctrl+L first".into();
            return Ok(());
        }

        // Check if any matching file exists in target remote directory listing
        let mut existing_clashes = Vec::new();
        if let Some(s) = self.slot() {
            for entry in &entries {
                let name = match &entry.location {
                    FileLocation::Local { path } | FileLocation::Remote { path, .. } => path
                        .file_name()
                        .map(|s| s.to_string_lossy().into_owned())
                        .unwrap_or_else(|| "file".into()),
                };
                if s.files.entries.iter().any(|e| e.name == name && !e.is_dir) {
                    existing_clashes.push(name);
                }
            }
        }

        if !existing_clashes.is_empty() {
            self.overwrite_prompt = Some(OverwritePrompt {
                title: "Confirm Overwrite".into(),
                files: existing_clashes,
                selected: 1, // default to 'No' for safety
                dest_dir,
                entries,
                op,
            });
            self.status = "warning: matching files exist, confirm overwrite".into();
            return Ok(());
        }

        self.execute_paste_clipboard_into(dest_dir, entries, op)
            .await
    }

    async fn execute_paste_clipboard_into(
        &mut self,
        dest_dir: PathBuf,
        entries: Vec<FileEntry>,
        op: FileOp,
    ) -> Result<()> {
        let Some(host_id) = self.active_host_id().map(str::to_owned) else {
            return Ok(());
        };
        let mut queued = 0usize;
        let mut moved = 0usize;
        let mut skipped = 0usize;

        let mut errors = Vec::new();

        for entry in &entries {
            let name = match &entry.location {
                FileLocation::Local { path } | FileLocation::Remote { path, .. } => path
                    .file_name()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_else(|| "file".into()),
            };
            let dest = dest_dir.join(&name);

            match (&entry.location, op, entry.is_dir) {
                (FileLocation::Remote { host_id: src, path }, FileOp::Cut, _)
                    if src == &host_id =>
                {
                    if path == &dest {
                        skipped += 1;
                        continue;
                    }
                    match self.hub.remote_rename(&host_id, path, &dest).await {
                        Ok(()) => moved += 1,
                        Err(e) => {
                            errors.push(format!("move {name} failed: {e}"));
                            skipped += 1;
                        }
                    }
                }
                (FileLocation::Remote { host_id: src, path }, FileOp::Copy, _)
                    if src == &host_id =>
                {
                    match self
                        .hub
                        .enqueue_remote_copy(&host_id, path.clone(), dest)
                        .await
                    {
                        Ok(_) => queued += 1,
                        Err(e) => {
                            errors.push(format!("copy {name} failed: {e}"));
                            skipped += 1;
                        }
                    }
                }
                (FileLocation::Local { path }, FileOp::Copy | FileOp::Cut, _) => {
                    let cut = op == FileOp::Cut;
                    match self
                        .hub
                        .enqueue_upload_ex(&host_id, path.clone(), dest_dir.clone(), cut)
                        .await
                    {
                        Ok(_) => queued += 1,
                        Err(e) => {
                            errors.push(format!("upload {name} failed: {e}"));
                            skipped += 1;
                        }
                    }
                }
                (FileLocation::Remote { host_id: src, .. }, _, _) if src != &host_id => {
                    skipped += 1;
                    errors.push("cross-host paste not supported yet".into());
                }
                _ => skipped += 1,
            }
        }

        if op == FileOp::Cut && errors.is_empty() {
            self.clipboard.clear_files();
        }

        if let Some(s) = self.slot_mut() {
            s.files.clear_marks();
            s.pending_files_refresh = true;
        }

        if errors.is_empty() {
            self.status = format!(
                "paste · {moved} moved · {queued} queued · {skipped} skipped → {}",
                dest_dir.display()
            );
        } else {
            for e in &errors {
                self.diagnostics
                    .push(DiagLevel::Error, format!("paste · {e}"));
            }
            self.status = format!(
                "paste incomplete · {} errors, {} done, {} queued, {} skipped · F9 log",
                errors.len(),
                moved,
                queued,
                skipped
            );
        }
        Ok(())
    }

    async fn clipboard_paste_to_local(&mut self) -> Result<()> {
        let Some(host_id) = self.active_host_id().map(str::to_owned) else {
            self.status = "no session".into();
            return Ok(());
        };
        let op = self.clipboard.file_op().unwrap_or(FileOp::Copy);
        let entries = self.clipboard.files().to_vec();
        if entries.is_empty() {
            // Fall back: download current selection via prompt
            self.begin_download_prompt();
            return Ok(());
        }

        let dest_dir = dirs::download_dir()
            .or_else(dirs::home_dir)
            .unwrap_or_else(|| PathBuf::from("."));

        let mut queued = 0usize;
        for entry in entries {
            match entry.location {
                FileLocation::Remote { host_id: src, path } if src == host_id => {
                    let name = path
                        .file_name()
                        .map(|s| s.to_string_lossy().into_owned())
                        .unwrap_or_else(|| "download.bin".into());
                    let local = dest_dir.join(name);
                    let cut = op == FileOp::Cut;
                    match self
                        .hub
                        .enqueue_download_ex(&host_id, path, local, None, cut)
                        .await
                    {
                        Ok(_) => queued += 1,
                        Err(e) => self.status = format!("download failed · {e}"),
                    }
                }
                FileLocation::Local { .. } => {
                    self.status = "clipboard already local · nothing to paste to disk".into();
                }
                _ => {
                    self.status = "skip other hosts for paste-to-local".into();
                }
            }
        }
        if op == FileOp::Cut {
            self.clipboard.clear_files();
        }
        if queued > 0 {
            self.status = format!("queued {queued} download(s) → {}", dest_dir.display());
            if let Some(desktop) = self.desktop_mut() {
                if let Some((tid, _)) = desktop
                    .tree
                    .leaves()
                    .into_iter()
                    .find(|(_, a)| *a == AppKind::Transfers)
                {
                    desktop.tree.set_focus(tid);
                }
            }
        }
        Ok(())
    }

    fn begin_upload_prompt(&mut self) {
        let (online, cwd) = self
            .slot()
            .map(|s| (s.files.online, s.files.cwd.clone()))
            .unwrap_or_default();
        if !online {
            self.status = "upload needs a live SFTP session".into();
            return;
        }
        self.path_prompt = Some(PathPrompt::upload(cwd));
        self.status = "upload · pick a local file (Enter) or type path".into();
    }

    fn begin_download_prompt(&mut self) {
        let (online, row, cwd, entries) = self
            .slot()
            .map(|s| {
                (
                    s.files.online,
                    s.files.selected_row(),
                    s.files.cwd.clone(),
                    s.files.entries.clone(),
                )
            })
            .unwrap_or_default();
        if !online {
            self.status = "download needs a live SFTP session".into();
            return;
        }
        let Some(row) = row else {
            return;
        };
        let Some((path, _is_dir)) = resolve_open_path(&cwd, row, &entries) else {
            return;
        };
        let size = entries.iter().find(|e| e.path == path).and_then(|e| e.size);
        self.path_prompt = Some(PathPrompt::download(path, size));
        self.status = "download · confirm local path and press Enter (dirs supported)".into();
    }

    async fn handle_path_prompt_key(&mut self, key: KeyEvent) -> Result<()> {
        let Some(prompt) = self.path_prompt.as_mut() else {
            return Ok(());
        };

        match (key.modifiers, key.code) {
            (_, KeyCode::Esc) => {
                self.path_prompt = None;
                self.status = "transfer cancelled".into();
                return Ok(());
            }
            (KeyModifiers::CONTROL, KeyCode::Char('e')) => {
                prompt.editing = !prompt.editing;
                return Ok(());
            }
            _ => {}
        }

        if prompt.editing {
            match key.code {
                KeyCode::Char(c) => prompt.buffer.push(c),
                KeyCode::Backspace => {
                    prompt.buffer.pop();
                }
                KeyCode::Enter => {
                    let path = prompt.resolved_path();
                    let kind = prompt.kind;
                    let remote = prompt.remote.clone();
                    let remote_size = prompt.remote_size;
                    self.path_prompt = None;
                    self.submit_path_prompt(kind, path, remote, remote_size)
                        .await?;
                }
                KeyCode::Tab => prompt.editing = false,
                _ => {}
            }
            return Ok(());
        }

        // Space / Ctrl+Enter: select file or folder for upload/copy (Enter still navigates dirs).
        let select_entry = matches!(
            (key.modifiers, key.code),
            (KeyModifiers::NONE, KeyCode::Char(' '))
                | (KeyModifiers::CONTROL, KeyCode::Enter)
                | (KeyModifiers::CONTROL, KeyCode::Char('\n'))
        );
        if select_entry
            && matches!(
                prompt.kind,
                PathPromptKind::Upload | PathPromptKind::CopyLocal
            )
        {
            if let Some(path) = prompt.select_selected() {
                let kind = prompt.kind;
                let remote = prompt.remote.clone();
                let remote_size = prompt.remote_size;
                self.path_prompt = None;
                self.submit_path_prompt(kind, path, remote, remote_size)
                    .await?;
            } else {
                prompt.error = Some("select a file or folder (not ..)".into());
            }
            return Ok(());
        }

        match key.code {
            KeyCode::Up | KeyCode::Char('k') => prompt.move_up(),
            KeyCode::Down | KeyCode::Char('j') => prompt.move_down(),
            KeyCode::Enter => {
                if let Some(path) = prompt.activate_selected() {
                    let kind = prompt.kind;
                    let remote = prompt.remote.clone();
                    let remote_size = prompt.remote_size;
                    self.path_prompt = None;
                    self.submit_path_prompt(kind, path, remote, remote_size)
                        .await?;
                }
            }
            KeyCode::Char('s') if prompt.kind == PathPromptKind::Download => {
                let path = prompt.download_into_cwd();
                let remote = prompt.remote.clone();
                let remote_size = prompt.remote_size;
                self.path_prompt = None;
                self.submit_path_prompt(PathPromptKind::Download, path, remote, remote_size)
                    .await?;
            }
            KeyCode::Char('e') | KeyCode::Tab => prompt.editing = true,
            KeyCode::Backspace => {
                if let Some(parent) = prompt.browse_cwd.parent() {
                    prompt.browse_cwd = parent.to_path_buf();
                    prompt.refresh_listing();
                }
            }
            _ => {}
        }
        Ok(())
    }

    async fn submit_path_prompt(
        &mut self,
        kind: PathPromptKind,
        local: PathBuf,
        remote: PathBuf,
        remote_size: Option<u64>,
    ) -> Result<()> {
        match kind {
            PathPromptKind::CopyLocal => {
                if !(local.is_file() || local.is_dir()) {
                    self.status = format!("not found: {}", local.display());
                    return Ok(());
                }
                let is_dir = local.is_dir();
                self.clipboard
                    .set_files(vec![FileEntry::local(local.clone(), is_dir)], FileOp::Copy);
                self.status = format!(
                    "copied local {} · Ctrl+V to upload into remote cwd",
                    local.display()
                );
                return Ok(());
            }
            _ => {}
        }

        let Some(host_id) = self.active_host_id().map(str::to_owned) else {
            self.status = "no active session".into();
            return Ok(());
        };
        match kind {
            PathPromptKind::Upload => {
                match self.hub.enqueue_upload(&host_id, local, remote).await {
                    Ok(id) => {
                        self.status = format!("queued upload · {}", id.0);
                        if let Some(desktop) = self.desktop_mut() {
                            if let Some((tid, _)) = desktop
                                .tree
                                .leaves()
                                .into_iter()
                                .find(|(_, a)| *a == AppKind::Transfers)
                            {
                                desktop.tree.set_focus(tid);
                            }
                        }
                    }
                    Err(e) => self.status = format!("upload failed to queue · {e}"),
                }
            }
            PathPromptKind::Download => {
                let dest = if local.is_dir() {
                    let name = remote
                        .file_name()
                        .map(|s| s.to_string_lossy().into_owned())
                        .unwrap_or_else(|| "download.bin".into());
                    local.join(name)
                } else {
                    local
                };
                match self
                    .hub
                    .enqueue_download(&host_id, remote, dest, remote_size)
                    .await
                {
                    Ok(id) => {
                        self.status = format!("queued download · {}", id.0);
                        if let Some(desktop) = self.desktop_mut() {
                            if let Some((tid, _)) = desktop
                                .tree
                                .leaves()
                                .into_iter()
                                .find(|(_, a)| *a == AppKind::Transfers)
                            {
                                desktop.tree.set_focus(tid);
                            }
                        }
                    }
                    Err(e) => self.status = format!("download failed to queue · {e}"),
                }
            }
            PathPromptKind::CopyLocal => unreachable!(),
        }
        Ok(())
    }

    async fn handle_transfers_key(&mut self, key: KeyEvent) -> Result<()> {
        match key.code {
            KeyCode::Up | KeyCode::Char('k') => {
                if let Some(s) = self.slot_mut() {
                    s.transfers.move_up();
                }
            }
            KeyCode::Down | KeyCode::Char('j') => {
                if let Some(s) = self.slot_mut() {
                    s.transfers.move_down();
                }
            }
            KeyCode::Char('c') => {
                if let Some(id) = self.slot().and_then(|s| s.transfers.selected_id()) {
                    if self.hub.cancel_transfer(id).await {
                        self.status = "transfer cancel requested".into();
                    }
                }
            }
            KeyCode::Char('r') => {
                if let Some(id) = self.slot().and_then(|s| s.transfers.selected_id()) {
                    match self.hub.retry_transfer(id).await {
                        Ok(new_id) => self.status = format!("retry queued · {}", new_id.0),
                        Err(e) => self.status = format!("retry failed · {e}"),
                    }
                }
            }
            KeyCode::Char('u') => {
                self.begin_upload_prompt();
            }
            KeyCode::Char('d') => {
                self.begin_download_prompt();
            }
            _ => {
                self.status =
                    "transfers · j/k select · c cancel · r retry · u upload · d download".into();
            }
        }
        Ok(())
    }

    async fn handle_viewer_key(&mut self, key: KeyEvent) -> Result<()> {
        match key.code {
            KeyCode::Up | KeyCode::Char('k') => {
                if let Some(s) = self.slot_mut() {
                    s.viewer.scroll_by(-1);
                }
            }
            KeyCode::Down | KeyCode::Char('j') => {
                if let Some(s) = self.slot_mut() {
                    s.viewer.scroll_by(1);
                }
            }
            KeyCode::PageUp => {
                if let Some(s) = self.slot_mut() {
                    s.viewer.scroll_by(-10);
                }
            }
            KeyCode::PageDown => {
                if let Some(s) = self.slot_mut() {
                    s.viewer.scroll_by(10);
                }
            }
            KeyCode::Home => {
                if let Some(s) = self.slot_mut() {
                    s.viewer.scroll = 0;
                }
            }
            KeyCode::Char('e') => {
                let (path, binary) = self
                    .slot()
                    .map(|s| (s.viewer.path.clone(), s.viewer.binary))
                    .unwrap_or_default();
                if let Some(path) = path {
                    if !binary {
                        self.open_path(path, true).await?;
                    } else {
                        self.status = "cannot edit binary/hex view".into();
                    }
                }
            }
            KeyCode::Char('q') => {
                if let Some(s) = self.slot_mut() {
                    s.viewer.clear();
                }
                self.status = "viewer closed".into();
            }
            _ => {}
        }
        Ok(())
    }

    async fn handle_editor_key(&mut self, key: KeyEvent) -> Result<()> {
        match (key.modifiers, key.code) {
            (KeyModifiers::CONTROL, KeyCode::Char('s')) => {
                self.save_editor().await?;
                return Ok(());
            }
            _ => {}
        }
        match key.code {
            KeyCode::Left => {
                if let Some(s) = self.slot_mut() {
                    s.editor.move_left();
                }
            }
            KeyCode::Right => {
                if let Some(s) = self.slot_mut() {
                    s.editor.move_right();
                }
            }
            KeyCode::Up => {
                if let Some(s) = self.slot_mut() {
                    s.editor.move_up();
                }
            }
            KeyCode::Down => {
                if let Some(s) = self.slot_mut() {
                    s.editor.move_down();
                }
            }
            KeyCode::Home => {
                if let Some(s) = self.slot_mut() {
                    s.editor.cursor_col = 0;
                }
            }
            KeyCode::End => {
                if let Some(s) = self.slot_mut() {
                    s.editor.cursor_col = s.editor.lines[s.editor.cursor_row].chars().count();
                }
            }
            KeyCode::Enter => {
                if let Some(s) = self.slot_mut() {
                    s.editor.insert_newline();
                }
            }
            KeyCode::Backspace => {
                if let Some(s) = self.slot_mut() {
                    s.editor.backspace();
                }
            }
            KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                if let Some(s) = self.slot_mut() {
                    s.editor.insert_char(c);
                }
            }
            _ => {}
        }
        let height = self
            .last_geo
            .as_ref()
            .and_then(|g| g.files_pane_inner)
            .map(|r| r.height.saturating_sub(2).max(4))
            .unwrap_or(20);
        if let Some(s) = self.slot_mut() {
            s.editor.ensure_visible(height);
        }
        Ok(())
    }

    async fn handle_processes_key(&mut self, key: KeyEvent) -> Result<()> {
        match key.code {
            KeyCode::Up | KeyCode::Char('k') => {
                if let Some(s) = self.slot_mut() {
                    s.processes.move_up();
                }
            }
            KeyCode::Down | KeyCode::Char('j') => {
                if let Some(s) = self.slot_mut() {
                    s.processes.move_down();
                }
            }
            KeyCode::Char('r') => {
                self.refresh_processes().await?;
            }
            _ => {
                self.status = "processes · j/k select · r refresh".into();
            }
        }
        Ok(())
    }

    fn handle_mouse(&mut self, mouse: MouseEvent) {
        // Mouse handling is sync; drop completion is polled via flags set here.
        // Actual paste runs from event_loop when `pending_drop` is set.
        let _ = self.handle_mouse_inner(mouse);
    }

    fn handle_mouse_inner(&mut self, mouse: MouseEvent) -> Result<()> {
        if self.sessions.is_empty() || self.last_geo.is_none() {
            return Ok(());
        }
        let pos = (mouse.column, mouse.row);
        let mods = mouse.modifiers;
        let last_geo = self.last_geo.take();
        let (pane_id, files_row) = {
            let geo = last_geo.as_ref().unwrap();
            let pane_id = geo.pane_at(pos.0, pos.1).map(|p| p.id);
            let files_row = geo.files_row_at(pos.0, pos.1).cloned();
            (pane_id, files_row)
        };
        self.last_geo = last_geo;

        match mouse.kind {
            MouseEventKind::Down(event::MouseButton::Left) => {
                if let Some(id) = pane_id {
                    if let Some(desktop) = self.desktop_mut() {
                        desktop.tree.set_focus(id);
                    }
                }

                if let Some(row) = files_row {
                    if let Some(entry_idx) = row.entry_index {
                        let already_marked = self
                            .slot()
                            .map(|s| s.files.is_marked(entry_idx))
                            .unwrap_or(false);
                        let marked = self
                            .slot()
                            .map(|s| s.files.marked.clone())
                            .unwrap_or_default();
                        if let Some(s) = self.slot_mut() {
                            s.files.selected = row.row_index;
                        }
                        let indices = if already_marked {
                            marked
                        } else {
                            vec![entry_idx]
                        };
                        self.mouse_press = Some(MousePress {
                            origin: pos,
                            entry_indices: indices,
                        });
                    } else {
                        if let Some(s) = self.slot_mut() {
                            s.files.selected = row.row_index;
                        }
                        self.mouse_press = Some(MousePress {
                            origin: pos,
                            entry_indices: Vec::new(),
                        });
                    }
                } else {
                    self.mouse_press = None;
                }
            }
            MouseEventKind::Drag(event::MouseButton::Left) => {
                if let Some(press) = self.mouse_press.clone() {
                    let dx = pos.0.abs_diff(press.origin.0);
                    let dy = pos.1.abs_diff(press.origin.1);
                    if self.drag.is_none()
                        && (dx >= 1 || dy >= 1)
                        && !press.entry_indices.is_empty()
                    {
                        let Some(host_id) = self.active_host_id().map(str::to_owned) else {
                            return Ok(());
                        };
                        let slot_entries = self
                            .slot()
                            .map(|s| s.files.entries.clone())
                            .unwrap_or_default();
                        let files: Vec<FileEntry> = press
                            .entry_indices
                            .iter()
                            .filter_map(|i| slot_entries.get(*i))
                            .map(|e| FileEntry::remote(&host_id, e.path.clone(), e.is_dir))
                            .collect();
                        if files.is_empty() {
                            return Ok(());
                        }
                        let n = files.len();
                        self.drag =
                            Some(DragSession::start(DragPayload::Files(files), press.origin));
                        self.status =
                            format!("dragging {n} · drop on folder (Shift=move) · Esc cancel");
                    }
                }
                if let Some(drag) = self.drag.as_mut() {
                    drag.move_to(pos);
                }
                let cwd = self.slot().map(|s| s.files.cwd.clone()).unwrap_or_default();
                if let Some(geo) = self.last_geo.as_ref() {
                    self.drop_target = Some(geo.drop_target_at(pos.0, pos.1, &cwd));
                    if let Some(t) = &self.drop_target {
                        let move_hint = if mods.contains(KeyModifiers::SHIFT) {
                            "move"
                        } else {
                            "copy"
                        };
                        self.status = format!("{} · {move_hint}", t.describe());
                    }
                }
            }
            MouseEventKind::Up(event::MouseButton::Left) => {
                let force_move = mods.contains(KeyModifiers::SHIFT);
                if let Some(drag) = self.drag.take() {
                    let target = self.drop_target.take().unwrap_or(DropTarget::Ask);
                    self.mouse_press = None;
                    self.pending_drop = Some(PendingDrop {
                        payload: drag.payload,
                        target,
                        force_move,
                    });
                } else if let Some(press) = self.mouse_press.take() {
                    if press.entry_indices.is_empty() {
                        if self.slot().and_then(|s| s.files.selected_row()).as_ref()
                            == Some(&FilesRow::Parent)
                        {
                            self.pending_open_parent = true;
                        }
                    }
                }
                self.drop_target = None;
            }
            MouseEventKind::Moved => {
                if self.drag.is_some() {
                    if let Some(drag) = self.drag.as_mut() {
                        drag.move_to(pos);
                    }
                    let cwd2 = self.slot().map(|s| s.files.cwd.clone()).unwrap_or_default();
                    if let Some(geo) = self.last_geo.as_ref() {
                        self.drop_target = Some(geo.drop_target_at(pos.0, pos.1, &cwd2));
                    }
                }
            }
            _ => {}
        }
        Ok(())
    }

    async fn apply_pending_drop(&mut self) -> Result<()> {
        let Some(pending) = self.pending_drop.take() else {
            return Ok(());
        };

        match pending.payload {
            DragPayload::Files(files) => {
                if files.is_empty() {
                    return Ok(());
                }
                let dest = match &pending.target {
                    DropTarget::Folder { path, .. } => path.clone(),
                    DropTarget::TransferDock => {
                        self.slot().map(|s| s.files.cwd.clone()).unwrap_or_default()
                    }
                    DropTarget::Ask => {
                        self.status = "drop cancelled · no target".into();
                        return Ok(());
                    }
                };
                let op = if pending.force_move {
                    FileOp::Cut
                } else {
                    FileOp::Copy
                };
                self.clipboard.set_files(files, op);
                self.paste_clipboard_into(dest, pending.force_move).await?;
                if matches!(pending.target, DropTarget::TransferDock) {
                    if let Some(desktop) = self.desktop_mut() {
                        if let Some((tid, _)) = desktop
                            .tree
                            .leaves()
                            .into_iter()
                            .find(|(_, a)| *a == AppKind::Transfers)
                        {
                            desktop.tree.set_focus(tid);
                        }
                    }
                }
            }
            DragPayload::OsPaths(paths) => {
                let files = existing_files(&paths);
                if files.is_empty() {
                    self.status = "OS drop · no local files in payload".into();
                    return Ok(());
                }
                let dest = pending
                    .target
                    .remote_dir(&self.slot().map(|s| s.files.cwd.clone()).unwrap_or_default())
                    .unwrap_or_else(|| {
                        self.slot().map(|s| s.files.cwd.clone()).unwrap_or_default()
                    });
                if self.slot().map(|s| s.files.online).unwrap_or(false) {
                    self.os_drop = Some(OsDropOffer::new(files, dest));
                    self.status = "OS drop · confirm upload · Enter/y · Esc".into();
                } else {
                    self.offer_os_upload(files);
                }
            }
        }
        Ok(())
    }

    fn frame_model<'a>(&'a self) -> UiFrame<'a> {
        let slot = self.slot();
        let desktop = slot.map(|s| &s.desktop);
        let term: &TermEmulator = slot.map(|s| &s.term).unwrap_or(empty_term());
        let files: &FilesState = slot.map(|s| &s.files).unwrap_or(empty_files());
        let viewer: &ViewerState = slot.map(|s| &s.viewer).unwrap_or(empty_viewer());
        let editor: &EditorState = slot.map(|s| &s.editor).unwrap_or(empty_editor());
        let processes: &ProcessesState = slot.map(|s| &s.processes).unwrap_or(empty_procs());
        let transfers: &TransfersUi = slot.map(|s| &s.transfers).unwrap_or(empty_xfer());

        UiFrame {
            screen: match self.screen {
                Screen::Launcher => ui::ScreenKind::Launcher,
                Screen::Desktop => ui::ScreenKind::Desktop,
            },
            hosts: &self.hosts,
            selected_host: self.selected_host,
            sessions: self.sessions.iter().map(|s| s.host_name.as_str()).collect(),
            open_host_ids: self.sessions.iter().map(|s| s.host_id.as_str()).collect(),
            active_session_idx: self.active_idx,
            show_session_switcher: self.show_session_switcher,
            full_screen: self.full_screen,
            fullscreen_app: self.slot().and_then(|s| s.fullscreen_app),
            desktop,
            status: if let Some(_q) = &self.files_search {
                &self.status // keep custom search query prompt status
            } else {
                &self.status
            },
            term,
            clipboard_has_files: self.clipboard.has_files(),
            clipboard_label: match self.clipboard.file_op() {
                Some(ssh_os::FileOp::Copy) => "copy",
                Some(ssh_os::FileOp::Cut) => "cut",
                None => "",
            },
            files,
            viewer,
            editor,
            processes,
            transfers,
            path_prompt: self.path_prompt.as_ref(),
            host_form: self.host_form.as_ref(),
            vault_unlock: self.vault_unlock.as_ref(),
            connecting_host: self.connecting_host.as_deref(),
            connect_spinner: self.connect_spinner,
            drag: self.drag.as_ref(),
            drop_target: self.drop_target.as_ref(),
            os_drop: self.os_drop.as_ref(),
            overwrite_prompt: self.overwrite_prompt.as_ref(),
            files_prompt: self.files_prompt.as_ref(),
            diagnostics: &self.diagnostics,
        }
    }
}

// Empty-state references for the frame_model fallbacks (no active session).
fn empty_files() -> &'static FilesState {
    use std::sync::OnceLock;
    static CELL: OnceLock<FilesState> = OnceLock::new();
    CELL.get_or_init(FilesState::default)
}
fn empty_term() -> &'static TermEmulator {
    use std::sync::OnceLock;
    static CELL: OnceLock<TermEmulator> = OnceLock::new();
    CELL.get_or_init(TermEmulator::default)
}
fn empty_viewer() -> &'static ViewerState {
    use std::sync::OnceLock;
    static CELL: OnceLock<ViewerState> = OnceLock::new();
    CELL.get_or_init(ViewerState::default)
}
fn empty_editor() -> &'static EditorState {
    use std::sync::OnceLock;
    static CELL: OnceLock<EditorState> = OnceLock::new();
    CELL.get_or_init(EditorState::default)
}
fn empty_procs() -> &'static ProcessesState {
    use std::sync::OnceLock;
    static CELL: OnceLock<ProcessesState> = OnceLock::new();
    CELL.get_or_init(ProcessesState::default)
}
fn empty_xfer() -> &'static TransfersUi {
    use std::sync::OnceLock;
    static CELL: OnceLock<TransfersUi> = OnceLock::new();
    CELL.get_or_init(TransfersUi::default)
}

#[derive(Debug, Clone)]
struct PendingDrop {
    payload: DragPayload,
    target: DropTarget,
    force_move: bool,
}

fn key_to_bytes(key: KeyEvent) -> Vec<u8> {
    match key.code {
        KeyCode::Char(c) if key.modifiers.contains(KeyModifiers::CONTROL) => {
            let n = c.to_ascii_lowercase() as u8;
            if (b'a'..=b'z').contains(&n) {
                vec![n - b'a' + 1]
            } else {
                c.to_string().into_bytes()
            }
        }
        KeyCode::Char(c) => c.to_string().into_bytes(),
        KeyCode::Enter => vec![b'\r'],
        KeyCode::Backspace => vec![0x7f],
        KeyCode::Tab => vec![b'\t'],
        KeyCode::BackTab => b"\x1b[Z".to_vec(),
        KeyCode::Esc => vec![0x1b],
        KeyCode::Up => b"\x1b[A".to_vec(),
        KeyCode::Down => b"\x1b[B".to_vec(),
        KeyCode::Right => b"\x1b[C".to_vec(),
        KeyCode::Left => b"\x1b[D".to_vec(),
        KeyCode::Home => b"\x1b[H".to_vec(),
        KeyCode::End => b"\x1b[F".to_vec(),
        KeyCode::Delete => b"\x1b[3~".to_vec(),
        _ => Vec::new(),
    }
}

pub async fn run() -> Result<()> {
    let mut vault = Vault::open_default().context("open vault")?;
    vault.ensure_examples().context("seed example hosts")?;

    let (tx, rx) = mpsc::unbounded_channel();
    let hub = SessionHub::new(tx);

    let mut app = App::new(vault, hub, rx);
    app.try_restore_session();

    let mut terminal = setup_terminal()?;
    let result = event_loop(&mut terminal, &mut app).await;
    restore_terminal(&mut terminal)?;
    result
}

async fn event_loop(terminal: &mut DefaultTerminal, app: &mut App) -> Result<()> {
    loop {
        app.drain_events();
        app.tick_connect_spinner();
        if let Some(outcome) = app.poll_connect_outcome() {
            app.apply_connect_outcome(outcome).await?;
        }
        // Per-slot deferred refreshes.
        if app.slot().map(|s| s.pending_files_refresh).unwrap_or(false) {
            if let Some(s) = app.slot_mut() {
                s.pending_files_refresh = false;
            }
            let cwd = app.slot().map(|s| s.files.cwd.clone()).unwrap_or_default();
            let _ = app.load_dir(cwd).await;
        }
        if app.pending_open_parent {
            app.pending_open_parent = false;
            let (cwd, online) = app
                .slot()
                .map(|s| (s.files.cwd.clone(), s.files.online))
                .unwrap_or_default();
            if cwd != PathBuf::from("/") {
                let parent = join_remote(&cwd, "..");
                if online {
                    let _ = app.load_dir(parent).await;
                } else {
                    if let Some(s) = app.slot_mut() {
                        s.files = FilesState::demo();
                    }
                }
            }
        }
        app.apply_pending_drop().await?;

        let area = terminal.size().map(|s| ratatui::layout::Rect {
            x: 0,
            y: 0,
            width: s.width,
            height: s.height,
        })?;
        if let (Some(desktop), Some(files)) = (app.desktop(), app.slot().map(|s| &s.files)) {
            app.last_geo = Some(hit::compute_frame_geo(area, desktop, files));
        } else {
            app.last_geo = None;
        }

        // Keep local VT emulator + remote PTY sized to the Terminal pane.
        let term_size = app.last_geo.as_ref().and_then(|geo| {
            geo.panes
                .iter()
                .find(|p| p.app == AppKind::Terminal)
                .map(|p| (p.inner.width.max(2), p.inner.height.max(1)))
        });
        if let Some((cols, rows)) = term_size {
            let need_resize = app
                .slot()
                .map(|s| {
                    let (er, ec) = s.term.size();
                    er != rows || ec != cols
                })
                .unwrap_or(false);
            if need_resize {
                if let Some(slot) = app.slot_mut() {
                    slot.term.resize(rows, cols);
                }
                if let Some(host_id) = app.active_host_id().map(str::to_owned) {
                    let _ = app.hub.resize_pty(&host_id, cols, rows).await;
                }
            }
        }

        terminal.draw(|frame| ui::draw(frame, &app.frame_model()))?;

        if app.should_quit {
            break;
        }

        if event::poll(Duration::from_millis(50))? {
            match event::read()? {
                Event::Key(key) => app.handle_key(key).await?,
                Event::Mouse(mouse) => app.handle_mouse(mouse),
                Event::Paste(data) => app.handle_paste(data).await?,
                Event::Resize(w, h) => {
                    let _ = terminal.clear(); // Clear the screen completely to refresh borders and prevent collision leaks
                    // Resize all remote PTY channels to the new dimensions
                    if let Some(host_id) = app.active_host_id().map(str::to_owned) {
                        let (cols, rows) = app
                            .last_geo
                            .as_ref()
                            .and_then(|geo| geo.panes.iter().find(|p| p.app == AppKind::Terminal))
                            .map(|p| {
                                (
                                    p.area.width.saturating_sub(2),
                                    p.area.height.saturating_sub(2),
                                )
                            })
                            .unwrap_or((w.min(80), h.min(24)));
                        let _ = app.hub.resize_pty(&host_id, cols, rows).await;
                    }
                }
                _ => {}
            }
        }
    }
    Ok(())
}

fn setup_terminal() -> Result<DefaultTerminal> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(
        stdout,
        EnterAlternateScreen,
        EnableMouseCapture,
        EnableBracketedPaste
    )?;
    let backend = ratatui::backend::CrosstermBackend::new(stdout);
    let terminal = ratatui::Terminal::new(backend)?;
    Ok(terminal)
}

fn restore_terminal(terminal: &mut DefaultTerminal) -> Result<()> {
    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        DisableBracketedPaste,
        LeaveAlternateScreen,
        DisableMouseCapture
    )?;
    terminal.show_cursor()?;
    let _ = io::stdout().flush();
    Ok(())
}
