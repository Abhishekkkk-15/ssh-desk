//! Application state machine: launcher ↔ desktop session.

use std::io::{self, Write};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use crossterm::event::{
    self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEvent, KeyModifiers,
    MouseEvent, MouseEventKind,
};
use crossterm::execute;
use crossterm::terminal::{
    disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen,
};
use ratatui::DefaultTerminal;
use ssh_core::{join_remote, SessionEvent, SessionHub};
use ssh_os::{Clipboard, FileEntry, FileLocation, FileOp};
use ssh_vault::{HostProfile, Vault};
use ssh_wm::{AppKind, Desktop, Direction};
use tokio::sync::mpsc;

use crate::files::{resolve_open_path, FilesState, ViewerState};
use crate::transfers::{PathPrompt, PathPromptKind, TransfersUi};
use crate::ui::{self, UiFrame};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Screen {
    Launcher,
    Desktop,
}

pub struct App {
    screen: Screen,
    vault: Vault,
    hosts: Vec<HostProfile>,
    selected_host: usize,
    desktop: Option<Desktop>,
    active_host_id: Option<String>,
    status: String,
    hub: Arc<SessionHub>,
    events_rx: mpsc::UnboundedReceiver<SessionEvent>,
    clipboard: Clipboard,
    /// Shell scrollback / demo buffer.
    demo_term: String,
    files: FilesState,
    viewer: ViewerState,
    transfers: TransfersUi,
    path_prompt: Option<PathPrompt>,
    pending_files_refresh: bool,
    should_quit: bool,
    connect_in_flight: bool,
}

impl App {
    fn new(vault: Vault, hub: Arc<SessionHub>, events_rx: mpsc::UnboundedReceiver<SessionEvent>) -> Self {
        let hosts = vault.hosts().to_vec();
        Self {
            screen: Screen::Launcher,
            vault,
            hosts,
            selected_host: 0,
            desktop: None,
            active_host_id: None,
            status: "ssh-desk · select a host and press Enter to connect".into(),
            hub,
            events_rx,
            clipboard: Clipboard::new(),
            demo_term: String::from(
                "┌ ssh-desk terminal ─────────────────────\n\
                 │ Not connected yet.\n\
                 │ From the launcher, pick a host and press Enter.\n\
                 │ Keys: Tab focus · F2 Files · F3 Term · F5 Transfers\n\
                 │       Ctrl+U upload · Ctrl+D download · q quit\n\
                 └──────────────────────────────────────────\n",
            ),
            files: FilesState::default(),
            viewer: ViewerState::default(),
            transfers: TransfersUi::default(),
            path_prompt: None,
            pending_files_refresh: false,
            should_quit: false,
            connect_in_flight: false,
        }
    }

    fn selected_profile(&self) -> Option<&HostProfile> {
        self.hosts.get(self.selected_host)
    }

    async fn connect_selected(&mut self) -> Result<()> {
        let Some(profile) = self.selected_profile().cloned() else {
            self.status = "no hosts in vault — add one under ~/.config/ssh-desk/hosts.toml".into();
            return Ok(());
        };
        if self.connect_in_flight {
            return Ok(());
        }
        self.connect_in_flight = true;
        self.status = format!("connecting to {}…", profile.name);

        let hub = Arc::clone(&self.hub);
        let vault = self.vault.clone();
        match hub.connect(profile.clone(), &vault, None).await {
            Ok(_pty_id) => {
                self.active_host_id = Some(profile.id.clone());
                self.desktop = Some(Desktop::new(profile.id.clone(), profile.name.clone()));
                self.screen = Screen::Desktop;
                self.demo_term.clear();
                self.viewer.clear();
                self.status = format!("connected · {}", profile.name);
                if let Err(e) = self.refresh_files_home().await {
                    self.files = FilesState::demo();
                    self.status = format!("connected · files: {e}");
                }
            }
            Err(e) => {
                self.active_host_id = Some(profile.id.clone());
                self.desktop = Some(Desktop::new(profile.id.clone(), profile.name.clone()));
                self.screen = Screen::Desktop;
                self.demo_term = format!(
                    "Connection failed: {e}\n\n\
                     Desktop shell is open in offline/demo mode.\n\
                     Fix auth (ssh-agent / key) and reconnect from launcher (Esc).\n"
                );
                self.files = FilesState::demo();
                self.viewer.clear();
                self.status = format!("offline desktop · {e}");
            }
        }
        self.connect_in_flight = false;
        Ok(())
    }

    async fn refresh_files_home(&mut self) -> Result<(), ssh_core::CoreError> {
        let Some(host_id) = self.active_host_id.clone() else {
            return Ok(());
        };
        if !self.hub.has_sftp(&host_id).await {
            self.files = FilesState::demo();
            return Ok(());
        }
        let home = self.hub.canonicalize(&host_id, ".").await?;
        self.load_dir(home).await
    }

    async fn load_dir(&mut self, path: PathBuf) -> Result<(), ssh_core::CoreError> {
        let Some(host_id) = self.active_host_id.clone() else {
            return Ok(());
        };
        self.files.loading = true;
        self.files.error = None;
        match self.hub.list_dir(&host_id, &path).await {
            Ok(entries) => {
                self.files.set_listing(path.clone(), entries);
                self.status = format!("files · {}", self.files.cwd_display());
                Ok(())
            }
            Err(e) => {
                self.files.loading = false;
                self.files.error = Some(e.to_string());
                self.status = format!("files error · {e}");
                Err(e)
            }
        }
    }

    async fn open_selected_file(&mut self) -> Result<()> {
        let Some(row) = self.files.selected_row() else {
            return Ok(());
        };
        let Some((path, is_dir)) = resolve_open_path(&self.files.cwd, row, &self.files.entries) else {
            return Ok(());
        };

        if is_dir {
            if self.files.online {
                if let Err(e) = self.load_dir(path).await {
                    self.status = format!("cd failed · {e}");
                }
            } else {
                // Demo: only allow parent / Documents
                if path.ends_with("Documents") {
                    self.files.cwd = path;
                    self.files.entries = vec![];
                    self.files.selected = 0;
                    self.status = "files · /home/demo/Documents (demo empty)".into();
                } else {
                    self.files = FilesState::demo();
                    self.status = "files · demo root".into();
                }
            }
            return Ok(());
        }

        if self.files.online {
            let Some(host_id) = self.active_host_id.clone() else {
                return Ok(());
            };
            match self.hub.read_file(&host_id, &path).await {
                Ok(content) => {
                    self.viewer = ViewerState::from_content(content);
                    if let Some(desktop) = self.desktop.as_mut() {
                        desktop.tree.focus_or_open_viewer();
                    }
                    self.status = format!("viewer · {}", self.viewer.title);
                }
                Err(e) => self.status = format!("open failed · {e}"),
            }
        } else {
            self.viewer = ViewerState::demo_file(&path);
            if let Some(desktop) = self.desktop.as_mut() {
                desktop.tree.focus_or_open_viewer();
            }
            self.status = format!("viewer · {} (demo)", self.viewer.title);
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
                    if let Ok(s) = std::str::from_utf8(&out.data) {
                        self.demo_term.push_str(s);
                        if self.demo_term.len() > 200_000 {
                            let keep = self.demo_term.len() - 200_000;
                            self.demo_term.drain(..keep);
                        }
                    }
                }
                SessionEvent::TransferUpdate(job) => {
                    let done_upload = job.status == ssh_core::TransferStatus::Done
                        && job.direction == ssh_core::TransferDirection::Upload;
                    let name = job.display_name();
                    let status = job.status;
                    self.transfers.upsert(job);
                    self.status = format!("transfer · {} · {}", name, status.label());
                    if done_upload && self.files.online {
                        self.pending_files_refresh = true;
                    }
                }
                SessionEvent::Status(msg) => self.status = msg,
                SessionEvent::Error(msg) => self.status = format!("error: {msg}"),
            }
        }
    }

    async fn handle_key(&mut self, key: KeyEvent) -> Result<()> {
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
            KeyCode::Char('r') => {
                self.vault = Vault::open_default()?;
                self.hosts = self.vault.hosts().to_vec();
                self.status = "vault reloaded".into();
            }
            _ => {}
        }
        Ok(())
    }

    async fn handle_desktop_key(&mut self, key: KeyEvent) -> Result<()> {
        let focused = self
            .desktop
            .as_ref()
            .map(|d| d.focused_app())
            .unwrap_or(AppKind::Terminal);

        // Esc: clear file marks → close viewer → launcher
        if key.code == KeyCode::Esc && key.modifiers.is_empty() {
            if focused == AppKind::Files && !self.files.marked.is_empty() {
                self.files.clear_marks();
                self.status = "selection cleared".into();
                return Ok(());
            }
            if focused == AppKind::Viewer && self.viewer.is_open() {
                self.viewer.clear();
                if let Some(desktop) = self.desktop.as_mut() {
                    // Prefer returning focus to Files
                    if let Some((id, _)) = desktop
                        .tree
                        .leaves()
                        .into_iter()
                        .find(|(_, app)| *app == AppKind::Files)
                    {
                        desktop.tree.set_focus(id);
                    }
                }
                self.status = "viewer closed".into();
                return Ok(());
            }
            if let Some(id) = self.active_host_id.clone() {
                let _ = self.hub.disconnect(&id).await;
            }
            self.screen = Screen::Launcher;
            self.status = "back to launcher".into();
            return Ok(());
        }

        let Some(desktop) = self.desktop.as_mut() else {
            return Ok(());
        };

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
                return Ok(());
            }
            (_, KeyCode::F(5)) => {
                desktop.tree.set_focused_app(AppKind::Transfers);
                return Ok(());
            }
            (_, KeyCode::F(6)) => {
                desktop.tree.focus_or_open_viewer();
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

        let focused = desktop.focused_app();
        match focused {
            AppKind::Terminal => {
                if let Some(host_id) = &self.active_host_id {
                    if self.hub.is_connected(host_id).await {
                        let bytes = key_to_bytes(key);
                        if !bytes.is_empty() {
                            let _ = self.hub.write_pty(host_id, &bytes).await;
                        }
                        return Ok(());
                    }
                }
                match key.code {
                    KeyCode::Char(c) => self.demo_term.push(c),
                    KeyCode::Enter => self.demo_term.push('\n'),
                    KeyCode::Backspace => {
                        self.demo_term.pop();
                    }
                    _ => {}
                }
            }
            AppKind::Files => self.handle_files_key(key).await?,
            AppKind::Viewer => self.handle_viewer_key(key),
            AppKind::Transfers => self.handle_transfers_key(key).await?,
            AppKind::Processes => {
                self.status = "processes · Phase 4 remote ps view".into();
            }
            AppKind::Editor => {
                self.status = "editor · save-back in Phase 7".into();
            }
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
            KeyCode::Up | KeyCode::Char('k') => self.files.move_up(),
            KeyCode::Down | KeyCode::Char('j') => self.files.move_down(),
            KeyCode::Char(' ') => {
                self.files.toggle_mark_selected();
                self.files.move_down();
            }
            KeyCode::Enter | KeyCode::Right => {
                self.open_selected_file().await?;
            }
            // Keep `l` for open; local clipboard uses Ctrl+L
            KeyCode::Char('l') => {
                self.open_selected_file().await?;
            }
            KeyCode::Backspace | KeyCode::Left | KeyCode::Char('h') => {
                if self.files.cwd != PathBuf::from("/") {
                    let parent = join_remote(&self.files.cwd, "..");
                    if self.files.online {
                        let _ = self.load_dir(parent).await;
                    } else {
                        self.files = FilesState::demo();
                    }
                }
            }
            KeyCode::Char('r') => {
                if self.files.online {
                    let cwd = self.files.cwd.clone();
                    let _ = self.load_dir(cwd).await;
                } else {
                    self.status = "files · offline demo (connect for SFTP refresh)".into();
                }
            }
            KeyCode::Home => self.files.selected = 0,
            KeyCode::End => {
                let len = self.files.rows().len();
                if len > 0 {
                    self.files.selected = len - 1;
                }
            }
            _ => {}
        }
        Ok(())
    }

    fn clipboard_copy_remote(&mut self) {
        let Some(host_id) = self.active_host_id.clone() else {
            self.status = "no session".into();
            return;
        };
        let targets = self.files.clipboard_targets();
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
        let Some(host_id) = self.active_host_id.clone() else {
            self.status = "no session".into();
            return;
        };
        let targets = self.files.clipboard_targets();
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
        if !self.files.online {
            self.status = "paste needs a live SFTP session".into();
            return Ok(());
        }
        let Some(host_id) = self.active_host_id.clone() else {
            self.status = "no session".into();
            return Ok(());
        };
        let op = self.clipboard.file_op().unwrap_or(FileOp::Copy);
        let entries = self.clipboard.files().to_vec();
        if entries.is_empty() {
            self.status = "clipboard empty · Ctrl+C / Ctrl+L first".into();
            return Ok(());
        }

        let dest_dir = self.files.cwd.clone();
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

        if op == FileOp::Cut && moved > 0 {
            self.clipboard.clear_files();
        }
        // Cut uploads clear after successful transfer via delete_local_after;
        // clear clipboard now for cut remotes that were renamed.
        if op == FileOp::Cut && queued == 0 && moved > 0 {
            self.clipboard.clear_files();
        }
        if op == FileOp::Cut && queued > 0 {
            // Keep clipboard until transfers finish would be nicer; clear to avoid double paste.
            self.clipboard.clear_files();
        }

        self.files.clear_marks();
        self.pending_files_refresh = true;
        self.status = format!(
            "paste · {moved} moved · {queued} queued · {skipped} skipped"
        );
        Ok(())
    }

    async fn clipboard_paste_to_local(&mut self) -> Result<()> {
        let Some(host_id) = self.active_host_id.clone() else {
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
            if let Some(desktop) = self.desktop.as_mut() {
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
        if !self.files.online {
            self.status = "upload needs a live SFTP session".into();
            return;
        }
        self.path_prompt = Some(PathPrompt::upload(self.files.cwd.clone()));
        self.status = "upload · pick a local file (Enter) or type path".into();
    }

    fn begin_download_prompt(&mut self) {
        if !self.files.online {
            self.status = "download needs a live SFTP session".into();
            return;
        }
        let Some(row) = self.files.selected_row() else {
            return;
        };
        let Some((path, is_dir)) = resolve_open_path(&self.files.cwd, row, &self.files.entries) else {
            return;
        };
        if is_dir {
            self.status = "select a file to download (dirs later)".into();
            return;
        }
        let size = self
            .files
            .entries
            .iter()
            .find(|e| e.path == path)
            .and_then(|e| e.size);
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

        let Some(host_id) = self.active_host_id.clone() else {
            self.status = "no active session".into();
            return Ok(());
        };
        match kind {
            PathPromptKind::Upload => match self.hub.enqueue_upload(&host_id, local, remote).await {
                Ok(id) => {
                    self.status = format!("queued upload · {}", id.0);
                    if let Some(desktop) = self.desktop.as_mut() {
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
                        if let Some(desktop) = self.desktop.as_mut() {
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
            KeyCode::Up | KeyCode::Char('k') => self.transfers.move_up(),
            KeyCode::Down | KeyCode::Char('j') => self.transfers.move_down(),
            KeyCode::Char('c') => {
                if let Some(id) = self.transfers.selected_id() {
                    if self.hub.cancel_transfer(id).await {
                        self.status = "transfer cancel requested".into();
                    }
                }
            }
            KeyCode::Char('r') => {
                if let Some(id) = self.transfers.selected_id() {
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

    fn handle_viewer_key(&mut self, key: KeyEvent) {
        match key.code {
            KeyCode::Up | KeyCode::Char('k') => self.viewer.scroll_by(-1),
            KeyCode::Down | KeyCode::Char('j') => self.viewer.scroll_by(1),
            KeyCode::PageUp => self.viewer.scroll_by(-10),
            KeyCode::PageDown => self.viewer.scroll_by(10),
            KeyCode::Home => self.viewer.scroll = 0,
            KeyCode::Char('q') => {
                self.viewer.clear();
                self.status = "viewer closed".into();
            }
            _ => {}
        }
    }

    fn handle_mouse(&mut self, mouse: MouseEvent) {
        if mouse.kind == MouseEventKind::Down(event::MouseButton::Left) {
            if let Some(desktop) = self.desktop.as_mut() {
                let _ = mouse;
                desktop.focus_next();
                self.status = format!("focus → {}", desktop.focused_app().label());
            }
        }
    }

    fn frame_model(&self) -> UiFrame<'_> {
        UiFrame {
            screen: match self.screen {
                Screen::Launcher => ui::ScreenKind::Launcher,
                Screen::Desktop => ui::ScreenKind::Desktop,
            },
            hosts: &self.hosts,
            selected_host: self.selected_host,
            desktop: self.desktop.as_ref(),
            status: &self.status,
            term_buffer: &self.demo_term,
            clipboard_has_files: self.clipboard.has_files(),
            clipboard_label: match self.clipboard.file_op() {
                Some(ssh_os::FileOp::Copy) => "copy",
                Some(ssh_os::FileOp::Cut) => "cut",
                None => "",
            },
            files: &self.files,
            viewer: &self.viewer,
            transfers: &self.transfers,
            path_prompt: self.path_prompt.as_ref(),
        }
    }
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
        if app.pending_files_refresh {
            app.pending_files_refresh = false;
            let cwd = app.files.cwd.clone();
            let _ = app.load_dir(cwd).await;
        }
        terminal.draw(|frame| ui::draw(frame, &app.frame_model()))?;

        if app.should_quit {
            break;
        }

        if event::poll(Duration::from_millis(50))? {
            match event::read()? {
                Event::Key(key) => app.handle_key(key).await?,
                Event::Mouse(mouse) => app.handle_mouse(mouse),
                Event::Resize(_, _) => {}
                _ => {}
            }
        }
    }
    Ok(())
}

fn setup_terminal() -> Result<DefaultTerminal> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let backend = ratatui::backend::CrosstermBackend::new(stdout);
    let terminal = ratatui::Terminal::new(backend)?;
    Ok(terminal)
}

fn restore_terminal(terminal: &mut DefaultTerminal) -> Result<()> {
    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
    )?;
    terminal.show_cursor()?;
    let _ = io::stdout().flush();
    Ok(())
}
