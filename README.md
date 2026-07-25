# ssh-desk

**Remote operating-system shell in the terminal** — not just an SSH host list.

Connect to a host and work in a tiled desktop of panes: interactive shell, SFTP files, viewer, editor, transfer queue, and process list. Multi-host sessions, encrypted vault, TOFU host keys, session restore, clipboard, and drag-and-drop are built in.

| | |
|---|---|
| **Status** | Beta (`v0.1.0`) — daily-driver ready; CI + GitHub Releases |
| **License** | [MIT](LICENSE) |
| **Repo** | https://github.com/Abhishekkkk-15/ssh-desk |

---

## Table of contents

- [Features](#features)
- [Install](#install)
- [Quick start](#quick-start)
- [Paths & configuration](#paths--configuration)
- [Authentication](#authentication)
- [Desktop layout & panes](#desktop-layout--panes)
- [Session restore](#session-restore)
- [File transfers & clipboard](#file-transfers--clipboard)
- [Keybindings](#keybindings)
- [CLI](#cli)
- [Architecture](#architecture)
- [Development](#development)
- [Releases & CI](#releases--ci)
- [Environment variables](#environment-variables)
- [Limitations](#limitations)
- [Troubleshooting](#troubleshooting)

---

## Features

### Connection & security
- Host vault at `~/.config/ssh-desk/hosts.toml`
- Auth: **ssh-agent**, **private key**, or **password** (passwords encrypted with [age](https://github.com/FiloSottile/age) under `secrets/`)
- Optional **jump host** (`jump_via` in vault metadata — connect the jump host first)
- **known_hosts TOFU** (`accept-new`): unknown keys learned; changed keys rejected
- Animated connect spinner (vault unlock + direct connect)
- Background **keepalive** and silent reconnect when SSH/SFTP drops

### Multi-session desktop
- Multiple hosts open as session tabs in one process
- **Ctrl+N** → hosts launcher (sessions stay connected) → **Enter** on another host
- Switch with **Ctrl+Tab** / **F8**; open hosts marked `[open]` in the launcher
- Default 4-pane layout: **Shell | Files** over **Processes | Transfers**
- Open / close / focus panes; restore last layout on quit
- Pane fullscreen (`F11`) and chrome hide (`Ctrl+F`)
- **Light / dark theme** (`Ctrl+T`) and **compact dock** (`Ctrl+Shift+D`) — saved in `config.toml`
- Tokyo Night–inspired UI theme (dark default + light variant)

### Shell
- Live PTY over SSH with **VT100** emulation (colors, cursor, resize)
- **Tab** goes to the remote shell for autocomplete when Shell is focused
- Pane cycling uses `Ctrl+Space` so Tab stays available in the shell

### Files (SFTP)
- Browse, mark multi-select, search (`/`)
- Columns: mode, size, mtime, name (symlinks as `name@`)
- **mkdir** (`a`/`n`), **rename** (`R`), **delete** with confirm (`d`/`Delete`) — recursive delete for directories
- Open files in Viewer; `e` opens Editor; `o` opens images in the OS viewer
- Upload / download prompts; recursive directory transfers supported in the transfer engine

### Viewer & Editor
- Viewer: text, hex preview for binary, **Sixel/Kitty/iTerm2** in known-good terminals (Windows Terminal, Kitty, WezTerm, iTerm, Cursor/VS Code, …); Unicode ▀ half-blocks otherwise. Press **`o`** for the OS image viewer. Force protocol probe with `SSH_DESK_IMAGE_QUERY=1` (can freeze Git Bash → WSL).
- Editor: edit remote text, **Ctrl+S** save-back over SFTP; dirty Esc confirm

### Transfers
- Background upload/download queue with progress
- Cancel (`c`) / retry (`r`)
- Overwrite confirmation when targets already exist

### Processes
- Remote process list (CPU-oriented), refresh with `r`

### Clipboard & drag-and-drop
- Remote **Ctrl+C / Ctrl+X / Ctrl+V** (copy / cut / paste)
- **Cross-host paste**: copy on host A, switch session, paste on host B (relays via a local temp cache)
- Local file onto clipboard: **Ctrl+L**; paste to local: **Ctrl+Shift+V**
- In-TUI drag between folders (hold **Shift** to move)
- OS file drop / `file://` paste → confirm upload (folder drops depend on the terminal)

### Diagnostics & restore
- **F9** structured diagnostics log
- **Ctrl+Q** saves all open host tabs (layouts + cwd + active tab) and quits
- Next launch reconnects those hosts and restores each deck

---

## Install

### One-liner (recommended)

Prebuilt binaries from [GitHub Releases](https://github.com/Abhishekkkk-15/ssh-desk/releases).

**Linux / macOS / WSL**

```bash
curl -fsSL https://raw.githubusercontent.com/Abhishekkkk-15/ssh-desk/main/scripts/install.sh | bash
export PATH="$HOME/.local/bin:$PATH"   # if the script asks you to
ssh-desk
```

Pin version or install dir:

```bash
SSH_DESK_VERSION=v0.1.0 curl -fsSL https://raw.githubusercontent.com/Abhishekkkk-15/ssh-desk/main/scripts/install.sh | bash
SSH_DESK_INSTALL_DIR=/usr/local/bin curl -fsSL https://raw.githubusercontent.com/Abhishekkkk-15/ssh-desk/main/scripts/install.sh | bash
```

**Windows (PowerShell, x64)**

```powershell
irm https://raw.githubusercontent.com/Abhishekkkk-15/ssh-desk/main/scripts/install.ps1 | iex
```

Installs to `%LOCALAPPDATA%\ssh-desk\bin` and adds it to your **User PATH**. Open a **new** terminal, then run `ssh-desk`.

> ARM64 Windows: use WSL or build from source. Prebuilt Windows assets are `x86_64-pc-windows-msvc` only.

### From source

Requires [Rust](https://rustup.rs) (stable).

```bash
git clone https://github.com/Abhishekkkk-15/ssh-desk.git
cd ssh-desk
cargo install --path crates/ssh-desk
# binary → ~/.cargo/bin/ssh-desk
```

Or run without installing:

```bash
cargo run -p ssh-desk --release
```

### Release targets

| Platform | Target triple |
|----------|----------------|
| Linux x86_64 | `x86_64-unknown-linux-gnu` |
| Linux aarch64 | `aarch64-unknown-linux-gnu` |
| macOS Intel | `x86_64-apple-darwin` |
| macOS Apple Silicon | `aarch64-apple-darwin` |
| Windows x64 | `x86_64-pc-windows-msvc` |

---

## Quick start

1. Run `ssh-desk`.
2. On the **launcher**, press **`a`** (or **`n`**) to add a host.
3. Fill **Name / Host / Port / User**. On Auth, press **Space** to cycle: agent → private key → password.
4. **Ctrl+S** (or Enter on the last field) to save.
5. Select the host and press **Enter** to connect.
6. Password hosts: enter the **vault passphrase** used when the secret was saved.
7. Work in the tiled desktop. **Ctrl+Q** quits and remembers open sessions.

---

## Paths & configuration

| Path | Purpose |
|------|---------|
| `~/.config/ssh-desk/hosts.toml` | Host profiles (vault) |
| `~/.config/ssh-desk/secrets/` | age-encrypted password blobs |
| `~/.config/ssh-desk/known_hosts` | TOFU SSH host keys |
| `~/.config/ssh-desk/session.json` | Last open tabs / layouts (written on quit) |
| `~/.config/ssh-desk/config.toml` | UI prefs: `theme` (`dark`/`light`), `compact_dock` |
| `~/.local/state/ssh-desk/ssh-desk.log` | Runtime log (falls back under config dir if no XDG state) |

On Windows, `dirs` resolves the equivalent under `%APPDATA%` / `%LOCALAPPDATA%`.

---

## Authentication

| Method | Notes |
|--------|--------|
| **ssh-agent** | Default in the add-host form. Uses keys loaded in the agent. |
| **Private key** | Path to a key file (form default often `~/.ssh/id_ed25519`). |
| **Password** | Stored encrypted with your vault passphrase — never plaintext in `hosts.toml`. Unlock prompt on connect and on multi-host restore. |

**Jump hosts:** set `jump_via` to another host’s id/name in vault data. Connect the jump host first, then the target. The interactive add form does not yet expose jump fields — edit `hosts.toml` or extend the profile in the vault.

**Host key policy:** first connect to an unknown host learns the key (TOFU). If the key later changes, connection is rejected until you remove the old entry from `known_hosts`.

---

## Desktop layout & panes

Default after connect:

```
┌─────────────┬─────────────┐
│   SHELL     │   FILES     │
├─────────────┼─────────────┤
│ PROCESSES   │ TRANSFERS   │
└─────────────┴─────────────┘
```

| Action | Keys |
|--------|------|
| Next / prev pane | `Ctrl+Space` / `Ctrl+Shift+Space` |
| Next / prev pane (non-Shell) | `Tab` / `Shift+Tab` |
| Open or focus app | `F2` Files · `F3` Shell · `F4` Processes · `F5` Transfers · `F6` Viewer · `F7` Editor |
| Close focused pane | `Ctrl+W` or `F10` (cannot close the last pane) |
| Split Files to the right | `Ctrl+H` |
| Split Shell below | `Ctrl+B` |
| Fullscreen focused pane | `F11` |
| Hide / show title+dock chrome | `Ctrl+F` |

Closed apps appear dimmed as `[FILES]` on the dock; open ones without brackets; focused highlighted.

---

## Session restore

- **Ctrl+Q** with one or more hosts open → write `session.json` (all tabs: layout, focused pane index, files cwd, active host).
- **Ctrl+Q** / **`q`** on the launcher with **no** sessions → delete `session.json`.
- Next start: reconnect every saved host in order, restore each deck, focus the previous active host.
- If any restored host uses password auth, one vault unlock is shared across the queue.
- Cancel unlock with Esc → restore aborted and session file cleared.

---

## File transfers & clipboard

### Upload / download
- **Ctrl+U** / Transfers **`u`** — local path picker (upload into remote cwd). **Enter** opens a folder; **Space** or **Ctrl+Enter** selects a file or folder for upload (recursive).
- **Ctrl+D** / Transfers **`d`** — download selected remote path.
- Queue shows in **Transfers**; **`c`** cancel, **`r`** retry.

### Clipboard
| Chord | Action |
|-------|--------|
| `Ctrl+C` | Copy marked/focused remote entries |
| `Ctrl+X` | Cut marked/focused remote entries |
| `Ctrl+V` | Paste into remote cwd (Files focused) |
| `Ctrl+Shift+V` or `y` | Download clipboard (or selection) to local Downloads |
| `Ctrl+L` | Pick a local path onto the clipboard |

### Drag & drop
- **In-TUI:** drag from Files onto a folder or Transfers dock; **Shift** = move.
- **OS → terminal:** many terminals paste `file://` lists; confirm **Upload**. Folder drops often fail at the terminal layer (files work more reliably).

Overwrite existing targets: confirm dialog (Yes / No).

---

## Keybindings

### Launcher

| Key | Action |
|-----|--------|
| `Enter` | Connect selected host (or switch to it if already open) |
| `Esc` | Back to desktop when sessions are open |
| `a` / `n` | Add host |
| `d` / `Delete` | Delete selected host |
| `j` `k` / `↑` `↓` | Move selection |
| `r` | Reload vault from disk |
| `q` / `Ctrl+Q` | Quit |
| `F9` | Diagnostics |

### Add-host form

| Key | Action |
|-----|--------|
| `Tab` / `Enter` | Next field |
| `Shift+Tab` | Previous field |
| `Space` / `←` `→` / `h` `l` on Auth | Cycle auth method |
| `Ctrl+S` | Save |
| `Esc` | Cancel |

### Vault unlock

| Key | Action |
|-----|--------|
| type + `Enter` | Unlock / start connect or restore |
| `Esc` | Cancel (aborts restore if restoring) |

### Desktop — sessions & chrome

| Key | Action |
|-----|--------|
| `Ctrl+N` | Hosts launcher (keeps open sessions) |
| `Ctrl+T` | Toggle light / dark theme |
| `Ctrl+Shift+D` | Compact / full dock labels |
| `Ctrl+Tab` / `Ctrl+Shift+Tab` | Next / previous host session |
| `F8` | Session picker (`j`/`k`, digits `1`–`9`, `Enter`/`Esc`/`F8` close) |
| `F9` | Diagnostics (`j`/`k`, `PgUp`/`PgDn`, `Home`/`End`, `c` clear, `Esc`/`F9` close) |
| `Ctrl+Q` | Quit + save open sessions |
| `Esc` | Cancel OS drop → cancel drag → clear marks → close editor/viewer content → close session → launcher |

### Files

| Key | Action |
|-----|--------|
| `j` `k` / `↑` `↓` | Move |
| `Home` / `End` | First / last |
| `Enter` `→` `l` | Open dir or viewer |
| `e` | Open in editor |
| `Backspace` `←` `h` | Parent directory |
| `Space` | Toggle mark + move down |
| `/` | Search (type; `Enter` lock; `Esc` cancel) |
| `a` / `n` | New folder |
| `R` | Rename |
| `d` / `Delete` | Delete (confirm; default No) |
| `r` | Refresh listing |
| `Ctrl+U` / `Ctrl+D` | Upload / download prompts |
| `Ctrl+C` `Ctrl+X` `Ctrl+V` `Ctrl+Shift+V` `Ctrl+L` | Clipboard (see above) |
| `y` | Download clipboard/selection → local Downloads (fallback if terminal steals Ctrl+Shift+V) |

### Transfers

| Key | Action |
|-----|--------|
| `j` `k` | Select job |
| `c` | Cancel |
| `r` | Retry |
| `u` / `d` | Upload / download prompts |

### Viewer

| Key | Action |
|-----|--------|
| `j` `k` / `PgUp` `PgDn` / `Home` | Scroll |
| `e` | Open in editor |
| `o` | Open image in OS viewer |
| `q` / `Esc` | Close view |

### Editor

| Key | Action |
|-----|--------|
| arrows / `Home` `End` | Move |
| type / `Enter` / `Backspace` | Edit |
| `Ctrl+S` | Save to remote |
| `Esc` | Close (dirty: confirm once, second Esc discards) |

### Processes

| Key | Action |
|-----|--------|
| `j` `k` | Select |
| `r` | Refresh |

### Path picker / prompts

| Key | Action |
|-----|--------|
| `j` `k` | Browse |
| `Enter` | Open dir or pick file |
| `Backspace` | Parent |
| `Tab` / `e` | Edit path text |
| `Ctrl+E` | Toggle edit mode |
| `s` (download) | Save into browse cwd |
| `Esc` | Cancel |

### Confirm dialogs (OS drop, overwrite, delete)

| Key | Action |
|-----|--------|
| `Enter` / `y` | Confirm |
| `n` / `Esc` | Cancel |
| `Tab` / `h` `l` | Switch Yes/No |
| `u` (OS drop) | Choose upload |

### Mouse

- Click a pane to focus.
- Drag files in Files; drop on a directory or Transfers.
- Shift+drop = move when supported.
- Esc cancels an in-progress drag.

---

## CLI

```bash
ssh-desk              # start TUI
ssh-desk --help       # help
ssh-desk --version    # e.g. ssh-desk 0.1.0
ssh-desk -h | -V      # short forms
```

---

## Architecture

```
crates/
  ssh-desk/   # TUI binary (launcher, desktop, prompts, session restore)
  ssh-core/   # Session hub: SSH, PTY, SFTP, exec, transfers, known_hosts
  ssh-vault/  # Host store + age-encrypted secrets
  ssh-wm/     # Pane tree, focus, split/close, serializable layouts
  ssh-os/     # Clipboard, DnD, OS path paste, MIME, image preview
```

---

## Development

```bash
git clone https://github.com/Abhishekkkk-15/ssh-desk.git
cd ssh-desk
cargo run -p ssh-desk
cargo test --workspace --all-targets
cargo fmt --all
cargo clippy --workspace --all-targets -- -D clippy::correctness -D clippy::suspicious
```

### Local test SSH server

Throwaway Alpine `sshd` (not the app image):

```bash
docker build -f docker/sshd-test.Dockerfile -t ssh-desk-sshd .
docker run --rm -p 2222:22 ssh-desk-sshd
```

Connect in ssh-desk: host `127.0.0.1`, port `2222`, user `root`, password `rootpassword` (password auth + vault passphrase of your choosing when saving).

---

## Releases & CI

### CI ([`.github/workflows/ci.yml`](.github/workflows/ci.yml))

On every push/PR to `main`:

- `cargo fmt --check`
- `cargo clippy` (correctness + suspicious denies)
- `cargo test --workspace --all-targets`

### Release ([`.github/workflows/release.yml`](.github/workflows/release.yml))

Manual only (`workflow_dispatch` in the Actions UI). Builds all platform archives from the selected branch/commit and always publishes/updates the **`v0.1.0`** GitHub Release (replacing prior assets). Install scripts pull that release by default (`latest` → `v0.1.0`).

---

## Environment variables

| Variable | Used by | Purpose |
|----------|---------|---------|
| `RUST_LOG` | app | Tracing filter (default `info`; try `debug`) |
| `SSH_DESK_VERSION` | `install.sh` / `install.ps1` | Pin release tag (e.g. `v0.1.0`) |
| `SSH_DESK_INSTALL_DIR` | install scripts | Override install directory |
| `SSH_DESK_REPO` | install scripts | Override `owner/repo` |

---

## Limitations

- **Beta** — expect sharp edges; report issues on GitHub.
- **OS folder drag-and-drop** often ignored by terminals; use path picker / typed path for directories.
- **Windows ARM64** — no prebuilt asset yet (use WSL or source build).
- **Jump hosts** — supported in core/vault fields; not fully exposed in the add-host form.
- Limited automated coverage beyond unit tests; soak-test on real hosts before critical use.

---

## Troubleshooting

| Symptom | What to try |
|---------|-------------|
| `ssh-desk: command not found` | Ensure `~/.local/bin` or `%LOCALAPPDATA%\ssh-desk\bin` or `~/.cargo/bin` is on `PATH`; open a new terminal after install. |
| Install script 404 | Wait for the GitHub Release assets for your tag; check [Releases](https://github.com/Abhishekkkk-15/ssh-desk/releases). |
| Host key changed / rejected | Inspect and edit `~/.config/ssh-desk/known_hosts` if you trust the new key. |
| Password connect fails | Same vault passphrase used when the host was saved; check `F9` diagnostics. |
| Tab doesn’t complete in shell | Focus the Shell pane; use `Ctrl+Space` to cycle panes instead of Tab. |
| Can’t close pane | `Ctrl+W` / `F10`; last remaining pane cannot be closed. |
| Logs | `~/.local/state/ssh-desk/ssh-desk.log` · `RUST_LOG=debug ssh-desk` |

---

## License

[MIT](LICENSE) © ssh-desk contributors
