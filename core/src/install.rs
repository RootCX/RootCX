use std::path::PathBuf;
use rootcx_platform::service::{ServiceConfig, ServiceStatus};
use super::{die, SVC_NAME, SVC_LABEL, SVC_DESC};

pub fn run(home: PathBuf, bun_bin: PathBuf, with_service: bool) {
    let [bin_dir, res_dir, log_dir] = ["bin", "resources", "logs"].map(|d| home.join(d));
    for d in [&bin_dir, &res_dir, &log_dir] {
        std::fs::create_dir_all(d).unwrap_or_else(|e| die(format!("mkdir {}: {e}", d.display())));
    }

    let self_exe = std::env::current_exe().unwrap_or_else(|e| die(e));
    let daemon   = rootcx_platform::bin::binary_path(&bin_dir, SVC_NAME);
    std::fs::copy(&self_exe, &daemon).unwrap_or_else(|e| die(format!("copy daemon: {e}")));
    let _ = rootcx_platform::fs::set_executable(&daemon);

    let bun_dest = rootcx_platform::bin::binary_path(&res_dir, "bun");
    std::fs::copy(&bun_bin, &bun_dest).unwrap_or_else(|e| die(format!("copy bun: {e}")));
    let _ = rootcx_platform::fs::set_executable(&bun_dest);

    println!("Installed → {}", home.display());

    if with_service {
        let cfg = ServiceConfig {
            name: SVC_NAME, label: SVC_LABEL, description: SVC_DESC,
            binary:   daemon,
            args:     &["--daemon"],
            log_file: log_dir.join("runtime.log"),
        };
        if rootcx_platform::service::status(&cfg).ok() == Some(ServiceStatus::Running) {
            return println!("Service already running.");
        }
        let _ = rootcx_platform::service::uninstall(&cfg);
        rootcx_platform::service::install(&cfg).unwrap_or_else(|e| die(e));
        println!("Service registered — starts on login.");
    }
}
