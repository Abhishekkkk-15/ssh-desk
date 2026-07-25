# Install ssh-desk from the latest GitHub Release (Windows x64).
#
# Usage (PowerShell):
#   irm https://raw.githubusercontent.com/Abhishekkkk-15/ssh-desk/main/scripts/install.ps1 | iex
#
# Or:
#   powershell -ExecutionPolicy Bypass -File .\scripts\install.ps1
#
# Env / params:
#   -Version v0.1.0
#   -InstallDir "$env:LOCALAPPDATA\ssh-desk\bin"
#   -Repo owner/repo

[CmdletBinding()]
param(
    [string]$Version = $env:SSH_DESK_VERSION,
    [string]$InstallDir = $(if ($env:SSH_DESK_INSTALL_DIR) { $env:SSH_DESK_INSTALL_DIR } else { Join-Path $env:LOCALAPPDATA "ssh-desk\bin" }),
    [string]$Repo = $(if ($env:SSH_DESK_REPO) { $env:SSH_DESK_REPO } else { "Abhishekkkk-15/ssh-desk" })
)

$ErrorActionPreference = "Stop"

function Write-Info([string]$Message) {
    Write-Host "==> $Message"
}

function Get-LatestTag([string]$Repository) {
    $release = Invoke-RestMethod -Uri "https://api.github.com/repos/$Repository/releases/latest" -Headers @{
        "User-Agent" = "ssh-desk-installer"
        "Accept"     = "application/vnd.github+json"
    }
    if (-not $release.tag_name) {
        throw "Could not resolve latest release for $Repository"
    }
    return $release.tag_name
}

function Ensure-OnPath([string]$Dir) {
    $userPath = [Environment]::GetEnvironmentVariable("Path", "User")
    if (-not $userPath) { $userPath = "" }
    $parts = $userPath -split ";" | Where-Object { $_ -and $_.Trim() -ne "" }
    if ($parts -contains $Dir) {
        return $false
    }
    $newPath = if ($userPath.Trim().Length -eq 0) { $Dir } else { "$userPath;$Dir" }
    [Environment]::SetEnvironmentVariable("Path", $newPath, "User")
    # Also update current session
    if (-not (($env:Path -split ";") -contains $Dir)) {
        $env:Path = "$Dir;$env:Path"
    }
    return $true
}

Write-Host "ssh-desk installer (Windows)" -ForegroundColor Cyan

if (-not $Version -or $Version.Trim().Length -eq 0) {
    Write-Info "resolving latest release..."
    $Version = Get-LatestTag -Repository $Repo
}

$arch = $env:PROCESSOR_ARCHITECTURE
if ($arch -ne "AMD64") {
    throw "Unsupported architecture '$arch'. Prebuilt Windows releases are x86_64 only for now."
}

$target = "x86_64-pc-windows-msvc"
$stage = "ssh-desk-$Version-$target"
$asset = "$stage.zip"
$url = "https://github.com/$Repo/releases/download/$Version/$asset"

Write-Info "repo:    $Repo"
Write-Info "version: $Version"
Write-Info "target:  $target"
Write-Info "install: $InstallDir"

$tmp = Join-Path ([System.IO.Path]::GetTempPath()) ("ssh-desk-install-" + [guid]::NewGuid().ToString("N"))
New-Item -ItemType Directory -Force -Path $tmp | Out-Null

try {
    $zipPath = Join-Path $tmp $asset
    Write-Info "downloading $asset..."
    try {
        Invoke-WebRequest -Uri $url -OutFile $zipPath -UseBasicParsing
    } catch {
        throw "Download failed ($url). Create a GitHub Release first (git tag vX.Y.Z && git push --tags)."
    }

    $extract = Join-Path $tmp "extract"
    New-Item -ItemType Directory -Force -Path $extract | Out-Null
    Expand-Archive -Path $zipPath -DestinationPath $extract -Force

    $bin = Get-ChildItem -Path $extract -Recurse -Filter "ssh-desk.exe" | Select-Object -First 1
    if (-not $bin) {
        throw "ssh-desk.exe not found in archive"
    }

    New-Item -ItemType Directory -Force -Path $InstallDir | Out-Null
    $dest = Join-Path $InstallDir "ssh-desk.exe"
    Copy-Item -Force $bin.FullName $dest

    $added = Ensure-OnPath -Dir $InstallDir
    Write-Host "installed $dest" -ForegroundColor Green
    if ($added) {
        Write-Info "added $InstallDir to your User PATH (open a new terminal)"
    }

    Write-Info "run: ssh-desk --version"
} finally {
    Remove-Item -Recurse -Force $tmp -ErrorAction SilentlyContinue
}
