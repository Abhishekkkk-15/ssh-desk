# ssh-desk

Remote **operating-system shell** in the terminal — not just an SSH host manager.

Connect to a host, then work in a tiled desktop of panes: shell, files, viewer/editor, transfers, processes. Copy/paste and drag-and-drop are first-class (phased).

## Status

Phase **0–1** scaffold:

- Desktop window manager (split panes, focus, dock)
- Host vault (`~/.config/ssh-desk`)
- SSH connect + PTY via `russh` (agent / key / encrypted password)
- Files / transfers / processes panes present as OS shell placeholders

## Build

```bash
cargo build -p ssh-desk
cargo run -p ssh-desk
```

## Keys

| Context | Binding |
|---------|---------|
| Launcher | `Enter` connect, `j/k` select, `r` reload vault, `q` quit |
| Desktop | `Tab` focus, `F2`–`F5` apps, `Ctrl+H`/`Ctrl+V` split, `Esc` launcher, `Ctrl+Q` quit |

## Layout

```
crates/
  ssh-desk/   # TUI binary
  ssh-core/   # SSH session hub (PTY / SFTP later)
  ssh-vault/  # encrypted host store
  ssh-wm/     # pane tree + layouts
  ssh-os/     # clipboard, DnD, open-with
```
