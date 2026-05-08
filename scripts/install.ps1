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
  "ARM64" { "aarch64-pc-windows-msvc" }
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

# Resolve version — use curl.exe for consistency with download path (avoids PS 5.1 TLS/UA quirks)
if ($Version -eq "latest") {
  $json = & curl.exe -fsSL "https://api.github.com/repos/${Repo}/releases/latest"
  if ($LASTEXITCODE -ne 0 -or -not $json) {
    Write-Host "error: could not query GitHub releases API" -ForegroundColor Red
    exit 1
  }
  $Version = ($json | ConvertFrom-Json).tag_name
  if (-not $Version) {
    Write-Host "error: could not determine latest version" -ForegroundColor Red
    exit 1
  }
}

$Archive = "rootcx-${Target}.tar.gz"
$Url = "https://github.com/${Repo}/releases/download/${Version}/${Archive}"
$ArchivePath = "${BinDir}\${Archive}"

Write-Host "installing rootcx ${Version} (${Target})" -ForegroundColor DarkGray

Remove-Item -Force $ArchivePath -ErrorAction SilentlyContinue

# curl.exe is noticeably faster than Invoke-WebRequest on PS5
& curl.exe "-#SfLo" $ArchivePath $Url
if ($LASTEXITCODE -ne 0) {
  try {
    Invoke-RestMethod -Uri $Url -OutFile $ArchivePath
  } catch {
    Write-Host "error: could not download $Url" -ForegroundColor Red
    exit 1
  }
}

# tar is shipped with Windows 10 1803+ and Windows 11
& tar.exe -xzf $ArchivePath -C $BinDir
if ($LASTEXITCODE -ne 0) {
  Write-Host "error: could not extract $ArchivePath" -ForegroundColor Red
  exit 1
}
Remove-Item -Force $ArchivePath

if (-not (Test-Path "${BinDir}\rootcx.exe")) {
  Write-Host "error: rootcx.exe missing after extraction" -ForegroundColor Red
  exit 1
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
