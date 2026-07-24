//! ratatui views for launcher and desktop OS shell.

use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, List, ListItem, Paragraph, Wrap};
use ratatui::Frame;
use ssh_vault::HostProfile;
use ssh_wm::{AppKind, Desktop, Direction as SplitDir, PaneNode};

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
        Line::from("After connect you get a tiled desktop:"),
        Line::from("  shell · files · processes · transfers"),
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
            let border = if focused {
                Style::default().fg(Color::Cyan)
            } else {
                Style::default().fg(Color::DarkGray)
            };
            let title = format!(
                " {}{} ",
                app.label(),
                if focused { " ●" } else { "" }
            );
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
                // Show tail that fits.
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
        AppKind::Files => {
            let lines = vec![
                Line::from(Span::styled(
                    " /home  (remote · Phase 2 SFTP)",
                    Style::default().fg(Color::Yellow),
                )),
                Line::from("  ../"),
                Line::from("  bin/"),
                Line::from("  etc/"),
                Line::from("  var/www/"),
                Line::from("  index.html"),
                Line::from(""),
                Line::from(if focused {
                    "Enter open · Ctrl+C/V file clipboard · drag to transfer"
                } else {
                    "focus this pane to browse"
                }),
            ];
            frame.render_widget(Paragraph::new(lines), area);
        }
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
        AppKind::Transfers => {
            let lines = vec![
                Line::from(Span::styled(
                    " queue ",
                    Style::default().add_modifier(Modifier::BOLD),
                )),
                Line::from("  (empty)"),
                Line::from(""),
                Line::from("Uploads from picker, paste, and DnD"),
                Line::from("appear here with progress."),
            ];
            frame.render_widget(Paragraph::new(lines), area);
        }
        AppKind::Viewer => {
            frame.render_widget(
                Paragraph::new("Open a file from Files to view it here."),
                area,
            );
        }
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

fn draw_dock(frame: &mut Frame<'_>, area: Rect, model: &UiFrame<'_>) {
    let focused = model
        .desktop
        .map(|d| d.focused_app())
        .unwrap_or(AppKind::Launcher);

    let mut spans = vec![Span::styled(" dock ", Style::default().fg(Color::DarkGray))];
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
            " [files on clipboard] ",
            Style::default().fg(Color::Magenta),
        ));
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

fn draw_status(frame: &mut Frame<'_>, area: Rect, model: &UiFrame<'_>) {
    let help = match model.screen {
        ScreenKind::Launcher => "Enter connect · q quit",
        ScreenKind::Desktop => {
            "Tab focus · F2-F5 apps · Ctrl+H/V split · Esc launcher · Ctrl+Q quit"
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
