# ssh-desk

Remote **operating-system shell** in the terminal — not just an SSH host manager.

Connect to a host, then work in a tiled desktop of panes: shell, files, viewer/editor, transfers, processes. Copy/paste and drag-and-drop are first-class (phased).

## Status

Phase **6** complete:

- OS → TUI file drops via bracketed paste (`file://` / path lists)
- Confirm dialog before upload into the remote Files cwd
- In-TUI DnD, clipboard, transfer queue, SFTP Files + Viewer

## OS file drop

Most terminals deliver Explorer/Finder drops as bracketed paste.

1. Connect to a host and open the desktop  
2. Drop files onto the terminal window (or paste a `file://` URI list)  
3. Confirm **Upload** → files queue into the current remote directory  

Offline: paths land on the file clipboard for later `Ctrl+V`.

Fallbacks: `Ctrl+U` picker · `Ctrl+L` copy-local · in-TUI drag.

## Build

```bash
cargo run -p ssh-desk
```

## Layout

```
crates/
  ssh-desk/   # TUI binary
  ssh-core/   # SSH session hub (PTY / SFTP later)
  ssh-vault/  # encrypted host store
  ssh-wm/     # pane tree + layouts
  ssh-os/     # clipboard, DnD, open-with
```
