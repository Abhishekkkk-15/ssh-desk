# ssh-desk

Remote **operating-system shell** in the terminal — not just an SSH host manager.

Connect to a host, then work in a tiled desktop of panes: shell, files, viewer/editor, transfers, processes. Copy/paste and drag-and-drop are first-class (phased).

## Status

Phase **5** complete:

- In-TUI drag-and-drop: drag files onto folders / transfers dock
- Yellow drop highlights + ghost label (Shift+drop = move)
- File clipboard, transfer queue, SFTP Files + Viewer

## Drag & drop

1. Mouse-down on a file (or marked selection) in Files  
2. Drag onto a directory row, Files pane, or Transfers/dock  
3. Release to copy · hold **Shift** to move · **Esc** cancels  

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
