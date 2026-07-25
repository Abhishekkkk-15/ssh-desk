//! ratatui views for launcher and desktop OS shell.

use std::path::PathBuf;

use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Modifier, Style};
use crate::theme::Theme as Th;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, Paragraph, Wrap};
use ratatui::Frame;
use ssh_os::{DragPayload, DragSession, DropTarget, OsDropOffer};
use ssh_vault::HostProfile;
use ssh_wm::{AppKind, Desktop, Direction as SplitDir, PaneNode};

use crate::apps::{EditorState, ProcessesState};
use crate::diagnostics::{DiagLevel, DiagnosticsState};
use crate::files::{FilesRow, FilesState, ViewerKind, ViewerState};
use crate::files_prompt::FilesPrompt;
use crate::hostform::{HostField, HostForm, VaultUnlockPrompt};
use crate::term::TermEmulator;
use crate::transfers::{PathPrompt, PathPromptKind, TransfersUi};
use crate::app::OverwritePrompt;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScreenKind {
    Launcher,
    Desktop,
}

pub struct UiFrame<'a> {
    pub screen: ScreenKind,
    pub hosts: &'a [HostProfile],
    pub selected_host: usize,
    /// Names of all open sessions, for the tab bar.
    pub sessions: Vec<&'a str>,
    pub active_session_idx: usize,
    pub show_session_switcher: bool,
    pub full_screen: bool,
    pub fullscreen_app: Option<AppKind>,
    pub desktop: Option<&'a Desktop>,
    pub status: &'a str,
    pub term: &'a TermEmulator,
    pub clipboard_has_files: bool,
    pub clipboard_label: &'a str,
    pub files: &'a FilesState,
    pub viewer: &'a ViewerState,
    pub editor: &'a EditorState,
    pub processes: &'a ProcessesState,
    pub transfers: &'a TransfersUi,
    pub path_prompt: Option<&'a PathPrompt>,
    pub host_form: Option<&'a HostForm>,
    pub vault_unlock: Option<&'a VaultUnlockPrompt>,
    pub drag: Option<&'a DragSession>,
    pub drop_target: Option<&'a DropTarget>,
    pub os_drop: Option<&'a OsDropOffer>,
    pub overwrite_prompt: Option<&'a OverwritePrompt>,
    pub files_prompt: Option<&'a FilesPrompt>,
    pub diagnostics: &'a DiagnosticsState,
}

pub fn draw(frame: &mut Frame<'_>, model: &UiFrame<'_>) {
    let area = frame.area();
    // Soft slate canvas (avoid pure pitch-black).
    frame.render_widget(
        Block::default().style(Style::default().bg(Th::BG)),
        area,
    );
    if model.screen == ScreenKind::Desktop && model.full_screen {
        if let Some(desktop) = model.desktop {
            draw_desktop(frame, area, desktop, model);
        }
        if model.show_session_switcher {
            draw_session_switcher(frame, model);
        }
        if let Some(prompt) = model.path_prompt {
            draw_path_prompt(frame, area, prompt);
        }
        if let Some(form) = model.host_form {
            draw_host_form(frame, area, form);
        }
        if let Some(unlock) = model.vault_unlock {
            draw_vault_unlock(frame, area, unlock);
        }
        if let Some(offer) = model.os_drop {
            draw_os_drop_confirm(frame, area, offer);
        }
        if let Some(oprompt) = model.overwrite_prompt {
            draw_overwrite_confirm(frame, area, oprompt);
        }
        if let Some(fprompt) = model.files_prompt {
            draw_files_prompt(frame, area, fprompt);
        }
        if model.diagnostics.open {
            draw_diagnostics(frame, area, model.diagnostics);
        }
        if let Some(drag) = model.drag {
            draw_drag_ghost(frame, drag, model.drop_target);
        }
        return;
    }

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(1),
            Constraint::Min(3),
            Constraint::Length(1),
            Constraint::Length(1),
        ])
        .split(area);

    draw_title(frame, chunks[0], model);
    match model.screen {
        ScreenKind::Launcher => draw_launcher(frame, chunks[1], model),
        ScreenKind::Desktop => {
            if let Some(desktop) = model.desktop {
                draw_desktop(frame, chunks[1], desktop, model);
            }
        }
    }
    draw_dock(frame, chunks[2], model);
    draw_status(frame, chunks[3], model);
    if model.show_session_switcher {
        draw_session_switcher(frame, model);
    }
    if let Some(prompt) = model.path_prompt {
        draw_path_prompt(frame, area, prompt);
    }
    if let Some(form) = model.host_form {
        draw_host_form(frame, area, form);
    }
    if let Some(unlock) = model.vault_unlock {
        draw_vault_unlock(frame, area, unlock);
    }
    if let Some(offer) = model.os_drop {
        draw_os_drop_confirm(frame, area, offer);
    }
    if let Some(oprompt) = model.overwrite_prompt {
        draw_overwrite_confirm(frame, area, oprompt);
    }
    if let Some(fprompt) = model.files_prompt {
        draw_files_prompt(frame, area, fprompt);
    }
    if model.diagnostics.open {
        draw_diagnostics(frame, area, model.diagnostics);
    }
    if let Some(drag) = model.drag {
        draw_drag_ghost(frame, drag, model.drop_target);
    }
}

fn draw_title(frame: &mut Frame<'_>, area: Rect, model: &UiFrame<'_>) {
    let mut spans = vec![
        Span::styled(
            " ssh-desk ",
            Style::default()
                .fg(Th::ON_ACCENT)
                .bg(Th::ACCENT)
                .add_modifier(Modifier::BOLD),
        ),
        Span::styled("▌", Style::default().fg(Th::ACCENT).bg(Th::CHROME)),
    ];

    match model.screen {
        ScreenKind::Launcher => {
            spans.push(Span::styled(
                " LAUNCHER ",
                Style::default()
                    .fg(Th::ACCENT)
                    .bg(Th::CHROME)
                    .add_modifier(Modifier::BOLD),
            ));
            spans.push(Span::styled(
                " remote OS shell",
                Style::default()
                    .fg(Th::FG_MUTED)
                    .bg(Th::CHROME),
            ));
        }
        ScreenKind::Desktop => {
            if model.sessions.len() <= 1 {
                let name = model.desktop.map(|d| d.title.as_str()).unwrap_or("session");
                spans.push(Span::styled(
                    " DESKTOP ",
                    Style::default()
                        .fg(Th::ACCENT)
                        .bg(Th::CHROME)
                        .add_modifier(Modifier::BOLD),
                ));
                spans.push(Span::styled(
                    format!(" {name} "),
                    Style::default()
                        .fg(Th::FG)
                        .bg(Th::CHROME)
                        .add_modifier(Modifier::BOLD),
                ));
            } else {
                spans.push(Span::styled(
                    " SESSIONS ",
                    Style::default()
                        .fg(Th::ACCENT)
                        .bg(Th::CHROME)
                        .add_modifier(Modifier::BOLD),
                ));
                for (i, name) in model.sessions.iter().enumerate() {
                    let active = i == model.active_session_idx;
                    let label = format!(" {}:{} ", i + 1, name);
                    let style = if active {
                        Style::default()
                            .fg(Th::ON_ACCENT)
                            .bg(Th::OK)
                            .add_modifier(Modifier::BOLD)
                    } else {
                        Style::default()
                            .fg(Th::FG_DIM)
                            .bg(Th::CHROME)
                    };
                    spans.push(Span::styled(label, style));
                }

                let total_chars: usize = spans.iter().map(|s| s.content.chars().count()).sum();
                if area.width as usize > total_chars + 36 {
                    spans.push(Span::styled(
                        " │ Ctrl+Tab · F8 · Ctrl+W ",
                        Style::default()
                            .fg(Th::FG_DIM)
                            .bg(Th::CHROME),
                    ));
                }
            }
        }
    }

    // Fill remaining title bar so the strip reads as one solid band.
    frame.render_widget(
        Paragraph::new(Line::from(spans)).style(Style::default().bg(Th::CHROME)),
        area,
    );
}

/// Session-switcher overlay (F8).
fn draw_session_switcher(frame: &mut Frame<'_>, model: &UiFrame<'_>) {
    let area = frame.area();
    let width = 40u16.min(area.width.saturating_sub(4));
    let height = (model.sessions.len() as u16 + 4).min(area.height.saturating_sub(4));
    let x = (area.width.saturating_sub(width)) / 2;
    let y = (area.height.saturating_sub(height)) / 2;
    let rect = Rect { x, y, width, height };

    frame.render_widget(Clear, rect);

    let mut items: Vec<ListItem> = model
        .sessions
        .iter()
        .enumerate()
        .map(|(i, name)| {
            let marker = if i == model.active_session_idx { "▶ " } else { "  " };
            let key = if i < 9 { (b'1' + i as u8) as char } else { ' ' };
            let label = format!("{marker}[{key}] {name}");
            let style = if i == model.active_session_idx {
                Style::default()
                    .fg(Th::ON_ACCENT)
                    .bg(Th::OK)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Th::FG)
            };
            ListItem::new(Line::from(Span::styled(label, style)))
        })
        .collect();
    if items.is_empty() {
        items.push(ListItem::new(Line::from("No sessions open")));
    }

    let list = List::new(items).block(
        Block::default()
            .borders(Borders::ALL)
            .title(" Sessions  (j/k·1-9·Enter·F8) ")
            .border_style(Style::default().fg(Th::OK)),
    );
    frame.render_widget(list, rect);
}

fn draw_launcher(frame: &mut Frame<'_>, area: Rect, model: &UiFrame<'_>) {
    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(42), Constraint::Percentage(58)])
        .split(area);

    let items: Vec<ListItem> = model
        .hosts
        .iter()
        .enumerate()
        .map(|(i, h)| {
            let marker = if i == model.selected_host { "●" } else { "○" };
            let jump = h.jump_via.as_deref().map(|j| format!(" via:{j}")).unwrap_or_default();
            let line = format!(
                " {marker} {}  {}@{}:{}{}",
                h.name, h.user, h.host, h.port, jump
            );
            let style = if i == model.selected_host {
                Style::default()
                    .fg(Th::ACCENT)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default()
            };
            ListItem::new(Line::from(Span::styled(line, style)))
        })
        .collect();

    let list = List::new(items).block(
        Block::default()
            .borders(Borders::ALL)
            .title(" Hosts ")
            .border_style(Style::default().fg(Th::FG_DIM)),
    );
    frame.render_widget(list, chunks[0]);

    let help = Paragraph::new(vec![
        Line::from("Enter   connect / open desktop"),
        Line::from("a / n   add host"),
        Line::from("d       delete selected host"),
        Line::from("j/k     move selection"),
        Line::from("r       reload vault"),
        Line::from("q       quit"),
        Line::from(""),
        Line::from("Vault: ~/.config/ssh-desk/hosts.toml"),
        Line::from("Auth: ssh-agent · private key · password"),
        Line::from(""),
        Line::from("After connect: tiled desktop with SFTP files."),
        Line::from("  F2–F7 open/focus pane · Ctrl+W or F10 close pane"),
        Line::from("  Esc closes session · returns here"),
    ])
    .block(
        Block::default()
            .borders(Borders::ALL)
            .title(" Welcome ")
            .border_style(Style::default().fg(Th::FG_DIM)),
    )
    .wrap(Wrap { trim: false });
    frame.render_widget(help, chunks[1]);
}

fn draw_desktop(frame: &mut Frame<'_>, area: Rect, desktop: &Desktop, model: &UiFrame<'_>) {
    if let Some(app) = model.fullscreen_app {
        draw_pane_leaf(frame, area, app, true, false, model);
    } else {
        draw_pane(frame, area, desktop.tree.root_node(), desktop, model);
    }
}

fn draw_pane(
    frame: &mut Frame<'_>,
    area: Rect,
    node: &PaneNode,
    desktop: &Desktop,
    model: &UiFrame<'_>,
) {
    match node {
        PaneNode::Leaf { id, app } => {
            let focused = *id == desktop.tree.focused();
            let drop_hot = pane_is_drop_hot(*app, model.drop_target);
            draw_pane_leaf(frame, area, *app, focused, drop_hot, model);
        }
        PaneNode::Split(split) => {
            let (constraint_a, constraint_b) = ratio_constraints(split.ratio);
            let dir = match split.direction {
                SplitDir::Vertical => Direction::Horizontal,
                SplitDir::Horizontal => Direction::Vertical,
            };
            let chunks = Layout::default()
                .direction(dir)
                .constraints([constraint_a, constraint_b])
                .split(area);
            draw_pane(frame, chunks[0], &split.first, desktop, model);
            draw_pane(frame, chunks[1], &split.second, desktop, model);
        }
    }
}

fn draw_pane_leaf(
    frame: &mut Frame<'_>,
    area: Rect,
    app: AppKind,
    focused: bool,
    drop_hot: bool,
    model: &UiFrame<'_>,
) {
    let border = if drop_hot {
        Style::default()
            .fg(Th::WARN)
            .add_modifier(Modifier::BOLD)
    } else if focused {
        Style::default().fg(Th::ACCENT)
    } else {
        Style::default().fg(Th::FG_DIM)
    };

    // Border only — title lives in a dedicated header strip below.
    let block = Block::default()
        .borders(Borders::ALL)
        .border_style(border)
        .style(Style::default().bg(Th::BG));
    let inner = block.inner(area);
    frame.render_widget(Clear, area);
    frame.render_widget(block, area);

    if inner.height == 0 || inner.width == 0 {
        return;
    }

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(1), Constraint::Min(0)])
        .split(inner);

    draw_pane_header(frame, chunks[0], app, focused, drop_hot, model);
    if chunks[1].height > 0 {
        draw_app_body(frame, chunks[1], app, focused, model);
    }
}

fn draw_pane_header(
    frame: &mut Frame<'_>,
    area: Rect,
    app: AppKind,
    focused: bool,
    drop_hot: bool,
    model: &UiFrame<'_>,
) {
    let (name, detail) = pane_header_parts(app, model);
    let focus_mark = if focused { " ●" } else { "" };

    let (fg, bg) = if drop_hot {
        (Th::ON_ACCENT, Th::WARN)
    } else if focused {
        (Th::ON_ACCENT, Th::ACCENT)
    } else {
        (Th::FG_MUTED, Th::SURFACE)
    };

    let mut spans = vec![Span::styled(
        format!(" {name}{focus_mark} "),
        Style::default()
            .fg(fg)
            .bg(bg)
            .add_modifier(Modifier::BOLD),
    )];

    if !detail.is_empty() {
        spans.push(Span::styled(
            format!(" {detail} "),
            Style::default().fg(Th::FG_DIM).bg(bg),
        ));
    }

    frame.render_widget(
        Paragraph::new(Line::from(spans)).style(Style::default().bg(bg)),
        area,
    );
}

fn pane_header_parts(app: AppKind, model: &UiFrame<'_>) -> (String, String) {
    let name = app.label().to_ascii_uppercase();
    let detail = match app {
        AppKind::Files => model.files.cwd_display(),
        AppKind::Viewer if model.viewer.is_open() => model.viewer.title.clone(),
        AppKind::Editor if model.editor.is_open() => {
            let dirty = if model.editor.dirty { "*" } else { "" };
            format!("{}{dirty}", model.editor.title)
        }
        AppKind::Processes => {
            if model.processes.online {
                format!("{} procs", model.processes.rows.len())
            } else {
                "demo".into()
            }
        }
        AppKind::Transfers => {
            let n = model.transfers.jobs.len();
            if n == 0 {
                String::new()
            } else {
                format!("{n} jobs")
            }
        }
        AppKind::Terminal if model.fullscreen_app == Some(AppKind::Terminal) => {
            "fullscreen".into()
        }
        _ => String::new(),
    };
    (name, detail)
}

fn pane_is_drop_hot(app: AppKind, target: Option<&DropTarget>) -> bool {
    match (app, target) {
        (AppKind::Files, Some(DropTarget::Folder { .. })) => true,
        (AppKind::Transfers, Some(DropTarget::TransferDock)) => true,
        _ => false,
    }
}

fn ratio_constraints(ratio: f32) -> (Constraint, Constraint) {
    let a = ((ratio * 100.0).round() as u16).clamp(15, 85);
    let b = 100 - a;
    (Constraint::Percentage(a), Constraint::Percentage(b))
}

fn draw_app_body(
    frame: &mut Frame<'_>,
    area: Rect,
    app: AppKind,
    focused: bool,
    model: &UiFrame<'_>,
) {
    frame.render_widget(Clear, area); // Wipe pane canvas to prevent layout overlap/residual text leaks
    match app {
        AppKind::Terminal => {
            let lines = model.term.lines();
            frame.render_widget(Paragraph::new(lines), area);
        }
        AppKind::Files => draw_files(frame, area, focused, model.files, model.drop_target),
        AppKind::Processes => draw_processes(frame, area, focused, model.processes),
        AppKind::Transfers => draw_transfers(frame, area, focused, model.transfers),
        AppKind::Viewer => draw_viewer(frame, area, model.viewer),
        AppKind::Editor => draw_editor(frame, area, focused, model.editor),
        AppKind::Launcher => {}
    }
}

fn draw_files(
    frame: &mut Frame<'_>,
    area: Rect,
    focused: bool,
    files: &FilesState,
    drop_target: Option<&DropTarget>,
) {
    let drop_path = match drop_target {
        Some(DropTarget::Folder { path, .. }) => Some(path.as_path()),
        _ => None,
    };

    let mut lines: Vec<Line> = Vec::new();
    if let Some(err) = &files.error {
        lines.push(Line::from(Span::styled(
            err.clone(),
            Style::default().fg(Th::WARN),
        )));
    }
    if files.loading {
        lines.push(Line::from("loading…"));
    }

    lines.push(Line::from(Span::styled(
        format!(
            "  {:<10} {:>8}  {:<16}  {}",
            "MODE", "SIZE", "MODIFIED", "NAME"
        ),
        Style::default()
            .fg(Th::FG_DIM)
            .add_modifier(Modifier::BOLD),
    )));

    let prefix = lines.len();
    let help_reserve = if focused { 1usize } else { 0 };
    let rows = files.rows();
    let visible = area
        .height
        .saturating_sub(prefix as u16)
        .saturating_sub(help_reserve as u16) as usize;
    let start = files.selected.saturating_sub(visible.saturating_sub(1));

    for (idx, row) in rows.iter().enumerate().skip(start).take(visible) {
        let (label, row_path, is_dir) = match row {
            FilesRow::Parent => {
                let p = files
                    .cwd
                    .parent()
                    .map(|p| p.to_path_buf())
                    .unwrap_or_else(|| PathBuf::from("/"));
                (
                    format!("  {:<10} {:>8}  {:<16}  ../", "", "—", "—"),
                    Some(p),
                    true,
                )
            }
            FilesRow::Entry(i) => files
                .entries
                .get(*i)
                .map(|e| {
                    let mark = if files.is_marked(*i) { "* " } else { "  " };
                    (
                        format!(
                            "{mark}{:<10} {:>8}  {:<16}  {}",
                            e.mode_string(),
                            e.size_label(),
                            e.mtime_label(),
                            e.display_name()
                        ),
                        Some(e.path.clone()),
                        e.is_dir,
                    )
                })
                .unwrap_or_else(|| (String::new(), None, false)),
        };
        let selected = idx == files.selected;
        let is_drop = drop_path.is_some_and(|dp| row_path.as_ref().is_some_and(|rp| rp == dp));
        let style = if is_drop {
            Style::default()
                .fg(Th::ON_ACCENT)
                .bg(Th::WARN)
                .add_modifier(Modifier::BOLD)
        } else if selected && focused {
            Style::default()
                .fg(Th::ON_ACCENT)
                .bg(Th::ACCENT)
                .add_modifier(Modifier::BOLD)
        } else if selected {
            Style::default().fg(Th::ACCENT)
        } else if is_dir {
            Style::default().fg(Th::ACCENT_2)
        } else {
            Style::default().fg(Th::FG)
        };
        lines.push(Line::from(Span::styled(label, style)));
    }

    if rows.is_empty() && files.error.is_none() && !files.loading {
        lines.push(Line::from(Span::styled(
            "  (empty)  ·  a mkdir · R rename · d delete",
            Style::default().fg(Th::FG_DIM),
        )));
    }

    if focused {
        let help = if files.search_query.is_some() {
            "searching · Backspace edit · Esc cancel query · Enter lock"
        } else {
            "/ search · a mkdir · R rename · d delete · Space mark · Ctrl+C/X/V"
        };
        lines.push(Line::from(Span::styled(
            help,
            Style::default().fg(Th::FG_DIM),
        )));
    }

    frame.render_widget(Paragraph::new(lines), area);
}

fn draw_transfers(frame: &mut Frame<'_>, area: Rect, focused: bool, transfers: &TransfersUi) {
    let mut lines: Vec<Line> = Vec::new();
    if transfers.jobs.is_empty() {
        lines.push(Line::from("queue empty"));
        lines.push(Line::from(""));
        lines.push(Line::from("Ctrl+U / u  upload"));
        lines.push(Line::from("Ctrl+D / d  download"));
    } else {
        for (idx, job) in transfers.jobs.iter().enumerate() {
            let selected = idx == transfers.selected;
            let pct = job
                .progress_pct()
                .map(|p| format!("{p:5.1}%"))
                .unwrap_or_else(|| "  —  ".into());
            let bar = progress_bar(job.progress_pct().unwrap_or(0.0), 10);
            let line = format!(
                "{} {} {} {} {}",
                job.direction.arrow(),
                job.display_name(),
                bar,
                pct,
                job.status.label()
            );
            let style = if selected && focused {
                Style::default()
                    .fg(Th::ON_ACCENT)
                    .bg(Th::ACCENT)
                    .add_modifier(Modifier::BOLD)
            } else if selected {
                Style::default().fg(Th::ACCENT)
            } else {
                match job.status {
                    ssh_core::TransferStatus::Failed => Style::default().fg(Th::ERR),
                    ssh_core::TransferStatus::Done => Style::default().fg(Th::OK),
                    ssh_core::TransferStatus::Running => Style::default().fg(Th::WARN),
                    _ => Style::default(),
                }
            };
            lines.push(Line::from(Span::styled(line, style)));
            if selected {
                let detail = format!(
                    "  {} {}  {}",
                    ssh_core::format_bytes(job.bytes_done),
                    job.remote_path.display(),
                    if job.bytes_per_sec > 0.0 {
                        ssh_core::format_rate(job.bytes_per_sec)
                    } else {
                        String::new()
                    }
                );
                lines.push(Line::from(Span::styled(
                    detail,
                    Style::default().fg(Th::FG_DIM),
                )));
                if let Some(err) = &job.error {
                    lines.push(Line::from(Span::styled(
                        format!("  {err}"),
                        Style::default().fg(Th::ERR),
                    )));
                }
            }
        }
    }
    if focused {
        lines.push(Line::from(Span::styled(
            "c cancel · r retry · u/d queue",
            Style::default().fg(Th::FG_DIM),
        )));
    }
    frame.render_widget(Paragraph::new(lines), area);
}

fn progress_bar(pct: f64, width: usize) -> String {
    let filled = ((pct / 100.0) * width as f64).round() as usize;
    let filled = filled.min(width);
    let mut s = String::from("[");
    for i in 0..width {
        s.push(if i < filled { '#' } else { '-' });
    }
    s.push(']');
    s
}

fn draw_host_form(frame: &mut Frame<'_>, area: Rect, form: &HostForm) {
    let width = area.width.min(64).max(44);
    let height = area.height.min(16).max(12);
    let x = area.x + (area.width.saturating_sub(width)) / 2;
    let y = area.y + (area.height.saturating_sub(height)) / 2;
    let rect = Rect {
        x,
        y,
        width,
        height,
    };

    frame.render_widget(Clear, rect);
    let block = Block::default()
        .borders(Borders::ALL)
        .title(" Add host ")
        .border_style(Style::default().fg(Th::ACCENT));
    let inner = block.inner(rect);
    frame.render_widget(block, rect);

    let mut lines = Vec::new();
    for field in form.active_fields() {
        let focused = *field == form.focus;
        let value = match field {
            HostField::Name => form.name.as_str(),
            HostField::Host => form.host.as_str(),
            HostField::Port => form.port.as_str(),
            HostField::User => form.user.as_str(),
            HostField::Auth => form.auth.label(),
            HostField::KeyPath => form.key_path.as_str(),
            HostField::Password => {
                if form.password.is_empty() {
                    ""
                } else {
                    "••••••••"
                }
            }
            HostField::VaultPass => {
                if form.vault_pass.is_empty() {
                    ""
                } else {
                    "••••••••"
                }
            }
        };
        let hint = if *field == HostField::Auth {
            "  (Space cycle)"
        } else {
            ""
        };
        let label = format!("{:<10} {}", format!("{}:", field.label()), value);
        let style = if focused {
            Style::default()
                .fg(Th::ON_ACCENT)
                .bg(Th::ACCENT)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default()
        };
        lines.push(Line::from(Span::styled(format!("{label}{hint}"), style)));
    }
    if let Some(err) = &form.error {
        lines.push(Line::from(""));
        lines.push(Line::from(Span::styled(
            err.clone(),
            Style::default().fg(Th::ERR),
        )));
    }
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        "Tab next · Ctrl+S save · Esc cancel",
        Style::default().fg(Th::FG_DIM),
    )));

    frame.render_widget(Paragraph::new(lines), inner);
}

fn draw_vault_unlock(frame: &mut Frame<'_>, area: Rect, prompt: &VaultUnlockPrompt) {
    let width = area.width.min(52).max(36);
    let height = 7u16;
    let x = area.x + (area.width.saturating_sub(width)) / 2;
    let y = area.y + (area.height.saturating_sub(height)) / 2;
    let rect = Rect {
        x,
        y,
        width,
        height,
    };

    frame.render_widget(Clear, rect);
    let block = Block::default()
        .borders(Borders::ALL)
        .title(format!(" Unlock vault · {} ", prompt.host_name))
        .border_style(Style::default().fg(Th::WARN));
    let inner = block.inner(rect);
    frame.render_widget(block, rect);

    if prompt.connecting {
        let lines = vec![
            Line::from(""),
            Line::from(Span::styled(
                "  connecting... please wait",
                Style::default()
                    .fg(Th::ACCENT)
                    .add_modifier(Modifier::BOLD),
            )),
            Line::from(""),
        ];
        frame.render_widget(Paragraph::new(lines), inner);
        return;
    }

    let masked: String = std::iter::repeat_n('•', prompt.buffer.chars().count()).collect();
    let mut lines = vec![
        Line::from("Enter the vault passphrase used when saving this host."),
        Line::from(Span::styled(
            format!("> {masked}"),
            Style::default()
                .fg(Th::ON_ACCENT)
                .bg(Th::WARN)
                .add_modifier(Modifier::BOLD),
        )),
    ];
    if let Some(err) = &prompt.error {
        lines.push(Line::from(Span::styled(
            err.clone(),
            Style::default().fg(Th::ERR),
        )));
    }
    lines.push(Line::from(Span::styled(
        "Enter connect · Esc cancel",
        Style::default().fg(Th::FG_DIM),
    )));
    frame.render_widget(Paragraph::new(lines), inner);
}

fn draw_path_prompt(frame: &mut Frame<'_>, area: Rect, prompt: &PathPrompt) {
    let width = area.width.min(72).max(40);
    let height = area.height.min(18).max(10);
    let x = area.x + (area.width.saturating_sub(width)) / 2;
    let y = area.y + (area.height.saturating_sub(height)) / 2;
    let rect = Rect {
        x,
        y,
        width,
        height,
    };

    frame.render_widget(Clear, rect);
    let block = Block::default()
        .borders(Borders::ALL)
        .title(format!(" {} ", prompt.title))
        .border_style(Style::default().fg(Th::ACCENT));
    let inner = block.inner(rect);
    frame.render_widget(block, rect);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Length(2),
            Constraint::Min(3),
            Constraint::Length(2),
        ])
        .split(inner);

    let mode = if prompt.editing {
        "path edit"
    } else {
        "browse"
    };
    let kind = match prompt.kind {
        PathPromptKind::Upload => "upload local file",
        PathPromptKind::Download => "save download as",
        PathPromptKind::CopyLocal => "copy local to clipboard",
    };
    frame.render_widget(
        Paragraph::new(vec![
            Line::from(format!("{kind}  [{mode}]")),
            Line::from(Span::styled(
                format!("> {}", prompt.buffer),
                if prompt.editing {
                    Style::default()
                        .fg(Th::ON_ACCENT)
                        .bg(Th::ACCENT)
                } else {
                    Style::default().fg(Th::FG)
                },
            )),
        ]),
        chunks[0],
    );

    let mut lines = vec![Line::from(Span::styled(
        format!("local: {}", prompt.browse_cwd.display()),
        Style::default().fg(Th::WARN),
    ))];
    if let Some(err) = &prompt.error {
        lines.push(Line::from(Span::styled(
            err.clone(),
            Style::default().fg(Th::ERR),
        )));
    }
    let visible = chunks[1].height.saturating_sub(1) as usize;
    let start = prompt
        .browse_selected
        .saturating_sub(visible.saturating_sub(1));
    for (idx, entry) in prompt
        .browse_entries
        .iter()
        .enumerate()
        .skip(start)
        .take(visible)
    {
        let label = if entry.is_dir {
            format!("{}/", entry.name)
        } else {
            entry.name.clone()
        };
        let selected = idx == prompt.browse_selected && !prompt.editing;
        let style = if selected {
            Style::default()
                .fg(Th::ON_ACCENT)
                .bg(Th::ACCENT)
                .add_modifier(Modifier::BOLD)
        } else if entry.is_dir {
            Style::default().fg(Th::WARN)
        } else {
            Style::default()
        };
        lines.push(Line::from(Span::styled(format!("  {label}"), style)));
    }
    frame.render_widget(Paragraph::new(lines), chunks[1]);

    frame.render_widget(
        Paragraph::new(match prompt.kind {
            PathPromptKind::Upload => {
                "Enter pick file · Tab/e edit path · Esc cancel".to_string()
            }
            PathPromptKind::Download => {
                "Enter overwrite file · s save here · Tab/e edit · Esc".to_string()
            }
            PathPromptKind::CopyLocal => {
                "Enter copy file to clipboard · Tab/e edit · Esc".to_string()
            }
        }),
        chunks[2],
    );
}

fn draw_os_drop_confirm(frame: &mut Frame<'_>, area: Rect, offer: &OsDropOffer) {
    let width = area.width.min(64).max(40);
    let height = 10u16.min(area.height).max(8);
    let x = area.x + (area.width.saturating_sub(width)) / 2;
    let y = area.y + (area.height.saturating_sub(height)) / 2;
    let rect = Rect {
        x,
        y,
        width,
        height,
    };

    frame.render_widget(Clear, rect);
    let block = Block::default()
        .borders(Borders::ALL)
        .title(" OS file drop ")
        .border_style(Style::default().fg(Th::WARN));
    let inner = block.inner(rect);
    frame.render_widget(block, rect);

    let mut lines = vec![
        Line::from(Span::styled(
            offer.summary(),
            Style::default().add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
    ];
    for (i, path) in offer.paths.iter().take(4).enumerate() {
        lines.push(Line::from(format!("  {}. {}", i + 1, path.display())));
    }
    if offer.paths.len() > 4 {
        lines.push(Line::from(format!("  … +{} more", offer.paths.len() - 4)));
    }
    lines.push(Line::from(""));

    let upload_style = if offer.selected == 0 {
        Style::default()
            .fg(Th::ON_ACCENT)
            .bg(Th::WARN)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Th::FG_MUTED)
    };
    let cancel_style = if offer.selected == 1 {
        Style::default()
            .fg(Th::ON_ACCENT)
            .bg(Th::FG_DIM)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Th::FG_MUTED)
    };
    lines.push(Line::from(vec![
        Span::styled(" Upload ", upload_style),
        Span::raw("  "),
        Span::styled(" Cancel ", cancel_style),
    ]));
    lines.push(Line::from(Span::styled(
        "Enter/y confirm · Tab switch · Esc cancel",
        Style::default().fg(Th::FG_DIM),
    )));

    frame.render_widget(Paragraph::new(lines), inner);
}

fn draw_diagnostics(frame: &mut Frame<'_>, area: Rect, diag: &DiagnosticsState) {
    let width = area.width.min(90).max(48);
    let height = area.height.min(22).max(10);
    let x = area.x + (area.width.saturating_sub(width)) / 2;
    let y = area.y + (area.height.saturating_sub(height)) / 2;
    let rect = Rect {
        x,
        y,
        width,
        height,
    };

    frame.render_widget(Clear, rect);
    let block = Block::default()
        .borders(Borders::ALL)
        .title(format!(
            " Diagnostics · {} entries · F9/Esc close ",
            diag.entries.len()
        ))
        .border_style(Style::default().fg(Th::ERR));
    let inner = block.inner(rect);
    frame.render_widget(block, rect);

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(3), Constraint::Length(1)])
        .split(inner);

    let visible = chunks[0].height as usize;
    let total = diag.entries.len();
    let end = total.saturating_sub(diag.scroll_from_bottom as usize);
    let start = end.saturating_sub(visible);
    let slice = diag.entries.get(start..end).unwrap_or(&[]);

    let mut lines = Vec::new();
    if slice.is_empty() {
        lines.push(Line::from("(no entries)"));
    }
    for entry in slice {
        let color = match entry.level {
            DiagLevel::Info => Th::FG_MUTED,
            DiagLevel::Warn => Th::WARN,
            DiagLevel::Error => Th::ERR,
        };
        // Compact clock: last 5 digits of epoch seconds is enough as relative marker.
        let clock = entry.ts_secs % 100_000;
        lines.push(Line::from(vec![
            Span::styled(
                format!("{clock:05} ",),
                Style::default().fg(Th::FG_DIM),
            ),
            Span::styled(
                format!("[{}] ", entry.level.tag()),
                Style::default().fg(color).add_modifier(Modifier::BOLD),
            ),
            Span::styled(entry.message.clone(), Style::default().fg(color)),
        ]));
    }
    frame.render_widget(Paragraph::new(lines), chunks[0]);
    frame.render_widget(
        Paragraph::new(Span::styled(
            "j/k scroll · PgUp/PgDn · Home/End · c clear",
            Style::default().fg(Th::FG_DIM),
        )),
        chunks[1],
    );
}

fn draw_overwrite_confirm(frame: &mut Frame<'_>, area: Rect, prompt: &OverwritePrompt) {
    let width = area.width.min(64).max(40);
    let height = 10u16.min(area.height).max(8);
    let x = area.x + (area.width.saturating_sub(width)) / 2;
    let y = area.y + (area.height.saturating_sub(height)) / 2;
    let rect = Rect {
        x,
        y,
        width,
        height,
    };

    frame.render_widget(Clear, rect);
    let block = Block::default()
        .borders(Borders::ALL)
        .title(format!(" {} ", prompt.title))
        .border_style(Style::default().fg(Th::ERR));
    let inner = block.inner(rect);
    frame.render_widget(block, rect);

    let mut lines = vec![
        Line::from(Span::styled(
            "The following file(s) already exist on the target folder. Overwrite?",
            Style::default().fg(Th::ERR).add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
    ];
    for (i, name) in prompt.files.iter().take(4).enumerate() {
        lines.push(Line::from(format!("  {}. {}", i + 1, name)));
    }
    if prompt.files.len() > 4 {
        lines.push(Line::from(format!("  … +{} more", prompt.files.len() - 4)));
    }
    lines.push(Line::from(""));

    let yes_style = if prompt.selected == 0 {
        Style::default()
            .fg(Th::ON_ACCENT)
            .bg(Th::ERR)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Th::FG_MUTED)
    };
    let no_style = if prompt.selected == 1 {
        Style::default()
            .fg(Th::ON_ACCENT)
            .bg(Th::FG_DIM)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Th::FG_MUTED)
    };
    lines.push(Line::from(vec![
        Span::styled(" Yes, Overwrite ", yes_style),
        Span::raw("  "),
        Span::styled(" No, Cancel ", no_style),
    ]));
    lines.push(Line::from(Span::styled(
        "Enter confirm · Tab switch · Esc cancel",
        Style::default().fg(Th::FG_DIM),
    )));

    frame.render_widget(Paragraph::new(lines), inner);
}

fn draw_files_prompt(frame: &mut Frame<'_>, area: Rect, prompt: &FilesPrompt) {
    let width = area.width.min(64).max(40);
    let height = match prompt {
        FilesPrompt::Delete { names, .. } => (9 + names.len().min(4) as u16).min(area.height).max(8),
        _ => 9u16.min(area.height).max(7),
    };
    let x = area.x + (area.width.saturating_sub(width)) / 2;
    let y = area.y + (area.height.saturating_sub(height)) / 2;
    let rect = Rect {
        x,
        y,
        width,
        height,
    };

    frame.render_widget(Clear, rect);
    let border_fg = match prompt {
        FilesPrompt::Delete { .. } => Th::ERR,
        _ => Th::ACCENT,
    };
    let block = Block::default()
        .borders(Borders::ALL)
        .title(format!(" {} ", prompt.title()))
        .border_style(Style::default().fg(border_fg));
    let inner = block.inner(rect);
    frame.render_widget(block, rect);

    let mut lines: Vec<Line> = Vec::new();
    match prompt {
        FilesPrompt::Mkdir { buffer, error } => {
            lines.push(Line::from(Span::styled(
                "Folder name",
                Style::default().fg(Th::FG_MUTED),
            )));
            lines.push(Line::from(format!("  {buffer}█")));
            if let Some(err) = error {
                lines.push(Line::from(""));
                lines.push(Line::from(Span::styled(
                    err.clone(),
                    Style::default().fg(Th::ERR),
                )));
            }
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                "Enter create · Esc cancel",
                Style::default().fg(Th::FG_DIM),
            )));
        }
        FilesPrompt::Rename { buffer, error, .. } => {
            lines.push(Line::from(Span::styled(
                "New name",
                Style::default().fg(Th::FG_MUTED),
            )));
            lines.push(Line::from(format!("  {buffer}█")));
            if let Some(err) = error {
                lines.push(Line::from(""));
                lines.push(Line::from(Span::styled(
                    err.clone(),
                    Style::default().fg(Th::ERR),
                )));
            }
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                "Enter rename · Esc cancel",
                Style::default().fg(Th::FG_DIM),
            )));
        }
        FilesPrompt::Delete {
            names,
            selected,
            ..
        } => {
            lines.push(Line::from(Span::styled(
                "Permanently delete the following?",
                Style::default().fg(Th::ERR).add_modifier(Modifier::BOLD),
            )));
            lines.push(Line::from(""));
            for (i, name) in names.iter().take(4).enumerate() {
                lines.push(Line::from(format!("  {}. {name}", i + 1)));
            }
            if names.len() > 4 {
                lines.push(Line::from(format!("  … +{} more", names.len() - 4)));
            }
            lines.push(Line::from(""));

            let yes_style = if *selected == 0 {
                Style::default()
                    .fg(Th::ON_ACCENT)
                    .bg(Th::ERR)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Th::FG_MUTED)
            };
            let no_style = if *selected == 1 {
                Style::default()
                    .fg(Th::ON_ACCENT)
                    .bg(Th::FG_DIM)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(Th::FG_MUTED)
            };
            lines.push(Line::from(vec![
                Span::styled(" Yes, Delete ", yes_style),
                Span::raw("  "),
                Span::styled(" No, Cancel ", no_style),
            ]));
            lines.push(Line::from(Span::styled(
                "Enter confirm · Tab switch · Esc cancel",
                Style::default().fg(Th::FG_DIM),
            )));
        }
    }

    frame.render_widget(Paragraph::new(lines), inner);
}

fn draw_drag_ghost(frame: &mut Frame<'_>, drag: &DragSession, target: Option<&DropTarget>) {
    let label = match &drag.payload {
        DragPayload::Files(files) => {
            let name = files
                .first()
                .and_then(|f| match &f.location {
                    ssh_os::FileLocation::Remote { path, .. }
                    | ssh_os::FileLocation::Local { path } => path
                        .file_name()
                        .map(|s| s.to_string_lossy().into_owned()),
                })
                .unwrap_or_else(|| "file".into());
            if files.len() > 1 {
                format!(" {} (+{}) ", name, files.len() - 1)
            } else {
                format!(" {name} ")
            }
        }
        DragPayload::OsPaths(paths) => format!(" {} path(s) ", paths.len()),
    };
    let hint = target.map(DropTarget::describe).unwrap_or_default();
    let text = if hint.is_empty() {
        label
    } else {
        format!("{label}→ {hint} ")
    };

    let width = (text.chars().count() as u16).clamp(8, 48);
    let (x, y) = drag.current;
    let area = frame.area();
    let gx = x.min(area.width.saturating_sub(width));
    let gy = y.min(area.height.saturating_sub(1));
    let rect = Rect {
        x: gx,
        y: gy,
        width,
        height: 1,
    };
    frame.render_widget(Clear, rect);
    frame.render_widget(
        Paragraph::new(text).style(
            Style::default()
                .fg(Th::ON_ACCENT)
                .bg(Th::WARN)
                .add_modifier(Modifier::BOLD),
        ),
        rect,
    );
}

fn draw_viewer(frame: &mut Frame<'_>, area: Rect, viewer: &ViewerState) {
    if !viewer.is_open() {
        frame.render_widget(
            Paragraph::new(
                "Open a file from Files (Enter).\nImages use half-block preview.\ne open in editor · Esc closes.",
            ),
            area,
        );
        return;
    }

    if let ViewerKind::Image(preview) = &viewer.kind {
        let mut lines = vec![Line::from(Span::styled(
            format!("{}  {}", viewer.title, preview.meta),
            Style::default()
                .fg(Th::INFO)
                .add_modifier(Modifier::BOLD),
        ))];
        let max = area.height.saturating_sub(1) as usize;
        let start = (viewer.scroll as usize).min(preview.rows.len().saturating_sub(1));
        for row in preview.rows.iter().skip(start).take(max) {
            let spans: Vec<Span> = row
                .iter()
                .map(|cell| {
                    Span::styled(
                        "▀",
                        Style::default().fg(cell.fg).bg(cell.bg),
                    )
                })
                .collect();
            lines.push(Line::from(spans));
        }
        frame.render_widget(Paragraph::new(lines), area);
        return;
    }

    let mut header = viewer.title.clone();
    match viewer.kind {
        ViewerKind::Hex => header.push_str("  [hex]"),
        ViewerKind::Text => {}
        ViewerKind::Image(_) => {}
    }
    if viewer.truncated {
        header.push_str("  [truncated]");
    }

    let body_lines: Vec<&str> = viewer.body.lines().collect();
    let max = area.height.saturating_sub(1) as usize;
    let start = (viewer.scroll as usize).min(body_lines.len().saturating_sub(1));
    let slice = body_lines
        .get(start..start.saturating_add(max).min(body_lines.len()))
        .unwrap_or(&[]);

    let mut lines = vec![Line::from(Span::styled(
        header,
        Style::default()
            .fg(Th::INFO)
            .add_modifier(Modifier::BOLD),
    ))];
    for line in slice {
        lines.push(Line::from(line.to_string()));
    }

    frame.render_widget(Paragraph::new(lines), area);
}

fn draw_editor(frame: &mut Frame<'_>, area: Rect, focused: bool, editor: &EditorState) {
    if !editor.is_open() {
        frame.render_widget(
            Paragraph::new(
                "Open text with Enter (or e) from Files, or press e in Viewer.\nCtrl+S save · Esc close.",
            ),
            area,
        );
        return;
    }

    let dirty = if editor.dirty { " *" } else { "" };
    let mode = if editor.online { "" } else { " [demo]" };
    let header = format!(
        "{}{}{}  · Ctrl+S save · Esc",
        editor.title, dirty, mode
    );
    let mut lines = vec![Line::from(Span::styled(
        header,
        Style::default()
            .fg(Th::WARN)
            .add_modifier(Modifier::BOLD),
    ))];

    let max = area.height.saturating_sub(1) as usize;
    let start = editor.scroll as usize;
    for (i, line) in editor.lines.iter().enumerate().skip(start).take(max) {
        let row = i;
        let on_cursor = focused && row == editor.cursor_row;
        if on_cursor {
            let chars: Vec<char> = line.chars().collect();
            let col = editor.cursor_col.min(chars.len());
            let mut spans = Vec::new();
            let before: String = chars[..col].iter().collect();
            if !before.is_empty() {
                spans.push(Span::raw(before));
            }
            let ch = chars.get(col).copied().unwrap_or(' ');
            spans.push(Span::styled(
                ch.to_string(),
                Style::default()
                    .fg(Th::ON_ACCENT)
                    .bg(Th::ACCENT)
                    .add_modifier(Modifier::BOLD),
            ));
            if col < chars.len() {
                let after: String = chars[col + 1..].iter().collect();
                if !after.is_empty() {
                    spans.push(Span::raw(after));
                }
            }
            lines.push(Line::from(spans));
        } else {
            lines.push(Line::from(line.clone()));
        }
    }

    frame.render_widget(Paragraph::new(lines), area);
}

fn draw_processes(frame: &mut Frame<'_>, area: Rect, focused: bool, procs: &ProcessesState) {
    let mut lines = Vec::new();
    if let Some(err) = &procs.error {
        lines.push(Line::from(Span::styled(
            err.clone(),
            Style::default().fg(Th::WARN),
        )));
    }
    if procs.loading {
        lines.push(Line::from("loading…"));
    }
    lines.push(Line::from(Span::styled(
        format!("{:<7} {:<8} {:>5} {:>5}  COMMAND", "PID", "USER", "%CPU", "%MEM"),
        Style::default()
            .fg(Th::FG_DIM)
            .add_modifier(Modifier::BOLD),
    )));

    let visible = area.height.saturating_sub(lines.len() as u16) as usize;
    let start = procs.selected.saturating_sub(visible.saturating_sub(1));
    for (idx, row) in procs.rows.iter().enumerate().skip(start).take(visible) {
        let label = format!(
            "{:<7} {:<8} {:>5} {:>5}  {}",
            row.pid, row.user, row.cpu, row.mem, row.command
        );
        let style = if idx == procs.selected && focused {
            Style::default()
                .fg(Th::ON_ACCENT)
                .bg(Th::ACCENT)
                .add_modifier(Modifier::BOLD)
        } else if idx == procs.selected {
            Style::default().fg(Th::ACCENT)
        } else {
            Style::default()
        };
        lines.push(Line::from(Span::styled(label, style)));
    }
    if procs.rows.is_empty() && procs.error.is_none() {
        lines.push(Line::from("no processes · F4 / r to refresh"));
    }

    frame.render_widget(Paragraph::new(lines), area);
}

fn draw_dock(frame: &mut Frame<'_>, area: Rect, model: &UiFrame<'_>) {
    let focused = model
        .desktop
        .map(|d| d.focused_app())
        .unwrap_or(AppKind::Launcher);
    let open_apps: Vec<AppKind> = model
        .desktop
        .map(|d| d.tree.leaves().into_iter().map(|(_, a)| a).collect())
        .unwrap_or_default();

    let dock_hot = matches!(model.drop_target, Some(DropTarget::TransferDock));
    let mut spans = vec![Span::styled(
        " DOCK ",
        if dock_hot {
            Style::default()
                .fg(Th::ON_ACCENT)
                .bg(Th::WARN)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default()
                .fg(Th::FG_DIM)
                .bg(Th::CHROME)
                .add_modifier(Modifier::BOLD)
        },
    )];
    for app in AppKind::all_dock() {
        let is_open = open_apps.contains(app);
        let active = model.screen == ScreenKind::Desktop && focused == *app && is_open;
        let style = if active {
            Style::default()
                .fg(Th::ON_ACCENT)
                .bg(Th::ACCENT)
                .add_modifier(Modifier::BOLD)
        } else if is_open {
            Style::default()
                .fg(Th::FG_MUTED)
                .bg(Th::CHROME)
        } else {
            // Closed / available to open
            Style::default()
                .fg(Th::FG_DIM)
                .bg(Th::CHROME)
                .add_modifier(Modifier::DIM)
        };
        let label = if is_open {
            format!(" {} ", app.label().to_ascii_uppercase())
        } else {
            format!(" [{}] ", app.label().to_ascii_uppercase())
        };
        spans.push(Span::styled(label, style));
    }
    if model.clipboard_has_files {
        spans.push(Span::styled(
            format!(" [clipboard:{}] ", model.clipboard_label),
            Style::default()
                .fg(Th::INFO)
                .bg(Th::CHROME),
        ));
    }
    frame.render_widget(
        Paragraph::new(Line::from(spans)).style(Style::default().bg(Th::CHROME)),
        area,
    );
}

fn draw_status(frame: &mut Frame<'_>, area: Rect, model: &UiFrame<'_>) {
    let help = match model.screen {
        ScreenKind::Launcher => "a add · F9 log · Enter connect · q quit",
        ScreenKind::Desktop => {
            "F2–F7 open/focus · Ctrl+W / F10 close pane · Esc session"
        }
    };
    let line = Line::from(vec![
        Span::styled(
            format!(" {} ", model.status),
            Style::default().fg(Th::FG),
        ),
        Span::styled("│ ", Style::default().fg(Th::FG_DIM)),
        Span::styled(help, Style::default().fg(Th::FG_DIM)),
    ]);
    frame.render_widget(
        Paragraph::new(line).style(Style::default().bg(Th::CHROME)),
        area,
    );
}
