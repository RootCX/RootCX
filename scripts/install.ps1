#!/usr/bin/env pwsh
# RootCX CLI installer — https://rootcx.com
# Usage: powershell -c "irm https://rootcx.com/install.ps1 | iex"

param(
  [String]$Version = "latest",
  [Switch]$NoPathUpdate = $false
)

$ErrorActionPreference = "Stop"

# PS 5.1 (shipped with Windows 10) defaults to TLS 1.0 — GitHub requires 1.2+
[Net.ServicePointManager]::SecurityProtocol = `
  [Net.ServicePointManager]::SecurityProtocol -bor [Net.SecurityProtocolType]::Tls12

$Repo = "RootCX/RootCX"

$Arch = (Get-ItemProperty 'HKLM:\SYSTEM\CurrentControlSet\Control\Session Manager\Environment').PROCESSOR_ARCHITECTURE
$Target = switch ($Arch) {
  "AMD64" { "x86_64-pc-windows-msvc" }
  "ARM64" {
    Write-Host "error: Windows ARM64 is not available yet" -ForegroundColor Red
    exit 1
  }
  default {
    Write-Host "error: unsupported architecture: $Arch" -ForegroundColor Red
    exit 1
  }
}

# Registry-based User PATH edit (avoids the %expansion% corruption that
# [Environment]::SetEnvironmentVariable causes on REG_EXPAND_SZ values).
function Publish-Env {
  if (-not ("Win32.NativeMethods" -as [Type])) {
    Add-Type -Namespace Win32 -Name NativeMethods -MemberDefinition @"
[DllImport("user32.dll", SetLastError = true, CharSet = CharSet.Auto)]
public static extern IntPtr SendMessageTimeout(
    IntPtr hWnd, uint Msg, UIntPtr wParam, string lParam,
    uint fuFlags, uint uTimeout, out UIntPtr lpdwResult);
"@
  }
  $r = [UIntPtr]::Zero
  [Win32.NativeMethods]::SendMessageTimeout(
    [IntPtr]0xffff, 0x1a, [UIntPtr]::Zero, "Environment", 2, 5000, [ref]$r) | Out-Null
}

function Get-UserPath {
  $key = (Get-Item 'HKCU:').OpenSubKey('Environment')
  $key.GetValue('Path', $null, [Microsoft.Win32.RegistryValueOptions]::DoNotExpandEnvironmentNames)
}

function Set-UserPath([string]$Value) {
  $key = (Get-Item 'HKCU:').OpenSubKey('Environment', $true)
  $kind = if ($Value.Contains('%')) {
    [Microsoft.Win32.RegistryValueKind]::ExpandString
  } else {
    [Microsoft.Win32.RegistryValueKind]::String
  }
  $key.SetValue('Path', $Value, $kind)
  Publish-Env
}

$InstallDir = if ($env:ROOTCX_INSTALL) { $env:ROOTCX_INSTALL } else { "${Home}\.rootcx" }
$BinDir = "${InstallDir}\bin"
$null = New-Item -ItemType Directory -Force -Path $BinDir

# Resolve the latest CLI release, not the latest release of another RootCX component.
if ($Version -eq "latest") {
  $json = & curl.exe -fsSL "https://api.github.com/repos/${Repo}/releases?per_page=100"
  if ($LASTEXITCODE -ne 0 -or -not $json) {
    Write-Host "error: could not query GitHub releases API" -ForegroundColor Red
    exit 1
  }
  $Version = (($json | ConvertFrom-Json) | Where-Object { $_.tag_name -like 'cli-v*' } | Select-Object -First 1).tag_name
  if (-not $Version) {
    Write-Host "error: could not determine latest CLI version" -ForegroundColor Red
    exit 1
  }
} elseif ($Version -notlike 'cli-v*') {
  $Version = "cli-v$($Version -replace '^v', '')"
}

$Archive = "rootcx-${Target}.tar.gz"
$ReleaseUrl = "https://github.com/${Repo}/releases/download/${Version}"
$TempDir = Join-Path $InstallDir "install-$([Guid]::NewGuid().ToString('N'))"
$ArchivePath = Join-Path $TempDir $Archive
$ChecksumsPath = Join-Path $TempDir 'SHA256SUMS'
$ExtractDir = Join-Path $TempDir 'extracted'

Write-Host "installing rootcx ${Version} (${Target})" -ForegroundColor DarkGray

try {
  $null = New-Item -ItemType Directory -Force -Path $TempDir, $ExtractDir

  # curl.exe is noticeably faster than Invoke-WebRequest on PS5.
  & curl.exe "-#SfLo" $ArchivePath "$ReleaseUrl/$Archive"
  if ($LASTEXITCODE -ne 0) {
    Invoke-RestMethod -Uri "$ReleaseUrl/$Archive" -OutFile $ArchivePath
  }
  & curl.exe "-fsLo" $ChecksumsPath "$ReleaseUrl/SHA256SUMS"
  if ($LASTEXITCODE -ne 0) {
    Remove-Item -Force $ChecksumsPath -ErrorAction SilentlyContinue
  }

  if (Test-Path $ChecksumsPath) {
    $escapedArchive = [Regex]::Escape($Archive)
    $checksumLine = Get-Content $ChecksumsPath | Where-Object { $_ -match "^([0-9a-fA-F]{64})\s+\*?${escapedArchive}$" } | Select-Object -First 1
    if ($checksumLine) {
      $expected = ([Regex]::Match($checksumLine, '^([0-9a-fA-F]{64})')).Groups[1].Value.ToLowerInvariant()
    }
  }
  if (-not $expected) {
    $release = Invoke-RestMethod -Uri "https://api.github.com/repos/${Repo}/releases/tags/${Version}"
    $asset = $release.assets | Where-Object { $_.name -eq $Archive } | Select-Object -First 1
    if ($asset.digest -notmatch '^sha256:([0-9a-fA-F]{64})$') {
      throw "checksum for $Archive is missing"
    }
    $expected = $Matches[1].ToLowerInvariant()
  }
  $actual = (Get-FileHash -Algorithm SHA256 $ArchivePath).Hash.ToLowerInvariant()
  if ($actual -ne $expected) {
    throw "checksum verification failed for $Archive"
  }

  # tar is shipped with Windows 10 1803+ and Windows 11.
  & tar.exe -xzf $ArchivePath -C $ExtractDir
  if ($LASTEXITCODE -ne 0) {
    throw "could not extract $ArchivePath"
  }
  $ExtractedBinary = Join-Path $ExtractDir 'rootcx.exe'
  if (-not (Test-Path -PathType Leaf $ExtractedBinary)) {
    throw "rootcx.exe is missing from the release archive"
  }
  Move-Item -Force $ExtractedBinary "${BinDir}\rootcx.exe"
} catch {
  Write-Host "error: $($_.Exception.Message)" -ForegroundColor Red
  exit 1
} finally {
  Remove-Item -Recurse -Force $TempDir -ErrorAction SilentlyContinue
}

Write-Host "rootcx ${Version} installed to ${BinDir}\rootcx.exe" -ForegroundColor Green

if (-not $NoPathUpdate) {
  $userPath = Get-UserPath
  $entries = @($userPath -split ';' | Where-Object { $_ })
  if ($entries -notcontains $BinDir) {
    Set-UserPath (($entries + $BinDir) -join ';')
    $env:Path = "$BinDir;$env:Path"
    Write-Host "added ${BinDir} to user PATH" -ForegroundColor DarkGray
  }
}

Write-Host ""
Write-Host "✓" -ForegroundColor Green -NoNewline
Write-Host " rootcx installed successfully"
Write-Host ""
