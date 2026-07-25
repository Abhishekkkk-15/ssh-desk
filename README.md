# ssh-desk

Remote **operating-system shell** in the terminal — not just an SSH host manager.

Connect to a host, then work in a tiled desktop of panes: shell, files, viewer/editor, transfers, processes. Copy/paste and drag-and-drop are first-class.

**Status:** beta — suitable for daily personal use; CI builds on every push.

## Install

### One-liner (recommended)

**Linux / macOS / WSL**

Downloads the correct binary for your OS/CPU from [GitHub Releases](https://github.com/Abhishekkkk-15/ssh-desk/releases) into `~/.local/bin`:

```bash
curl -fsSL https://raw.githubusercontent.com/Abhishekkkk-15/ssh-desk/main/scripts/install.sh | bash
```

Then ensure `~/.local/bin` is on your `PATH` (the script prints the line if needed):

```bash
export PATH="$HOME/.local/bin:$PATH"
ssh-desk
```

Pin a version or install location:

```bash
SSH_DESK_VERSION=v0.1.0 curl -fsSL https://raw.githubusercontent.com/Abhishekkkk-15/ssh-desk/main/scripts/install.sh | bash
SSH_DESK_INSTALL_DIR=/usr/local/bin curl -fsSL … | bash   # may need sudo
```

**Windows (PowerShell)**

```powershell
irm https://raw.githubusercontent.com/Abhishekkkk-15/ssh-desk/main/scripts/install.ps1 | iex
```

Installs `ssh-desk.exe` to `%LOCALAPPDATA%\ssh-desk\bin` and adds that folder to your **User PATH**. Open a **new** terminal, then:

```powershell
ssh-desk
ssh-desk --version
```

Prebuilt Windows assets are **x64** (`x86_64-pc-windows-msvc`). ARM64 Windows: use WSL or build from source for now.

### From source (Rust / rustup)

```bash
git clone https://github.com/Abhishekkkk-15/ssh-desk.git
cd ssh-desk
cargo install --path crates/ssh-desk
```

### Publish a release (maintainers)

```bash
git tag v0.1.0
git push origin v0.1.0
```

That triggers [`.github/workflows/release.yml`](.github/workflows/release.yml), which builds Linux / macOS / Windows archives and attaches them to the GitHub Release.

```bash
ssh-desk --version
ssh-desk --help
```

## Paths

| Path | Purpose |
|------|---------|
| `~/.config/ssh-desk/hosts.toml` | Host vault |
| `~/.config/ssh-desk/secrets/` | Encrypted password blobs |
| `~/.config/ssh-desk/known_hosts` | TOFU host keys |
| `~/.config/ssh-desk/session.json` | Last open tabs (layouts) |
| `~/.local/state/ssh-desk/ssh-desk.log` | Runtime log (`RUST_LOG=debug` supported) |

## Session restore

- **Ctrl+Q** quits the app. If one or more hosts are open, all tabs (pane layout + files cwd + active tab) are saved to `session.json`.
- On next start, ssh-desk reconnects those hosts and restores each deck. Password-auth hosts prompt once for the vault passphrase (shared across the restore queue).
- Quit from the launcher with **no** open sessions clears `session.json` (nothing remembered).

## Add a host

1. Start `ssh-desk`
2. On the launcher press **`a`** / **`n`**
3. Fill Name / Host / Port / User; Space cycles Auth
4. **Ctrl+S** (or Enter on the last field) to save

## Keybindings (desktop)

| Key | Action |
|-----|--------|
| `Ctrl+Space` / `Ctrl+Shift+Space` | Next / previous pane |
| `Tab` | Pane cycle (outside Shell); autocomplete inside Shell |
| `F2`–`F7` | Open/focus Files · Shell · Processes · Transfers · Viewer · Editor |
| `Ctrl+W` / `F10` | Close focused pane |
| `Ctrl+Tab` | Next host session |
| `F8` | Session picker |
| `F9` | Diagnostics log |
| `F11` | Fullscreen focused pane |
| `Ctrl+Q` | Quit (saves open sessions) |
| `Esc` | Close current host session / back to launcher |

## OS file drop

Most terminals deliver Explorer/Finder drops as bracketed paste.

1. Connect and open the desktop  
2. Drop files onto the terminal (or paste a `file://` URI list)  
3. Confirm **Upload** → queue into the current remote directory  

Fallbacks: `Ctrl+U` picker · `Ctrl+L` copy-local · in-TUI drag.

## Dev: test SSH server

A throwaway Alpine sshd for local testing lives at [`docker/sshd-test.Dockerfile`](docker/sshd-test.Dockerfile):

```bash
docker build -f docker/sshd-test.Dockerfile -t ssh-desk-sshd .
docker run --rm -p 2222:22 ssh-desk-sshd
# user root / password rootpassword · port 2222
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

## CI

GitHub Actions on every push/PR to `main` runs:

- `cargo fmt --check`
- `cargo clippy` (correctness + suspicious denies)
- `cargo test --workspace --all-targets`
