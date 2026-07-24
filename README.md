# ssh-desk

Remote **operating-system shell** in the terminal — not just an SSH host manager.

Connect to a host, then work in a tiled desktop of panes: shell, files, viewer/editor, transfers, processes. Copy/paste and drag-and-drop are first-class (phased).

## Status

Phase **4** complete:

- Desktop WM, vault, SSH/PTY, SFTP Files + Viewer
- Transfer queue with upload/download
- **File clipboard** — multi-select, copy/cut/paste local↔remote

## Build

```bash
cargo build -p ssh-desk
cargo run -p ssh-desk
```

## Keys

| Context | Binding |
|---------|---------|
| Files | `Space` mark, `Ctrl+C` copy, `Ctrl+X` cut, `Ctrl+V` paste into cwd |
| Files | `Ctrl+L` copy local file onto clipboard, `Ctrl+Shift+V` paste remote→~/Downloads |
| Files | `Ctrl+U`/`Ctrl+D` upload/download picker |
| Transfers | `j/k` select, `c` cancel, `r` retry |

## Layout

```
crates/
  ssh-desk/   # TUI binary
  ssh-core/   # SSH session hub (PTY / SFTP later)
  ssh-vault/  # encrypted host store
  ssh-wm/     # pane tree + layouts
  ssh-os/     # clipboard, DnD, open-with
```
