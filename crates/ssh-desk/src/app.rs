//! Application state machine: launcher ↔ desktop session.

use std::io::{self, Write};
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
use ssh_core::{SessionEvent, SessionHub};
use ssh_os::Clipboard;
use ssh_vault::{HostProfile, Vault};
use ssh_wm::{AppKind, Desktop, Direction};
use tokio::sync::mpsc;

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
    /// Demo scrollback when not yet connected.
    demo_term: String,
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
                 │ Keys: Tab focus · F2 Files · F3 Term · F4 Procs\n\
                 │       Ctrl+H/V split · q quit\n\
                 └──────────────────────────────────────────\n",
            ),
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
                self.status = format!("connected · {}", profile.name);
            }
            Err(e) => {
                // Still enter desktop with demo panes so Phase 0 UX is usable offline.
                self.active_host_id = Some(profile.id.clone());
                self.desktop = Some(Desktop::new(profile.id.clone(), profile.name.clone()));
                self.screen = Screen::Desktop;
                self.demo_term = format!(
                    "Connection failed: {e}\n\n\
                     Desktop shell is open in offline/demo mode.\n\
                     Fix auth (ssh-agent / key) and reconnect from launcher (Esc).\n"
                );
                self.status = format!("offline desktop · {e}");
            }
        }
        self.connect_in_flight = false;
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
                        // Cap UI buffer
                        if self.demo_term.len() > 200_000 {
                            let keep = self.demo_term.len() - 200_000;
                            self.demo_term.drain(..keep);
                        }
                    }
                }
                SessionEvent::Status(msg) => self.status = msg,
                SessionEvent::Error(msg) => self.status = format!("error: {msg}"),
            }
        }
    }

    async fn handle_key(&mut self, key: KeyEvent) -> Result<()> {
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
        let Some(desktop) = self.desktop.as_mut() else {
            return Ok(());
        };

        // Global desktop bindings
        match (key.modifiers, key.code) {
            (KeyModifiers::CONTROL, KeyCode::Char('q')) => {
                self.should_quit = true;
                return Ok(());
            }
            (_, KeyCode::Esc) => {
                if let Some(id) = self.active_host_id.clone() {
                    let _ = self.hub.disconnect(&id).await;
                }
                self.screen = Screen::Launcher;
                self.status = "back to launcher".into();
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
            (KeyModifiers::CONTROL, KeyCode::Char('h')) => {
                let _ = desktop.tree.split_focused(Direction::Vertical, 0.5, AppKind::Files);
                return Ok(());
            }
            (KeyModifiers::CONTROL, KeyCode::Char('v')) => {
                // Avoid conflicting with paste later; use Ctrl+S for vertical split mnemonic "split"
                let _ = desktop
                    .tree
                    .split_focused(Direction::Horizontal, 0.5, AppKind::Terminal);
                return Ok(());
            }
            _ => {}
        }

        // Route input to focused app
        match desktop.focused_app() {
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
                // Demo mode echo
                match key.code {
                    KeyCode::Char(c) => self.demo_term.push(c),
                    KeyCode::Enter => self.demo_term.push('\n'),
                    KeyCode::Backspace => {
                        self.demo_term.pop();
                    }
                    _ => {}
                }
            }
            AppKind::Files => {
                self.status = "files · Phase 2 will browse SFTP (clipboard/DnD wired in OS layer)"
                    .into();
            }
            AppKind::Transfers => {
                self.status = "transfers · queue empty (paste/DnD will feed this)".into();
            }
            AppKind::Processes => {
                self.status = "processes · Phase 4 remote ps view".into();
            }
            AppKind::Viewer | AppKind::Editor => {
                self.status = "viewer/editor · open files from Files app (Phase 2+)".into();
            }
            AppKind::Launcher => {}
        }
        Ok(())
    }

    fn handle_mouse(&mut self, mouse: MouseEvent) {
        if mouse.kind == MouseEventKind::Down(event::MouseButton::Left) {
            if let Some(desktop) = self.desktop.as_mut() {
                // Simple focus cycle on click for Phase 0; hit-testing arrives with layout geo.
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
        terminal.draw(|frame| ui::draw(frame, &app.frame_model()))?;

        if app.should_quit {
            break;
        }

        // Poll input with a short timeout so SSH events paint promptly.
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
