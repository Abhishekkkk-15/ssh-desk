# ssh-desk

Remote **operating-system shell** in the terminal — not just an SSH host manager.

Connect to a host, then work in a tiled desktop of panes: shell, files, viewer/editor, transfers, processes. Copy/paste and drag-and-drop are first-class (phased).

## Status

Phase **7** complete, plus interactive host management and diagnostics:

- Launcher: **`a` / `n`** add host (agent · key · password), **`d`** delete
- Password hosts prompt for the vault passphrase on connect
- Editor save-back, image preview, process monitor
- **`F9`** diagnostics log (status bar stays one-line; full history in the overlay)
- File log: `ssh-desk.log` (developer tracing)

## Add a host

1. Run `cargo run -p ssh-desk`
2. On the launcher press **`a`**
3. Fill Name / Host / Port / User; Space cycles Auth
4. **Ctrl+S** (or Enter on the last field) to save

Vault file: `~/.config/ssh-desk/hosts.toml`

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
