# Dociler Installer for Windows
# https://github.com/deanyouknow/Dociler
#
# Usage:
#   irm https://raw.githubusercontent.com/deanyouknow/Dociler/main/install.ps1 | iex
#
# Parameters:
#   -Version <string>     Specify version to install (e.g., v0.1.0)
#   -InstallDir <string>  Destination directory for dociler.exe
#   -Target <string>      Target platform override (default: windows-x86_64)
#   -File <string>        Install from local archive (.zip) or executable (.exe)
#   -Checksum <string>    Expected SHA-256 hash of the archive
#   -DryRun               Display installation plan without modifying files
#   -NoModifyPath         Do not offer or attempt to modify user PATH
#   -Help                 Show usage help

[CmdletBinding()]
param (
    [string]$Version = "",
    [string]$InstallDir = "",
    [string]$Repo = "deanyouknow/Dociler",
    [string]$Target = "windows-x86_64",
    [string]$File = "",
    [string]$Checksum = "",
    [switch]$DryRun,
    [switch]$NoModifyPath,
    [switch]$Help
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

if ($Help) {
    Write-Host @"
Dociler Installer for Windows

Usage:
  install.ps1 [-Version <ver>] [-InstallDir <dir>] [-Target <target>] [-File <path>] [-Checksum <hash>] [-DryRun] [-NoModifyPath]

Parameters:
  -Version <ver>     Version tag to install (e.g., v0.1.0, default: v0.1.0-dev)
  -InstallDir <dir>  Installation folder (default: %LOCALAPPDATA%\Programs\Dociler\bin)
  -Target <target>   Platform target (default: windows-x86_64)
  -File <path>       Install directly from local zip archive or dociler.exe
  -Checksum <hash>   Expected SHA-256 checksum of archive
  -DryRun            Preview installation plan without downloading or modifying files
  -NoModifyPath      Do not update user PATH environment variable
  -Help              Show this help message

Environment variables:
  DOCILER_VERSION      Version to install
  DOCILER_INSTALL_DIR  Installation folder
  DOCILER_REPO         GitHub repository (default: deanyouknow/Dociler)
  DOCILER_TARGET       Platform target override

Safety note:
  Dociler is a standalone, local-first application. This installer copies a single
  binary to your user directory. It does NOT install Docker, Python, Ollama, model
  weights, background services, or administrative daemons.
"@
    exit 0
}

# Check architecture
$arch = $env:PROCESSOR_ARCHITECTURE
if ($arch -notin @('AMD64', 'x86_64')) {
    Write-Error "Unsupported processor architecture '$arch'. Dociler v1 for Windows requires x86-64 (64-bit AMD/Intel)."
    exit 1
}

# Resolve defaults from environment if parameters are omitted
if (-not $Version) {
    if ($env:DOCILER_VERSION) {
        $Version = $env:DOCILER_VERSION
    } else {
        $Version = "v0.1.0-dev"
    }
}

if (-not ($Version.StartsWith("v"))) {
    $Version = "v$Version"
}

if (-not $InstallDir) {
    if ($env:DOCILER_INSTALL_DIR) {
        $InstallDir = $env:DOCILER_INSTALL_DIR
    } elseif ($env:LOCALAPPDATA) {
        $InstallDir = Join-Path $env:LOCALAPPDATA "Programs\Dociler\bin"
    } else {
        $InstallDir = Join-Path $env:USERPROFILE ".local\bin"
    }
}

if (-not $File -and $env:DOCILER_FILE) {
    $File = $env:DOCILER_FILE
}

if (-not $Checksum -and $env:DOCILER_CHECKSUM) {
    $Checksum = $env:DOCILER_CHECKSUM
}

Write-Host "Dociler Installer for Windows" -ForegroundColor Cyan
Write-Host "=============================" -ForegroundColor Cyan
Write-Host "Target Platform : $Target"
Write-Host "Version         : $Version"
Write-Host "Install Folder  : $InstallDir"

if ($DryRun) {
    Write-Host ""
    Write-Host "Dry run enabled. No files will be downloaded or modified." -ForegroundColor Yellow
    exit 0
}

# Create temporary staging directory
$tempDir = Join-Path [System.IO.Path]::GetTempPath() ([System.Guid]::NewGuid().ToString("N"))
New-Item -ItemType Directory -Path $tempDir -Force | Out-Null

try {
    # Ensure destination directory exists
    if (-not (Test-Path $InstallDir)) {
        New-Item -ItemType Directory -Path $InstallDir -Force | Out-Null
    }

    $archiveName = "dociler-$Version-$Target.zip"
    $stagingArchive = Join-Path $tempDir $archiveName

    if ($File) {
        Write-Host "Using local file: $File"
        if ($File.EndsWith(".zip", [System.StringComparison]::OrdinalIgnoreCase)) {
            Copy-Item -Path $File -Destination $stagingArchive -Force
        } else {
            # Direct executable
            Copy-Item -Path $File -Destination (Join-Path $tempDir "dociler.exe") -Force
        }
    } else {
        $releaseBaseUrl = "https://github.com/$Repo/releases/download/$Version"
        $archiveUrl = "$releaseBaseUrl/$archiveName"
        $checksumUrl = "$releaseBaseUrl/SHA256SUMS"

        Write-Host "Downloading $archiveName..."
        [System.Net.ServicePointManager]::SecurityProtocol = [System.Net.SecurityProtocolType]::Tls12 -bor [System.Net.SecurityProtocolType]::Tls13
        Invoke-WebRequest -Uri $archiveUrl -OutFile $stagingArchive -UseBasicParsing

        if (-not $Checksum) {
            $checksumFile = Join-Path $tempDir "SHA256SUMS"
            try {
                Invoke-WebRequest -Uri $checksumUrl -OutFile $checksumFile -UseBasicParsing
                if (Test-Path $checksumFile) {
                    $lines = Get-Content $checksumFile
                    foreach ($line in $lines) {
                        if ($line -match "^([a-fA-F0-9]{64})\s+.*$([regex]::Escape($archiveName))") {
                            $Checksum = $matches[1]
                            break
                        }
                    }
                }
            } catch {
                # Optional checksum manifest
            }
        }
    }

    if (Test-Path $stagingArchive) {
        $actualHash = (Get-FileHash -Path $stagingArchive -Algorithm SHA256).Hash.ToLowerInvariant()
        if ($Checksum) {
            Write-Host "Verifying SHA-256 checksum..."
            $expectedHash = $Checksum.Trim().ToLowerInvariant()
            if ($actualHash -ne $expectedHash) {
                Write-Error "Checksum verification failed! Expected: $expectedHash, Actual: $actualHash"
                exit 1
            }
            Write-Host "Checksum verified: $actualHash" -ForegroundColor Green
        }

        # Expand archive
        Expand-Archive -Path $stagingArchive -DestinationPath $tempDir -Force
    }

    $extractedExe = Join-Path $tempDir "dociler.exe"
    if (-not (Test-Path $extractedExe)) {
        Write-Error "Failed to find 'dociler.exe' in extracted package."
        exit 1
    }

    $destExe = Join-Path $InstallDir "dociler.exe"
    Move-Item -Path $extractedExe -Destination $destExe -Force

    Write-Host ""
    Write-Host "Installation complete!" -ForegroundColor Green
    Write-Host "Installed binary: $destExe"
    Write-Host ""
    Write-Host "Note: Dociler is a standalone, local-first tool."
    Write-Host "No Docker, Python, Ollama, model weights, or background services were installed."
    Write-Host ""

    if (-not $NoModifyPath) {
        $userPath = [System.Environment]::GetEnvironmentVariable("Path", "User")
        $pathParts = $userPath -split ';'
        $inPath = $false
        foreach ($p in $pathParts) {
            if ($p.TrimEnd('\') -ieq $InstallDir.TrimEnd('\')) {
                $inPath = $true
                break
            }
        }

        if (-not $inPath) {
            try {
                $newPath = "$userPath;$InstallDir"
                [System.Environment]::SetEnvironmentVariable("Path", $newPath, "User")
                $env:Path = "$env:Path;$InstallDir"
                Write-Host "Added '$InstallDir' to your user PATH environment variable." -ForegroundColor Cyan
                Write-Host "Restart open terminal windows to apply the updated PATH."
            } catch {
                Write-Host "Notice: '$InstallDir' is not in your PATH." -ForegroundColor Yellow
                Write-Host "To run 'dociler' from any command prompt, add '$InstallDir' to your User PATH."
            }
        } else {
            Write-Host "'$InstallDir' is already in your PATH." -ForegroundColor Cyan
            Write-Host "Run 'dociler --version' or 'dociler doctor' to get started."
        }
    }
} finally {
    if (Test-Path $tempDir) {
        Remove-Item -Path $tempDir -Recurse -Force -ErrorAction SilentlyContinue
    }
}
