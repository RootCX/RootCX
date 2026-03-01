# Runtime Distribution — Windows & Linux Portability

Status: **Plan** | Ref branch: `feat/bundle-distribution`

## Current State (macOS-only)

| Component | macOS | Linux | Windows |
|---|---|---|---|
| `pack_runtime.rs` | `.pkg` via pkgbuild | — | — |
| `prompt_runtime_install` | osascript dialog + open | Stub (error) | Stub (error) |
| `daemon.rs` spawn | `Command::spawn` | same | `creation_flags` ✓ |
| `platform/service.rs` | launchd | systemd --user ✓ | schtasks ✓ |
| `platform/bundle.rs` | cargo tauri build | same ✓ | same ✓ |
| `platform/process.rs` | lsof | /proc/net/tcp ✓ | netstat ✓ |
| `Makefile` pack-runtime | mac-arm, mac-x86 | — | — |

Cross-platform pieces already done: `spawn`, `service.rs`, `bundle.rs`, `process.rs`.
Remaining: runtime installer packaging + install prompt UX.

---

## Linux

### 1. Runtime Installer — `.tar.gz` + install script

No native package manager dependency. Simplest approach that works on all distros.

**Output**: `RootCX-Runtime-{version}-{arch}.tar.gz`

**Contents**:
```
rootcx-runtime/
├── bin/rootcx-core
├── resources/
│   ├── pg/...
│   └── bun
└── install.sh
```

**`install.sh`** (bundled inside tarball):
```bash
#!/bin/bash
set -euo pipefail
ROOTCX="${HOME}/.rootcx"
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
mkdir -p "$ROOTCX"/{bin,resources,logs}
cp "$SCRIPT_DIR/bin/rootcx-core" "$ROOTCX/bin/rootcx-core"
chmod +x "$ROOTCX/bin/rootcx-core"
cp -R "$SCRIPT_DIR/resources/"* "$ROOTCX/resources/"
"$ROOTCX/bin/rootcx-core" install --service
```

**Implementation** in `pack_runtime.rs`:
- Add `#[cfg(target_os = "linux")]` block to `run()`
- Stage files identically (bin + resources)
- Write `install.sh` instead of postinstall
- Create `.tar.gz` via `flate2` + `tar` (already in deps) instead of shelling to pkgbuild
- Pure Rust, no external tools needed

**Makefile**:
```makefile
pack-runtime-linux: require-linux deps-linux build-core-linux $(DIST)
	target/$(TARGET_LINUX)/release/rootcx-core pack-runtime

pack-runtime-linux-arm: require-linux deps-linux-arm build-core-linux-arm $(DIST)
	target/$(TARGET_LINUX_ARM)/release/rootcx-core pack-runtime
```

### 2. `prompt_runtime_install` — Linux dialog

Detection priority:
1. `zenity --question` (GNOME/GTK)
2. `kdialog --yesno` (KDE)
3. Fallback: print URL to stderr, return error

```
fn prompt_dialog_linux(title: &str, msg: &str) -> bool
```

After user confirms: `xdg-open <download-url>` to open browser.
Same polling loop as macOS: check `runtime_installed()` every 2s, 5min timeout.

User downloads `.tar.gz`, extracts, runs `./install.sh`. Polling detects `~/.rootcx/bin/rootcx-core`.

### 3. Follow-up: `.deb` / `.rpm` (optional, enterprise)

For managed deployments where sysadmins push packages via apt/yum.
Lower priority — tar.gz covers all distros.

---

## Windows

### 1. Runtime Installer — NSIS `.exe`

Tauri already uses NSIS for Studio distribution. Reuse the same toolchain.

**Output**: `RootCX-Runtime-{version}-x86_64.exe`

**Behavior**:
1. Copy `rootcx-core.exe` → `%LOCALAPPDATA%\RootCX\bin\`
2. Copy `resources\` → `%LOCALAPPDATA%\RootCX\resources\`
3. Run `rootcx-core.exe install --service` (registers schtasks via `platform/service.rs`)
4. Add `%LOCALAPPDATA%\RootCX\bin` to user PATH (optional)

**Implementation** in `pack_runtime.rs`:
- `#[cfg(target_os = "windows")]` block
- Stage files into `target/runtime-nsis/`
- Generate NSIS script from template (`.nsi` file)
- Shell out to `makensis` (requires NSIS installed on CI)
- Alternatively: use WiX for `.msi` — heavier toolchain but better enterprise fit

**NSIS script template** (embedded as `const` like POSTINSTALL):
```nsi
!include "MUI2.nsh"
Name "RootCX Runtime"
OutFile "RootCX-Runtime-${VERSION}-x86_64.exe"
InstallDir "$LOCALAPPDATA\RootCX"
Section
  SetOutPath "$INSTDIR\bin"
  File "payload\bin\rootcx-core.exe"
  SetOutPath "$INSTDIR\resources"
  File /r "payload\resources\*"
  nsExec::ExecToLog '"$INSTDIR\bin\rootcx-core.exe" install --service'
SectionEnd
```

**Makefile**:
```makefile
pack-runtime-win: require-win deps-win build-core-win $(DIST)
	target/$(TARGET_WIN)/release/rootcx-core.exe pack-runtime
```

### 2. `prompt_runtime_install` — Windows dialog

Two options (pick one):

**A. Win32 MessageBox** via `windows` crate:
```rust
#[cfg(target_os = "windows")]
{
    use windows::Win32::UI::WindowsAndMessaging::*;
    let result = unsafe { MessageBoxW(None, w!("Install RootCX Runtime?"), w!("RootCX"), MB_OKCANCEL | MB_ICONQUESTION) };
    if result != IDOK { return Err(...); }
}
```
Pro: no process spawn. Con: adds `windows` crate dep.

**B. PowerShell one-liner** (no extra deps):
```rust
Command::new("powershell").args(["-Command",
    "[System.Windows.MessageBox]::Show('Install RootCX Runtime?','RootCX','OKCancel','Question')"
])
```
Pro: zero deps. Con: ~500ms cold start for powershell.

Recommend **B** — consistent with osascript pattern, zero deps.

After confirm: `open::that(url)` (already works on Windows).
Same polling loop: check `runtime_installed()`.

### 3. Path considerations

| Concern | Resolution |
|---|---|
| `.exe` suffix | `binary_name()` already handles via `cfg!(windows)` |
| `~/.rootcx` location | `dirs.rs` uses `%LOCALAPPDATA%\RootCX` on Windows |
| `tar` archive in bundle | `flate2` + `tar` crates are pure Rust — no platform issues |
| Process spawn flags | Already handled: `CREATE_NEW_PROCESS_GROUP \| CREATE_NO_WINDOW` |
| Service registration | `platform/service.rs` uses `schtasks` on Windows |
| NSIS availability on CI | GitHub Actions `windows-latest` needs `choco install nsis` step |

---

## Refactoring `pack_runtime.rs`

Current structure: one monolithic function with macOS-specific code.

Target structure:
```
core/src/pack_runtime.rs         — pub fn run() dispatches to platform module
core/src/pack_runtime/macos.rs   — pkgbuild + productbuild
core/src/pack_runtime/linux.rs   — tar.gz + install.sh
core/src/pack_runtime/windows.rs — NSIS script generation
```

Shared logic stays in `run()`:
- Stage bin + resources into `target/runtime-pkg/payload/`
- Call platform-specific `build_installer(staging, version, arch) -> PathBuf`

Each platform module only implements `build_installer`.

---

## CI Matrix

```yaml
strategy:
  matrix:
    include:
      - os: macos-latest
        target: aarch64-apple-darwin
        pack: pack-runtime-mac-arm
      - os: macos-13
        target: x86_64-apple-darwin
        pack: pack-runtime-mac-x86
      - os: ubuntu-latest
        target: x86_64-unknown-linux-gnu
        pack: pack-runtime-linux
      - os: windows-latest
        target: x86_64-pc-windows-msvc
        pack: pack-runtime-win
```

Windows step needs: `choco install nsis` before `make pack-runtime-win`.

---

## Priority Order

1. Linux `.tar.gz` installer — lowest effort, most user demand
2. Linux dialog (`zenity` / `kdialog` / fallback)
3. Refactor `pack_runtime.rs` into platform modules (before adding Windows)
4. Windows NSIS installer
5. Windows dialog (PowerShell)
6. CI matrix for all platforms
7. (Optional) `.deb` / `.rpm` / `.msi` for enterprise
