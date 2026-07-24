# ssh-desk

Remote **operating-system shell** in the terminal — not just an SSH host manager.

Connect to a host, then work in a tiled desktop of panes: shell, files, viewer/editor, transfers, processes. Copy/paste and drag-and-drop are first-class (phased).

## Status

Phase **7** complete:

- Editor with SFTP save-back (`Ctrl+S`; Esc warns if dirty)
- Image preview in Viewer (half-block `▀` cells)
- Processes app via remote `ps` (`F4` / `r` refresh)
- Files: `e` force-open in editor; Viewer: `e` edit text

Phases 0–6: vault, PTY, Files/Viewer, transfers, clipboard, in-TUI DnD, OS drops.

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
  ssh-core/   # SSH session hub (PTY / SFTP / exec)
  ssh-vault/  # encrypted host store
  ssh-wm/     # pane tree + layouts
  ssh-os/     # clipboard, DnD, open-with, image preview
```
