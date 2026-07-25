//! Application state machine: launcher ↔ desktop session.

use std::io::{self, Write};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use crossterm::event::{
    self, DisableBracketedPaste, DisableMouseCapture, EnableBracketedPaste, EnableMouseCapture,
    Event, KeyCode, KeyEvent, KeyModifiers, MouseEvent, MouseEventKind,
};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::DefaultTerminal;
use ssh_core::{join_remote, SessionEvent, SessionHub};
use ssh_os::{
    classify_paste, existing_files, Clipboard, DragPayload, DragSession, DropTarget, FileEntry,
    FileLocation, FileOp, OsDropOffer, PasteKind,
};
use ssh_vault::{AuthMethod, HostProfile, Vault};
use ssh_wm::{AppKind, Desktop, Direction};
use tokio::sync::mpsc;

use crate::apps::{EditorState, ProcessesState};
use crate::files::{resolve_open_path, FilesRow, FilesState, ViewerState};
use crate::hit::{self, FrameGeo};
use crate::hostform::{HostForm, VaultUnlockPrompt};
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
    demo_term: String,
    pending_files_refresh: bool,
    fullscreen_app: Option<AppKind>,
}

impl SessionSlot {
    fn new(host_id: String, host_name: String, online: bool) -> Self {
        let demo_term = if online {
            String::new()
        } else {
            format!(
                "Desktop for '{host_name}' running in offline/demo mode.\n\
                 Fix auth and reconnect (Esc → launcher → Enter).\n"
            )
        };
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
            demo_term,
            pending_files_refresh: false,
            fullscreen_app: None,
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
}

#[derive(Debug, Clone)]
struct MousePress {
    origin: (u16, u16),
    /// Entry indices to drag (into files.entries).
    entry_indices: Vec<usize>,
}

impl App {
    fn new(vault: Vault, hub: Arc<SessionHub>, events_rx: mpsc::UnboundedReceiver<SessionEvent>) -> Self {
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
        }
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
        if self.connect_in_flight {
            return Ok(());
        }
        self.connect_in_flight = true;
        self.status = format!("connecting to {}…", profile.name);

        let hub = Arc::clone(&self.hub);
        let vault = self.vault.clone();
        let online;
        let p_pass = password_passphrase.as_deref();
        match hub
            .connect(profile.clone(), &vault, p_pass)
            .await
        {
            Ok(_pty_id) => {
                online = true;
                self.status = format!("connected · {}", profile.name);
            }
            Err(e) => {
                online = false;
                self.status = format!("offline desktop · {e}");
            }
        }
        
        self.vault_unlock = None; // clear unlock prompt now that connection completed

        // Find or create a slot for this host.
        let slot_idx = if let Some(pos) = self.sessions.iter().position(|s| s.host_id == profile.id) {
            // Reconnect: reset state.
            let slot = &mut self.sessions[pos];
            slot.files = if online { FilesState::default() } else { FilesState::demo() };
            slot.viewer.clear();
            slot.editor.clear();
            slot.processes = if online { ProcessesState::default() } else { ProcessesState::demo() };
            slot.demo_term = if online {
                format!("Connected to '{}'.\nReady.\n", profile.name)
            } else {
                format!("Reconnect failed. Offline/demo mode.\n")
            };
            slot.desktop = Desktop::new(profile.id.clone(), profile.name.clone());
            pos
        } else {
            let mut slot = SessionSlot::new(profile.id.clone(), profile.name.clone(), online);
            slot.desktop = Desktop::new(profile.id.clone(), profile.name.clone());
            if online {
                slot.demo_term = format!(
                    "Connected to '{}'.\nReady.\n",
                    profile.name
                );
            } else {
                slot.demo_term = format!(
                    "Connection failed.\n\
                     Desktop for '{}' running in offline/demo mode.\n\
                     Fix auth and reconnect from launcher (Esc).\n",
                    profile.name
                );
            }
            self.sessions.push(slot);
            self.sessions.len() - 1
        };
        self.active_idx = slot_idx;
        self.screen = Screen::Desktop;
        self.connect_in_flight = false;

        if online {
            if let Err(e) = self.refresh_files_home().await {
                if let Some(s) = self.sessions.get_mut(slot_idx) {
                    s.files = FilesState::demo();
                }
                self.status = format!("connected · files: {e}");
            }
            let _ = self.refresh_processes().await;
        }
        let session_count = self.sessions.len();
        if session_count > 1 {
            self.status = format!(
                "{} sessions · Ctrl+Tab switch · F8 picker · {}",
                session_count,
                self.status
            );
        }
        Ok(())
    }

    /// Switch to the next session (Ctrl+Tab).
    fn session_next(&mut self) {
        if self.sessions.is_empty() { return; }
        self.active_idx = (self.active_idx + 1) % self.sessions.len();
        let name = self.sessions[self.active_idx].host_name.clone();
        self.status = format!("session [{}/{}] · {}", self.active_idx + 1, self.sessions.len(), name);
    }

    /// Switch to the previous session (Ctrl+Shift+Tab).
    fn session_prev(&mut self) {
        if self.sessions.is_empty() { return; }
        let n = self.sessions.len();
        self.active_idx = (self.active_idx + n - 1) % n;
        let name = self.sessions[self.active_idx].host_name.clone();
        self.status = format!("session [{}/{}] · {}", self.active_idx + 1, self.sessions.len(), name);
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
            if let Some(s) = self.slot_mut() { s.files = FilesState::demo(); }
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
                let cwd_display = self.slot().map(|s| s.files.cwd_display()).unwrap_or_default();
                self.status = format!("files · {}", cwd_display);
                Ok(())
            }
            Err(e) => {
                if let Some(s) = self.slot_mut() {
                    s.files.loading = false;
                    s.files.error = Some(e.to_string());
                }
                self.status = format!("files error · {e}");
                Err(e)
            }
        }
    }

    async fn open_selected_file(&mut self) -> Result<()> {
        let (cwd, row, entries, online) = match self.slot() {
            Some(s) => (s.files.cwd.clone(), s.files.selected_row(), s.files.entries.clone(), s.files.online),
            None => return Ok(()),
        };
        let Some(row) = row else { return Ok(()); };
        let Some((path, is_dir)) = resolve_open_path(&cwd, row, &entries) else {
            return Ok(());
        };

        if is_dir {
            if online {
                if let Err(e) = self.load_dir(path).await {
                    self.status = format!("cd failed · {e}");
                }
            } else if path.ends_with("Documents") {
                if let Some(s) = self.slot_mut() {
                    s.files.cwd = path;
                    s.files.entries = vec![];
                    s.files.selected = 0;
                }
                self.status = "files · /home/demo/Documents (demo empty)".into();
            } else {
                if let Some(s) = self.slot_mut() { s.files = FilesState::demo(); }
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
                            } else { String::new() };
                            self.status = format!("editor · {}", title);
                            return Ok(());
                        }
                    }
                    let (cols, rows) = self
                        .last_geo
                        .as_ref()
                        .and_then(|g| g.files_pane_inner)
                        .map(|r| (r.width.saturating_sub(2).max(20), r.height.saturating_sub(2).max(8)))
                        .unwrap_or((60, 20));
                    let title = if let Some(s) = self.slot_mut() {
                        s.viewer = ViewerState::from_content(content, cols, rows);
                        s.desktop.tree.focus_or_open_viewer();
                        s.viewer.title.clone()
                    } else { String::new() };
                    self.status = format!("viewer · {}", title);
                }
                Err(e) => self.status = format!("open failed · {e}"),
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
            } else { String::new() };
            self.status = format!("editor · {} (demo)", title);
        } else {
            let title = if let Some(s) = self.slot_mut() {
                s.viewer = ViewerState::demo_file(&path);
                s.desktop.tree.focus_or_open_viewer();
                s.viewer.title.clone()
            } else { String::new() };
            self.status = format!("viewer · {} (demo)", title);
        }
        Ok(())
    }

    async fn refresh_processes(&mut self) -> Result<()> {
        let host_id = match self.active_host_id() {
            Some(id) => id.to_owned(),
            None => {
                if let Some(s) = self.slot_mut() { s.processes = ProcessesState::demo(); }
                return Ok(());
            }
        };
        if !self.hub.is_connected(&host_id).await {
            if let Some(s) = self.slot_mut() { s.processes = ProcessesState::demo(); }
            return Ok(());
        }
        if let Some(s) = self.slot_mut() { s.processes.loading = true; }
        let cmd = "ps -eo pid,user,pcpu,pmem,comm --sort=-pcpu 2>/dev/null | head -n 50 || ps aux 2>/dev/null | head -n 50";
        match self.hub.exec_capture(&host_id, cmd).await {
            Ok(out) => {
                let row_count = if let Some(s) = self.slot_mut() {
                    s.processes = ProcessesState::from_ps(&out);
                    if s.processes.rows.is_empty() {
                        s.processes.error = Some("no process rows parsed".into());
                    }
                    s.processes.rows.len()
                } else { 0 };
                self.status = format!("processes · {} rows", row_count);
            }
            Err(e) => {
                if let Some(s) = self.slot_mut() {
                    s.processes.loading = false;
                    s.processes.error = Some(e.to_string());
                    s.processes.online = false;
                }
                self.status = format!("processes · {e}");
            }
        }
        Ok(())
    }

    async fn save_editor(&mut self) -> Result<()> {
        let (path, online) = match self.slot() {
            Some(s) => (s.editor.path.clone(), s.editor.online),
            None => return Ok(()),
        };
        let Some(path) = path else { return Ok(()); };
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
                if let Some(s) = self.slot_mut() { s.editor.dirty = false; }
                self.status = format!("saved · {}", path.display());
            }
            Err(e) => self.status = format!("save failed · {e}"),
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
                    self.status = format!("disconnected · {host_id}: {reason}");
                }
                SessionEvent::PtyData(out) => {
                    if let Ok(txt) = std::str::from_utf8(&out.data) {
                        // Strip ANSI control/escape sequence codes (like cursor positioning '16;21H')
                        // so they don't corrupt the terminal display layout.
                        let cleaned = strip_ansi_escapes(txt);
                        if let Some(slot) = self.slot_mut() {
                            slot.demo_term.push_str(&cleaned);
                            if slot.demo_term.len() > 200_000 {
                                let keep = slot.demo_term.len() - 200_000;
                                slot.demo_term.drain(..keep);
                            }
                        }
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
                SessionEvent::Status(msg) => self.status = msg,
                SessionEvent::Error(msg) => self.status = format!("error: {msg}"),
            }
        }
    }

    async fn handle_key(&mut self, key: KeyEvent) -> Result<()> {
        if self.os_drop.is_some() {
            self.handle_os_drop_key(key).await?;
            return Ok(());
        }
        if self.vault_unlock.is_some() {
            self.handle_vault_unlock_key(key).await?;
            return Ok(());
        }
        if self.host_form.is_some() {
            self.handle_host_form_key(key)?;
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
                        if let Some(s) = self.slot_mut() { s.demo_term.push_str(&text); }
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
                .map(|p| FileEntry::local(p, false))
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
        if let Some(s) = self.slot_mut() { s.pending_files_refresh = true; }
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
                self.should_quit = true;
            }
            KeyCode::Char('q') => self.should_quit = true,
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
            key.code == KeyCode::Enter
                && form.active_fields().last().copied() == Some(form.focus)
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
                self.status = format!("added · {} ({}@{}:{})", profile.name, profile.user, profile.host, profile.port);
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
                self.vault_unlock = None;
                self.status = "connect cancelled".into();
            }
            KeyCode::Backspace => {
                if let Some(prompt) = self.vault_unlock.as_mut() {
                    prompt.buffer.pop();
                    prompt.error = None;
                }
            }
            KeyCode::Enter => {
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
                let profile = match self.selected_profile().cloned() {
                    Some(p) => p,
                    None => {
                        self.vault_unlock = None;
                        return Ok(());
                    }
                };
                if let Some(prompt) = self.vault_unlock.as_mut() {
                    prompt.connecting = true;
                }
                self.status = format!("authenticating with vault key for {}...", profile.name);
                
                // We clear vault_unlock in connect_profile so the UI shows the spinner
                // while connection is in flight.
                self.connect_profile(profile, Some(passphrase)).await?;
            }
            KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
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
        if ctrl && key.code == KeyCode::Char('w') {
            self.close_current_session().await?;
            return Ok(());
        }
        if key.code == KeyCode::F(8) {
            self.show_session_switcher = !self.show_session_switcher;
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
            let mode = if self.full_screen { "decorations hidden" } else { "decorations visible" };
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
                        self.status = format!("session {}/{} · {}", idx + 1, self.sessions.len(), name);
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
            if focused == AppKind::Files && self.slot().map(|s| !s.files.marked.is_empty()).unwrap_or(false) {
                if let Some(s) = self.slot_mut() { s.files.clear_marks(); }
                self.status = "selection cleared".into();
                return Ok(());
            }
            if focused == AppKind::Editor && self.slot().map(|s| s.editor.is_open()).unwrap_or(false) {
                if self.slot().map(|s| s.editor.dirty && !s.editor.discard_armed).unwrap_or(false) {
                    if let Some(s) = self.slot_mut() { s.editor.discard_armed = true; }
                    self.status =
                        "unsaved changes · Ctrl+S save · Esc again discards".into();
                    return Ok(());
                }
                if let Some(s) = self.slot_mut() {
                    s.editor.clear();
                    if let Some((id, _)) = s.desktop.tree.leaves().into_iter().find(|(_, app)| *app == AppKind::Files) {
                        s.desktop.tree.set_focus(id);
                    }
                }
                self.status = "editor closed".into();
                return Ok(());
            }
            if focused == AppKind::Viewer && self.slot().map(|s| s.viewer.is_open()).unwrap_or(false) {
                if let Some(s) = self.slot_mut() {
                    s.viewer.clear();
                    if let Some((id, _)) = s.desktop.tree.leaves().into_iter().find(|(_, app)| *app == AppKind::Files) {
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

        let Some(desktop) = self.desktop_mut() else {
            return Ok(());
        };

        let mut refresh_procs = false;
        match (key.modifiers, key.code) {
            (KeyModifiers::CONTROL, KeyCode::Char('q')) => {
                self.should_quit = true;
                return Ok(());
            }
            (_, KeyCode::Tab) => {
                desktop.focus_next();
                return Ok(());
            }
            (KeyModifiers::SHIFT, KeyCode::BackTab) => {
                desktop.focus_prev();
                return Ok(());
            }
            (_, KeyCode::F(2)) => {
                desktop.tree.set_focused_app(AppKind::Files);
                return Ok(());
            }
            (_, KeyCode::F(3)) => {
                desktop.tree.set_focused_app(AppKind::Terminal);
                return Ok(());
            }
            (_, KeyCode::F(4)) => {
                desktop.tree.set_focused_app(AppKind::Processes);
                refresh_procs = true;
            }
            (_, KeyCode::F(5)) => {
                desktop.tree.set_focused_app(AppKind::Transfers);
                return Ok(());
            }
            (_, KeyCode::F(6)) => {
                desktop.tree.focus_or_open_viewer();
                return Ok(());
            }
            (_, KeyCode::F(7)) => {
                desktop.tree.focus_or_open_editor();
                return Ok(());
            }
            (KeyModifiers::CONTROL, KeyCode::Char('h')) => {
                let _ = desktop.tree.split_focused(Direction::Vertical, 0.5, AppKind::Files);
                return Ok(());
            }
            (KeyModifiers::CONTROL, KeyCode::Char('v')) => {
                let _ = desktop
                    .tree
                    .split_focused(Direction::Horizontal, 0.5, AppKind::Terminal);
                return Ok(());
            }
            _ => {}
        }
        if refresh_procs {
            let _ = self.refresh_processes().await;
            return Ok(());
        }

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
                    KeyCode::Char(c) => { if let Some(s) = self.slot_mut() { s.demo_term.push(c); } }
                    KeyCode::Enter => { if let Some(s) = self.slot_mut() { s.demo_term.push('\n'); } }
                    KeyCode::Backspace => {
                        if let Some(s) = self.slot_mut() { s.demo_term.pop(); }
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
            KeyCode::Up | KeyCode::Char('k') => { if let Some(s) = self.slot_mut() { s.files.move_up(); } }
            KeyCode::Down | KeyCode::Char('j') => { if let Some(s) = self.slot_mut() { s.files.move_down(); } }
            KeyCode::Char(' ') => {
                if let Some(s) = self.slot_mut() { s.files.toggle_mark_selected(); s.files.move_down(); }
            }
            KeyCode::Enter | KeyCode::Right => {
                self.open_selected_file().await?;
            }
            // Keep `l` for open; local clipboard uses Ctrl+L
            KeyCode::Char('l') => {
                self.open_selected_file().await?;
            }
            KeyCode::Backspace | KeyCode::Left | KeyCode::Char('h') => {
                let (cwd, online) = self.slot().map(|s| (s.files.cwd.clone(), s.files.online)).unwrap_or_default();
                if cwd != PathBuf::from("/") {
                    let parent = join_remote(&cwd, "..");
                    if online {
                        let _ = self.load_dir(parent).await;
                    } else {
                        if let Some(s) = self.slot_mut() { s.files = FilesState::demo(); }
                    }
                }
            }
            KeyCode::Char('r') => {
                let (online, cwd) = self.slot().map(|s| (s.files.online, s.files.cwd.clone())).unwrap_or((false, PathBuf::new()));
                if online {
                    let _ = self.load_dir(cwd).await;
                } else {
                    self.status = "files · offline demo (connect for SFTP refresh)".into();
                }
            }
            KeyCode::Char('e') => {
                let (row, cwd, entries) = self.slot().map(|s| (s.files.selected_row(), s.files.cwd.clone(), s.files.entries.clone())).unwrap_or_default();
                if let Some(row) = row {
                    if let Some((path, is_dir)) = resolve_open_path(&cwd, row, &entries) {
                        if !is_dir { self.open_path(path, true).await?; }
                    }
                }
            }
            KeyCode::Home => { if let Some(s) = self.slot_mut() { s.files.selected = 0; } }
            KeyCode::End => {
                let len = self.slot().map(|s| s.files.rows().len()).unwrap_or(0);
                if len > 0 { if let Some(s) = self.slot_mut() { s.files.selected = len - 1; } }
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
        let targets = self.slot().map(|s| s.files.clipboard_targets()).unwrap_or_default();
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
        let targets = self.slot().map(|s| s.files.clipboard_targets()).unwrap_or_default();
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
        let Some(host_id) = self.active_host_id().map(str::to_owned) else {
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

        let mut queued = 0usize;
        let mut moved = 0usize;
        let mut skipped = 0usize;

        for entry in &entries {
            let name = match &entry.location {
                FileLocation::Local { path } | FileLocation::Remote { path, .. } => path
                    .file_name()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_else(|| "file".into()),
            };
            let dest = dest_dir.join(&name);

            match (&entry.location, op, entry.is_dir) {
                (FileLocation::Remote { host_id: src, path }, FileOp::Cut, _) if src == &host_id => {
                    if path == &dest {
                        skipped += 1;
                        continue;
                    }
                    match self.hub.remote_rename(&host_id, path, &dest).await {
                        Ok(()) => moved += 1,
                        Err(e) => {
                            self.status = format!("move failed · {e}");
                            return Ok(());
                        }
                    }
                }
                (FileLocation::Remote { host_id: src, path }, FileOp::Copy, false)
                    if src == &host_id =>
                {
                    match self
                        .hub
                        .enqueue_remote_copy(&host_id, path.clone(), dest)
                        .await
                    {
                        Ok(_) => queued += 1,
                        Err(e) => self.status = format!("copy failed · {e}"),
                    }
                }
                (FileLocation::Remote { .. }, FileOp::Copy, true) => {
                    skipped += 1;
                    self.status = "directory copy not supported yet (files only)".into();
                }
                (FileLocation::Local { path }, FileOp::Copy | FileOp::Cut, false) => {
                    let cut = op == FileOp::Cut;
                    match self
                        .hub
                        .enqueue_upload_ex(&host_id, path.clone(), dest_dir.clone(), cut)
                        .await
                    {
                        Ok(_) => queued += 1,
                        Err(e) => self.status = format!("upload failed · {e}"),
                    }
                }
                (FileLocation::Local { .. }, _, true) => {
                    skipped += 1;
                    self.status = "directory upload via clipboard not supported yet".into();
                }
                (FileLocation::Remote { host_id: src, .. }, _, _) if src != &host_id => {
                    skipped += 1;
                    self.status = "cross-host paste not supported yet".into();
                }
                _ => skipped += 1,
            }
        }

        if op == FileOp::Cut {
            self.clipboard.clear_files();
        }

        if let Some(s) = self.slot_mut() { s.files.clear_marks(); s.pending_files_refresh = true; }
        self.status = format!(
            "paste · {moved} moved · {queued} queued · {skipped} skipped → {}",
            dest_dir.display()
        );
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
                FileLocation::Remote {
                    host_id: src,
                    path,
                } if src == host_id && !entry.is_dir => {
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
                    self.status = "skip dirs / other hosts for paste-to-local".into();
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
        let (online, cwd) = self.slot().map(|s| (s.files.online, s.files.cwd.clone())).unwrap_or_default();
        if !online {
            self.status = "upload needs a live SFTP session".into();
            return;
        }
        self.path_prompt = Some(PathPrompt::upload(cwd));
        self.status = "upload · pick a local file (Enter) or type path".into();
    }

    fn begin_download_prompt(&mut self) {
        let (online, row, cwd, entries) = self.slot().map(|s| (
            s.files.online,
            s.files.selected_row(),
            s.files.cwd.clone(),
            s.files.entries.clone(),
        )).unwrap_or_default();
        if !online {
            self.status = "download needs a live SFTP session".into();
            return;
        }
        let Some(row) = row else { return; };
        let Some((path, is_dir)) = resolve_open_path(&cwd, row, &entries) else { return; };
        if is_dir {
            self.status = "select a file to download (dirs later)".into();
            return;
        }
        let size = entries.iter().find(|e| e.path == path).and_then(|e| e.size);
        self.path_prompt = Some(PathPrompt::download(path, size));
        self.status = "download · confirm local path and press Enter".into();
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
                if !local.is_file() {
                    self.status = format!("not a file: {}", local.display());
                    return Ok(());
                }
                self.clipboard
                    .set_files(vec![FileEntry::local(local.clone(), false)], FileOp::Copy);
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
            PathPromptKind::Upload => match self.hub.enqueue_upload(&host_id, local, remote).await {
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
            },
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
            KeyCode::Up | KeyCode::Char('k') => { if let Some(s) = self.slot_mut() { s.transfers.move_up(); } }
            KeyCode::Down | KeyCode::Char('j') => { if let Some(s) = self.slot_mut() { s.transfers.move_down(); } }
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
            KeyCode::Up | KeyCode::Char('k') => { if let Some(s) = self.slot_mut() { s.viewer.scroll_by(-1); } }
            KeyCode::Down | KeyCode::Char('j') => { if let Some(s) = self.slot_mut() { s.viewer.scroll_by(1); } }
            KeyCode::PageUp => { if let Some(s) = self.slot_mut() { s.viewer.scroll_by(-10); } }
            KeyCode::PageDown => { if let Some(s) = self.slot_mut() { s.viewer.scroll_by(10); } }
            KeyCode::Home => { if let Some(s) = self.slot_mut() { s.viewer.scroll = 0; } }
            KeyCode::Char('e') => {
                let (path, binary) = self.slot().map(|s| (s.viewer.path.clone(), s.viewer.binary)).unwrap_or_default();
                if let Some(path) = path {
                    if !binary {
                        self.open_path(path, true).await?;
                    } else {
                        self.status = "cannot edit binary/hex view".into();
                    }
                }
            }
            KeyCode::Char('q') => {
                if let Some(s) = self.slot_mut() { s.viewer.clear(); }
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
            KeyCode::Left => { if let Some(s) = self.slot_mut() { s.editor.move_left(); } }
            KeyCode::Right => { if let Some(s) = self.slot_mut() { s.editor.move_right(); } }
            KeyCode::Up => { if let Some(s) = self.slot_mut() { s.editor.move_up(); } }
            KeyCode::Down => { if let Some(s) = self.slot_mut() { s.editor.move_down(); } }
            KeyCode::Home => { if let Some(s) = self.slot_mut() { s.editor.cursor_col = 0; } }
            KeyCode::End => {
                if let Some(s) = self.slot_mut() {
                    s.editor.cursor_col = s.editor.lines[s.editor.cursor_row].chars().count();
                }
            }
            KeyCode::Enter => { if let Some(s) = self.slot_mut() { s.editor.insert_newline(); } }
            KeyCode::Backspace => { if let Some(s) = self.slot_mut() { s.editor.backspace(); } }
            KeyCode::Char(c) if !key.modifiers.contains(KeyModifiers::CONTROL) => {
                if let Some(s) = self.slot_mut() { s.editor.insert_char(c); }
            }
            _ => {}
        }
        let height = self
            .last_geo
            .as_ref()
            .and_then(|g| g.files_pane_inner)
            .map(|r| r.height.saturating_sub(2).max(4))
            .unwrap_or(20);
        if let Some(s) = self.slot_mut() { s.editor.ensure_visible(height); }
        Ok(())
    }

    async fn handle_processes_key(&mut self, key: KeyEvent) -> Result<()> {
        match key.code {
            KeyCode::Up | KeyCode::Char('k') => { if let Some(s) = self.slot_mut() { s.processes.move_up(); } }
            KeyCode::Down | KeyCode::Char('j') => { if let Some(s) = self.slot_mut() { s.processes.move_down(); } }
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
                        let already_marked = self.slot().map(|s| s.files.is_marked(entry_idx)).unwrap_or(false);
                        let marked = self.slot().map(|s| s.files.marked.clone()).unwrap_or_default();
                        if let Some(s) = self.slot_mut() { s.files.selected = row.row_index; }
                        let indices = if already_marked { marked } else { vec![entry_idx] };
                        self.mouse_press = Some(MousePress { origin: pos, entry_indices: indices });
                    } else {
                        if let Some(s) = self.slot_mut() { s.files.selected = row.row_index; }
                        self.mouse_press = Some(MousePress { origin: pos, entry_indices: Vec::new() });
                    }
                } else {
                    self.mouse_press = None;
                }
            }
            MouseEventKind::Drag(event::MouseButton::Left) => {
                if let Some(press) = self.mouse_press.clone() {
                    let dx = pos.0.abs_diff(press.origin.0);
                    let dy = pos.1.abs_diff(press.origin.1);
                    if self.drag.is_none() && (dx >= 1 || dy >= 1) && !press.entry_indices.is_empty()
                    {
                        let Some(host_id) = self.active_host_id().map(str::to_owned) else {
                            return Ok(());
                        };
                        let slot_entries = self.slot().map(|s| s.files.entries.clone()).unwrap_or_default();
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
                        if self.slot().and_then(|s| s.files.selected_row()).as_ref() == Some(&FilesRow::Parent) {
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
                    DropTarget::TransferDock => self.slot().map(|s| s.files.cwd.clone()).unwrap_or_default(),
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
                    .unwrap_or_else(|| self.slot().map(|s| s.files.cwd.clone()).unwrap_or_default());
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
        let term_buffer = slot.map(|s| s.demo_term.as_str()).unwrap_or("");
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
            term_buffer,
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
            drag: self.drag.as_ref(),
            drop_target: self.drop_target.as_ref(),
            os_drop: self.os_drop.as_ref(),
        }
    }
}


// Empty-state references for the frame_model fallbacks (no active session).
fn empty_files() -> &'static FilesState {
    use std::sync::OnceLock;
    static CELL: OnceLock<FilesState> = OnceLock::new();
    CELL.get_or_init(FilesState::default)
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

    let mut terminal = setup_terminal()?;
    let result = event_loop(&mut terminal, &mut app).await;
    restore_terminal(&mut terminal)?;
    result
}

async fn event_loop(terminal: &mut DefaultTerminal, app: &mut App) -> Result<()> {
    loop {
        app.drain_events();
        // Per-slot deferred refreshes.
        if app.slot().map(|s| s.pending_files_refresh).unwrap_or(false) {
            if let Some(s) = app.slot_mut() { s.pending_files_refresh = false; }
            let cwd = app.slot().map(|s| s.files.cwd.clone()).unwrap_or_default();
            let _ = app.load_dir(cwd).await;
        }
        if app.pending_open_parent {
            app.pending_open_parent = false;
            let (cwd, online) = app.slot().map(|s| (s.files.cwd.clone(), s.files.online)).unwrap_or_default();
            if cwd != PathBuf::from("/") {
                let parent = join_remote(&cwd, "..");
                if online {
                    let _ = app.load_dir(parent).await;
                } else {
                    if let Some(s) = app.slot_mut() { s.files = FilesState::demo(); }
                }
            }
        }
        app.apply_pending_drop().await?;

        let area = terminal.size().map(|s| {
            ratatui::layout::Rect {
                x: 0,
                y: 0,
                width: s.width,
                height: s.height,
            }
        })?;
        if let (Some(desktop), Some(files)) = (app.desktop(), app.slot().map(|s| &s.files)) {
            app.last_geo = Some(hit::compute_frame_geo(area, desktop, files));
        } else {
            app.last_geo = None;
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
                         let (cols, rows) = app.last_geo.as_ref()
                             .and_then(|geo| geo.panes.iter().find(|p| p.app == AppKind::Terminal))
                             .map(|p| (p.area.width.saturating_sub(2), p.area.height.saturating_sub(2)))
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

fn strip_ansi_escapes(s: &str) -> String {
    let mut result = String::new();
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\x1b' {
            if chars.peek() == Some(&'[') {
                let _ = chars.next(); // consume '['
                // Consume arguments up to letter code
                while let Some(&c2) = chars.peek() {
                    let _ = chars.next();
                    if c2.is_ascii_alphabetic() {
                        break;
                    }
                }
            }
        } else if c == '\x08' {
            // PTY erases by sending: BS ('\x08'), Space (' '), BS ('\x08')
            result.pop();
            if chars.peek() == Some(&' ') {
                let _ = chars.next(); // consume space
                if chars.peek() == Some(&'\x08') {
                    let _ = chars.next(); // consume second BS
                }
            }
        } else if c == '\x7f' {
            result.pop();
        } else {
            result.push(c);
        }
    }
    result
}
