# ssh-desk

Remote **operating-system shell** in the terminal — not just an SSH host manager.

Connect to a host, then work in a tiled desktop of panes: shell, files, viewer/editor, transfers, processes. Copy/paste and drag-and-drop are first-class (phased).

## Status

Phase **3** complete:

- Desktop WM, vault, SSH/PTY connect
- SFTP Files + Viewer
- **Transfer queue** with upload/download, progress, cancel/retry, local path picker

## Build

```bash
cargo build -p ssh-desk
cargo run -p ssh-desk
```

## Keys

| Context | Binding |
|---------|---------|
| Launcher | `Enter` connect, `j/k` select, `r` reload vault, `q` quit |
| Desktop | `Tab` focus, `F2` Files, `F5` Transfers, `F6` Viewer, splits |
| Files | `Enter` open, `Ctrl+U` upload, `Ctrl+D` download, `r` refresh |
| Transfers | `j/k` select, `c` cancel, `r` retry, `u`/`d` queue |
| Path picker | browse or `Tab` edit path, `Enter` confirm, `Esc` cancel |
| Viewer | `j/k` scroll, `Esc`/`q` close |

## Layout

```
crates/
  ssh-desk/   # TUI binary
  ssh-core/   # SSH session hub (PTY / SFTP later)
  ssh-vault/  # encrypted host store
  ssh-wm/     # pane tree + layouts
  ssh-os/     # clipboard, DnD, open-with
```
