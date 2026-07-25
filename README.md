# ssh-desk

Remote **operating-system shell** in the terminal — not just an SSH host manager.

Connect to a host, then work in a tiled desktop of panes: shell, files, viewer/editor, transfers, processes. Copy/paste and drag-and-drop are first-class (phased).

## Status

Hardening in progress toward production:

- **VT100 terminal emulator** (colors, cursor, resize) via `vt100`
- **known_hosts** TOFU at `~/.config/ssh-desk/known_hosts` (reject on key change)
- **Recursive** upload / download / remote copy for directories
- Editor save-back, image preview, processes, multi-session, diagnostics (`F9`)
- Interactive host form (`a` / `n`)

Still not full production: limited tests, no `ssh_config` import polish, etc.

## Add a host

1. Run `cargo run -p ssh-desk`
2. On the launcher press **`a`**
3. Fill Name / Host / Port / User; Space cycles Auth
4. **Ctrl+S** (or Enter on the last field) to save

Vault file: `~/.config/ssh-desk/hosts.toml`  
Host keys: `~/.config/ssh-desk/known_hosts`

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
