# ssh-desk

Remote **operating-system shell** in the terminal — not just an SSH host manager.

Connect to a host, then work in a tiled desktop of panes: shell, files, viewer/editor, transfers, processes. Copy/paste and drag-and-drop are first-class (phased).

## Status

Phase **2** in progress:

- Desktop window manager (split panes, focus, dock)
- Host vault (`~/.config/ssh-desk`)
- SSH connect + PTY via `russh` (agent / key / encrypted password)
- **SFTP Files app** — browse remote dirs, open text/hex in Viewer
- Transfers / processes panes still placeholders

## Build

```bash
cargo build -p ssh-desk
cargo run -p ssh-desk
```

## Keys

| Context | Binding |
|---------|---------|
| Launcher | `Enter` connect, `j/k` select, `r` reload vault, `q` quit |
| Desktop | `Tab` focus, `F2` Files, `F3` Term, `F6` Viewer, `Ctrl+H`/`Ctrl+V` split |
| Files | `j/k` move, `Enter` open/cd, `Backspace` parent, `r` refresh |
| Viewer | `j/k` scroll, `Esc`/`q` close |
| Global | `Esc` launcher (closes viewer first), `Ctrl+Q` quit |

## Layout

```
crates/
  ssh-desk/   # TUI binary
  ssh-core/   # SSH session hub (PTY / SFTP later)
  ssh-vault/  # encrypted host store
  ssh-wm/     # pane tree + layouts
  ssh-os/     # clipboard, DnD, open-with
```
