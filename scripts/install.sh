#!/usr/bin/env bash
# Install ssh-desk from the latest GitHub Release (OS/arch auto-detected).
#
# Usage:
#   curl -fsSL https://raw.githubusercontent.com/Abhishekkkk-15/ssh-desk/main/scripts/install.sh | bash
#
# Options (env):
#   SSH_DESK_VERSION=v0.1.0     pin a release tag (default: latest)
#   SSH_DESK_INSTALL_DIR=...    install directory (default: ~/.local/bin)
#   SSH_DESK_REPO=owner/repo    override repo (default: Abhishekkkk-15/ssh-desk)

set -euo pipefail

REPO="${SSH_DESK_REPO:-Abhishekkkk-15/ssh-desk}"
INSTALL_DIR="${SSH_DESK_INSTALL_DIR:-${HOME}/.local/bin}"
VERSION="${SSH_DESK_VERSION:-}"

bold() { printf '\033[1m%s\033[0m\n' "$*"; }
info() { printf '==> %s\n' "$*"; }
die()  { printf 'error: %s\n' "$*" >&2; exit 1; }

need_cmd() {
  command -v "$1" >/dev/null 2>&1 || die "missing required command: $1"
}

detect_target() {
  local os arch
  os="$(uname -s)"
  arch="$(uname -m)"

  case "$arch" in
    x86_64|amd64) arch="x86_64" ;;
    aarch64|arm64) arch="aarch64" ;;
    *) die "unsupported CPU architecture: $arch" ;;
  esac

  case "$os" in
    Linux)
      echo "${arch}-unknown-linux-gnu"
      ;;
    Darwin)
      echo "${arch}-apple-darwin"
      ;;
    MINGW*|MSYS*|CYGWIN*)
      echo "${arch}-pc-windows-msvc"
      ;;
    *)
      die "unsupported OS: $os (use WSL/Linux/macOS, or download a release asset manually)"
      ;;
  esac
}

latest_tag() {
  # Prefer the GitHub API; fall back to redirect from /releases/latest.
  if command -v curl >/dev/null 2>&1; then
    local tag
    tag="$(curl -fsSL "https://api.github.com/repos/${REPO}/releases/latest" \
      | sed -n 's/.*"tag_name"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' \
      | head -n1)"
    if [[ -n "$tag" ]]; then
      echo "$tag"
      return
    fi
    tag="$(curl -fsSLI -o /dev/null -w '%{url_effective}' \
      "https://github.com/${REPO}/releases/latest" | sed 's#.*/##')"
    [[ -n "$tag" && "$tag" != "latest" ]] || die "could not resolve latest release for ${REPO}"
    echo "$tag"
  else
    die "curl is required"
  fi
}

download() {
  local url="$1" out="$2"
  curl -fsSL --retry 3 --retry-delay 1 -o "$out" "$url"
}

main() {
  need_cmd curl
  need_cmd tar
  need_cmd uname
  need_cmd mktemp

  local target archive_ext asset url tmpdir stage bin_name
  target="$(detect_target)"

  if [[ -z "$VERSION" ]]; then
    info "resolving latest release…"
    VERSION="$(latest_tag)"
  fi

  case "$target" in
    *-pc-windows-msvc)
      archive_ext="zip"
      bin_name="ssh-desk.exe"
      need_cmd unzip
      ;;
    *)
      archive_ext="tar.gz"
      bin_name="ssh-desk"
      ;;
  esac

  stage="ssh-desk-${VERSION}-${target}"
  asset="${stage}.${archive_ext}"
  url="https://github.com/${REPO}/releases/download/${VERSION}/${asset}"

  bold "ssh-desk installer"
  info "repo:    ${REPO}"
  info "version: ${VERSION}"
  info "target:  ${target}"
  info "install: ${INSTALL_DIR}"

  tmpdir="$(mktemp -d)"
  trap 'rm -rf "$tmpdir"' EXIT

  info "downloading ${asset}…"
  if ! download "$url" "${tmpdir}/${asset}"; then
    die "download failed (${url}). Create a GitHub Release first (git tag vX.Y.Z && git push --tags)."
  fi

  info "extracting…"
  mkdir -p "${tmpdir}/extract"
  case "$archive_ext" in
    tar.gz) tar -xzf "${tmpdir}/${asset}" -C "${tmpdir}/extract" ;;
    zip)    unzip -q "${tmpdir}/${asset}" -d "${tmpdir}/extract" ;;
  esac

  local bin
  bin="$(find "${tmpdir}/extract" -type f -name "$bin_name" | head -n1)"
  [[ -n "$bin" ]] || die "binary '$bin_name' not found in archive"

  mkdir -p "$INSTALL_DIR"
  install -m 0755 "$bin" "${INSTALL_DIR}/${bin_name}"

  bold "installed ${INSTALL_DIR}/${bin_name}"

  case ":$PATH:" in
    *":${INSTALL_DIR}:"*) ;;
    *)
      printf '\n'
      info "${INSTALL_DIR} is not on your PATH. Add this to your shell rc:"
      printf '\n  export PATH="%s:\$PATH"\n\n' "$INSTALL_DIR"
      info "then restart the shell (or: source ~/.bashrc / ~/.zshrc)"
      ;;
  esac

  if command -v ssh-desk >/dev/null 2>&1; then
    info "ok · $(command -v ssh-desk) · $(ssh-desk --version 2>/dev/null || true)"
  else
    info "run: ${INSTALL_DIR}/${bin_name} --version"
  fi
}

main "$@"
