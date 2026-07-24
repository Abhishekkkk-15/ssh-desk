//! ratatui views for launcher and desktop OS shell.

use std::path::PathBuf;

use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, Paragraph, Wrap};
use ratatui::Frame;
use ssh_os::{DragPayload, DragSession, DropTarget};
use ssh_vault::HostProfile;
use ssh_wm::{AppKind, Desktop, Direction as SplitDir, PaneNode};

use crate::files::{FilesRow, FilesState, ViewerState};
use crate::transfers::{PathPrompt, PathPromptKind, TransfersUi};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScreenKind {
    Launcher,
    Desktop,
}

pub struct UiFrame<'a> {
    pub screen: ScreenKind,
    pub hosts: &'a [HostProfile],
    pub selected_host: usize,
    pub desktop: Option<&'a Desktop>,
    pub status: &'a str,
    pub term_buffer: &'a str,
    pub clipboard_has_files: bool,
    pub clipboard_label: &'a str,
    pub files: &'a FilesState,
    pub viewer: &'a ViewerState,
    pub transfers: &'a TransfersUi,
    pub path_prompt: Option<&'a PathPrompt>,
    pub drag: Option<&'a DragSession>,
    pub drop_target: Option<&'a DropTarget>,
}

pub fn draw(frame: &mut Frame<'_>, model: &UiFrame<'_>) {
    let area = frame.area();
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
    if let Some(prompt) = model.path_prompt {
        draw_path_prompt(frame, area, prompt);
    }
    if let Some(drag) = model.drag {
        draw_drag_ghost(frame, drag, model.drop_target);
    }
}

fn draw_title(frame: &mut Frame<'_>, area: Rect, model: &UiFrame<'_>) {
    let title = match model.screen {
        ScreenKind::Launcher => Line::from(vec![
            Span::styled(
                " ssh-desk ",
                Style::default()
                    .fg(Color::Black)
                    .bg(Color::Cyan)
                    .add_modifier(Modifier::BOLD),
            ),
            Span::raw(" remote OS shell  ·  launcher"),
        ]),
        ScreenKind::Desktop => {
            let name = model
                .desktop
                .map(|d| d.title.as_str())
                .unwrap_or("session");
            Line::from(vec![
                Span::styled(
                    " ssh-desk ",
                    Style::default()
                        .fg(Color::Black)
                        .bg(Color::Cyan)
                        .add_modifier(Modifier::BOLD),
                ),
                Span::raw(format!(" desktop · {name}")),
            ])
        }
    };
    frame.render_widget(Paragraph::new(title), area);
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
            let line = format!(
                " {marker} {}  {}@{}:{}",
                h.name, h.user, h.host, h.port
            );
            let style = if i == model.selected_host {
                Style::default()
                    .fg(Color::Cyan)
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
            .border_style(Style::default().fg(Color::DarkGray)),
    );
    frame.render_widget(list, chunks[0]);

    let help = Paragraph::new(vec![
        Line::from("Enter   connect / open desktop"),
        Line::from("j/k     move selection"),
        Line::from("r       reload vault"),
        Line::from("q       quit"),
        Line::from(""),
        Line::from("Vault: ~/.config/ssh-desk/hosts.toml"),
        Line::from("Auth prefers ssh-agent, then private key."),
        Line::from(""),
        Line::from("After connect: tiled desktop with SFTP files."),
        Line::from("  F2 files · Enter open · F6 viewer"),
        Line::from("Esc returns here from a session."),
    ])
    .block(
        Block::default()
            .borders(Borders::ALL)
            .title(" Welcome ")
            .border_style(Style::default().fg(Color::DarkGray)),
    )
    .wrap(Wrap { trim: false });
    frame.render_widget(help, chunks[1]);
}

fn draw_desktop(frame: &mut Frame<'_>, area: Rect, desktop: &Desktop, model: &UiFrame<'_>) {
    draw_pane(frame, area, desktop.tree.root_node(), desktop, model);
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
            let border = if drop_hot {
                Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD)
            } else if focused {
                Style::default().fg(Color::Cyan)
            } else {
                Style::default().fg(Color::DarkGray)
            };
            let title = match app {
                AppKind::Files => format!(
                    " files {}{} ",
                    model.files.cwd_display(),
                    if focused { " ●" } else { "" }
                ),
                AppKind::Viewer => format!(
                    " viewer{}{} ",
                    if model.viewer.is_open() {
                        format!(" · {}", model.viewer.title)
                    } else {
                        String::new()
                    },
                    if focused { " ●" } else { "" }
                ),
                _ => format!(" {}{} ", app.label(), if focused { " ●" } else { "" }),
            };
            let block = Block::default()
                .borders(Borders::ALL)
                .title(title)
                .border_style(border);
            let inner = block.inner(area);
            frame.render_widget(block, area);
            draw_app_body(frame, inner, *app, focused, model);
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
    match app {
        AppKind::Terminal => {
            let text = if model.term_buffer.is_empty() {
                "(empty shell)".into()
            } else {
                let lines: Vec<&str> = model.term_buffer.lines().collect();
                let max = area.height as usize;
                let start = lines.len().saturating_sub(max);
                lines[start..].join("\n")
            };
            frame.render_widget(
                Paragraph::new(text).style(Style::default().fg(Color::Green)),
                area,
            );
        }
        AppKind::Files => draw_files(frame, area, focused, model.files, model.drop_target),
        AppKind::Processes => {
            let lines = vec![
                Line::from("PID   CPU  MEM  COMMAND"),
                Line::from("  1   0.0  0.1  systemd"),
                Line::from("428   1.2  2.4  nginx"),
                Line::from("901   0.3  1.1  sshd"),
                Line::from(""),
                Line::from("(demo · live remote ps in Phase 4)"),
            ];
            frame.render_widget(Paragraph::new(lines), area);
        }
        AppKind::Transfers => draw_transfers(frame, area, focused, model.transfers),
        AppKind::Viewer => draw_viewer(frame, area, model.viewer),
        AppKind::Editor => {
            frame.render_widget(
                Paragraph::new("Text editor · save-back over SFTP in Phase 7."),
                area,
            );
        }
        AppKind::Launcher => {
            frame.render_widget(Clear, area);
        }
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
            Style::default().fg(Color::Yellow),
        )));
    }
    if files.loading {
        lines.push(Line::from("loading…"));
    }

    let rows = files.rows();
    let visible = area.height as usize;
    let start = files.selected.saturating_sub(visible.saturating_sub(1));

    for (idx, row) in rows.iter().enumerate().skip(start).take(visible) {
        let (label, row_path, is_dir) = match row {
            FilesRow::Parent => {
                let p = files
                    .cwd
                    .parent()
                    .map(|p| p.to_path_buf())
                    .unwrap_or_else(|| PathBuf::from("/"));
                ("../".to_string(), Some(p), true)
            }
            FilesRow::Entry(i) => files
                .entries
                .get(*i)
                .map(|e| {
                    let mark = if files.is_marked(*i) { "* " } else { "  " };
                    let mut name = e.display_name();
                    if let Some(sz) = e.size {
                        if !e.is_dir {
                            name = format!("{name}  ({sz})");
                        }
                    }
                    (
                        format!("{mark}{name}"),
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
                .fg(Color::Black)
                .bg(Color::Yellow)
                .add_modifier(Modifier::BOLD)
        } else if selected && focused {
            Style::default()
                .fg(Color::Black)
                .bg(Color::Cyan)
                .add_modifier(Modifier::BOLD)
        } else if selected {
            Style::default().fg(Color::Cyan)
        } else if is_dir {
            Style::default().fg(Color::Yellow)
        } else {
            Style::default()
        };
        let marker = if is_drop {
            "↳ "
        } else if selected {
            "› "
        } else {
            "  "
        };
        lines.push(Line::from(Span::styled(format!("{marker}{label}"), style)));
    }

    if rows.is_empty() && !files.loading {
        lines.push(Line::from("(empty directory)"));
    }

    if focused {
        lines.push(Line::from(Span::styled(
            "drag files · Shift+drop move · Space mark · Ctrl+C/X/V",
            Style::default().fg(Color::DarkGray),
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
                    .fg(Color::Black)
                    .bg(Color::Cyan)
                    .add_modifier(Modifier::BOLD)
            } else if selected {
                Style::default().fg(Color::Cyan)
            } else {
                match job.status {
                    ssh_core::TransferStatus::Failed => Style::default().fg(Color::Red),
                    ssh_core::TransferStatus::Done => Style::default().fg(Color::Green),
                    ssh_core::TransferStatus::Running => Style::default().fg(Color::Yellow),
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
                    Style::default().fg(Color::DarkGray),
                )));
                if let Some(err) = &job.error {
                    lines.push(Line::from(Span::styled(
                        format!("  {err}"),
                        Style::default().fg(Color::Red),
                    )));
                }
            }
        }
    }
    if focused {
        lines.push(Line::from(Span::styled(
            "c cancel · r retry · u/d queue",
            Style::default().fg(Color::DarkGray),
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
        .border_style(Style::default().fg(Color::Cyan));
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
                        .fg(Color::Black)
                        .bg(Color::Cyan)
                } else {
                    Style::default().fg(Color::White)
                },
            )),
        ]),
        chunks[0],
    );

    let mut lines = vec![Line::from(Span::styled(
        format!("local: {}", prompt.browse_cwd.display()),
        Style::default().fg(Color::Yellow),
    ))];
    if let Some(err) = &prompt.error {
        lines.push(Line::from(Span::styled(
            err.clone(),
            Style::default().fg(Color::Red),
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
                .fg(Color::Black)
                .bg(Color::Cyan)
                .add_modifier(Modifier::BOLD)
        } else if entry.is_dir {
            Style::default().fg(Color::Yellow)
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
                .fg(Color::Black)
                .bg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        ),
        rect,
    );
}

fn draw_viewer(frame: &mut Frame<'_>, area: Rect, viewer: &ViewerState) {
    if !viewer.is_open() {
        frame.render_widget(
            Paragraph::new("Open a file from Files (Enter) to view it here.\nEsc closes."),
            area,
        );
        return;
    }

    let mut header = format!("{}", viewer.title);
    if viewer.binary {
        header.push_str("  [hex]");
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
            .fg(Color::Magenta)
            .add_modifier(Modifier::BOLD),
    ))];
    for line in slice {
        lines.push(Line::from(line.to_string()));
    }

    frame.render_widget(Paragraph::new(lines), area);
}

fn draw_dock(frame: &mut Frame<'_>, area: Rect, model: &UiFrame<'_>) {
    let focused = model
        .desktop
        .map(|d| d.focused_app())
        .unwrap_or(AppKind::Launcher);

    let dock_hot = matches!(model.drop_target, Some(DropTarget::TransferDock));
    let mut spans = vec![Span::styled(
        " dock ",
        if dock_hot {
            Style::default()
                .fg(Color::Black)
                .bg(Color::Yellow)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::DarkGray)
        },
    )];
    for app in AppKind::all_dock() {
        let active = model.screen == ScreenKind::Desktop && focused == *app;
        let style = if active {
            Style::default()
                .fg(Color::Black)
                .bg(Color::Cyan)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::Gray)
        };
        spans.push(Span::styled(format!(" {} ", app.label()), style));
        spans.push(Span::raw(" "));
    }
    if model.clipboard_has_files {
        spans.push(Span::styled(
            format!(" [clipboard:{}] ", model.clipboard_label),
            Style::default().fg(Color::Magenta),
        ));
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

fn draw_status(frame: &mut Frame<'_>, area: Rect, model: &UiFrame<'_>) {
    let help = match model.screen {
        ScreenKind::Launcher => "Enter connect · q quit",
        ScreenKind::Desktop => {
            "Space mark · Ctrl+C/X/V · Ctrl+L local · F2 Files · Esc"
        }
    };
    let line = Line::from(vec![
        Span::styled(
            format!(" {} ", model.status),
            Style::default().fg(Color::White),
        ),
        Span::styled("│ ", Style::default().fg(Color::DarkGray)),
        Span::styled(help, Style::default().fg(Color::DarkGray)),
    ]);
    frame.render_widget(
        Paragraph::new(line).style(Style::default().bg(Color::Black)),
        area,
    );
}
