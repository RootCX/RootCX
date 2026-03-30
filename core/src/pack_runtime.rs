use std::fs;
use std::path::PathBuf;
use std::process::Command;
use super::die;

const PKG_ID: &str = "com.rootcx.runtime";

pub fn run(core_binary: PathBuf, resources_dir: PathBuf) {
    let version = env!("CARGO_PKG_VERSION");
    let arch = rootcx_platform::bin::TARGET_TRIPLE;
    let staging = PathBuf::from("target/runtime-pkg");
    let _ = fs::remove_dir_all(&staging);

    let (payload, scripts) = (staging.join("payload"), staging.join("scripts"));
    for d in [&payload, &scripts] { fs::create_dir_all(d).unwrap_or_else(|e| die(format!("mkdir {}: {e}", d.display()))); }

    eprintln!("[pack-runtime] staging payload");
    let bin_dir = payload.join("bin");
    fs::create_dir_all(&bin_dir).unwrap_or_else(|e| die(e));
    let dest_bin = rootcx_platform::bin::binary_path(&bin_dir, "rootcx-core");
    fs::copy(&core_binary, &dest_bin).unwrap_or_else(|e| die(format!("copy core: {e}")));
    let _ = rootcx_platform::fs::set_executable(&dest_bin);
    rootcx_platform::fs::copy_dir(&resources_dir, &payload.join("resources")).unwrap_or_else(|e| die(format!("copy resources: {e}")));

    eprintln!("[pack-runtime] writing postinstall script");
    fs::write(scripts.join("postinstall"), POSTINSTALL).unwrap_or_else(|e| die(format!("write postinstall: {e}")));
    let _ = rootcx_platform::fs::set_executable(&scripts.join("postinstall"));

    eprintln!("[pack-runtime] running pkgbuild");
    run_cmd("pkgbuild", &[
        "--root", payload.to_str().expect("non-UTF-8 path"), "--identifier", PKG_ID, "--version", version,
        "--scripts", scripts.to_str().expect("non-UTF-8 path"), "--install-location", "/tmp/rootcx-install",
        staging.join("component.pkg").to_str().expect("non-UTF-8 path"),
    ]);

    fs::write(staging.join("distribution.xml"), format!(
        r#"<?xml version="1.0" encoding="utf-8"?>
<installer-gui-script minSpecVersion="2">
    <title>RootCX Runtime {version}</title>
    <welcome language="en" mime-type="text/plain"><![CDATA[RootCX Runtime v{version}
Installs core runtime and Bun. Starts automatically on login.]]></welcome>
    <options customize="never" require-scripts="false" />
    <choices-outline><line choice="default" /></choices-outline>
    <choice id="default" title="RootCX Runtime"><pkg-ref id="{PKG_ID}" /></choice>
    <pkg-ref id="{PKG_ID}" version="{version}">component.pkg</pkg-ref>
</installer-gui-script>"#
    )).unwrap_or_else(|e| die(format!("write distribution.xml: {e}")));

    let dist_dir = PathBuf::from("target/dist");
    fs::create_dir_all(&dist_dir).unwrap_or_else(|e| die(e));
    let output = dist_dir.join(format!("RootCX-Runtime-{version}-{arch}.pkg"));

    eprintln!("[pack-runtime] running productbuild");
    run_cmd("productbuild", &[
        "--distribution", staging.join("distribution.xml").to_str().expect("non-UTF-8 path"),
        "--package-path", staging.to_str().expect("non-UTF-8 path"), output.to_str().expect("non-UTF-8 path"),
    ]);
    eprintln!("[pack-runtime] done → {}", output.display());
}

fn run_cmd(program: &str, args: &[&str]) {
    let s = Command::new(program).args(args)
        .stderr(std::process::Stdio::inherit())
        .status().unwrap_or_else(|e| die(format!("{program}: {e}")));
    if !s.success() { die(format!("{program} failed")); }
}

const POSTINSTALL: &str = r#"#!/bin/bash
set -euo pipefail
if [ -n "${USER:-}" ]; then HOME_DIR=$(eval echo "~$USER"); else HOME_DIR="$HOME"; fi
ROOTCX="$HOME_DIR/.rootcx"
INSTALL_DIR="$2"
mkdir -p "$ROOTCX"/{bin,resources,logs}
cp "$INSTALL_DIR/bin/rootcx-core" "$ROOTCX/bin/rootcx-core"
chmod +x "$ROOTCX/bin/rootcx-core"
cp -R "$INSTALL_DIR/resources/"* "$ROOTCX/resources/"
sudo -u "$USER" "$ROOTCX/bin/rootcx-core" install --service
exit 0
"#;
