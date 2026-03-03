use std::path::PathBuf;
use crate::PlatformError;

pub struct ServiceConfig {
    pub name:        &'static str,
    pub label:       &'static str, // reverse-DNS label (used on macOS)
    pub description: &'static str,
    pub binary:      PathBuf,
    pub args:        &'static [&'static str],
    pub log_file:    PathBuf,
}

#[derive(Debug, PartialEq)]
pub enum ServiceStatus { Running, Stopped, NotInstalled }

impl std::fmt::Display for ServiceStatus {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Running      => "running",
            Self::Stopped      => "stopped",
            Self::NotInstalled => "not installed",
        })
    }
}

pub fn install(c: &ServiceConfig)   -> Result<(), PlatformError> { imp::install(c) }
pub fn uninstall(c: &ServiceConfig) -> Result<(), PlatformError> { imp::uninstall(c) }
pub fn start(c: &ServiceConfig)     -> Result<(), PlatformError> { imp::start(c) }
pub fn stop(c: &ServiceConfig)      -> Result<(), PlatformError> { imp::stop(c) }
pub fn status(c: &ServiceConfig)    -> Result<ServiceStatus, PlatformError> { imp::status(c) }

#[cfg(target_os = "macos")]
mod imp {
    use super::{PlatformError, ServiceConfig, ServiceStatus};
    use std::path::PathBuf;

    fn plist(c: &ServiceConfig) -> Result<PathBuf, PlatformError> {
        Ok(crate::dirs::home_dir()?
            .join(format!("Library/LaunchAgents/{}.plist", c.label)))
    }

    fn ctl(args: &[&str]) -> Result<(), PlatformError> {
        std::process::Command::new("launchctl").args(args).status()
            .map_err(|_| PlatformError("launchctl"))?
            .success().then_some(()).ok_or(PlatformError("launchctl"))
    }

    pub fn install(c: &ServiceConfig) -> Result<(), PlatformError> {
        let mut args_xml = format!("    <string>{}</string>\n", c.binary.display());
        for a in c.args { args_xml.push_str(&format!("    <string>{a}</string>\n")); }
        let p = plist(c)?;
        std::fs::create_dir_all(p.parent().unwrap()).map_err(|_| PlatformError("LaunchAgents dir"))?;
        std::fs::write(&p, format!(
            "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n\
             <!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \
             \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n\
             <plist version=\"1.0\"><dict>\n\
             <key>Label</key><string>{label}</string>\n\
             <key>ProgramArguments</key><array>\n{args}</array>\n\
             <key>RunAtLoad</key><true/><key>KeepAlive</key><false/>\n\
             <key>ProcessType</key><string>Background</string>\n\
             <key>StandardOutPath</key><string>{log}</string>\n\
             <key>StandardErrorPath</key><string>{log}</string>\n\
             </dict></plist>\n",
            label = c.label, args = args_xml, log = c.log_file.display()
        )).map_err(|_| PlatformError("write plist"))?;
        ctl(&["load", "-w", &p.to_string_lossy()])
    }

    pub fn uninstall(c: &ServiceConfig) -> Result<(), PlatformError> {
        let p = plist(c)?;
        if p.exists() {
            let _ = ctl(&["unload", "-w", &p.to_string_lossy()]);
            std::fs::remove_file(&p).map_err(|_| PlatformError("remove plist"))?;
        }
        Ok(())
    }

    pub fn start(c: &ServiceConfig)  -> Result<(), PlatformError> { ctl(&["start", c.label]) }
    pub fn stop(c: &ServiceConfig)   -> Result<(), PlatformError> { ctl(&["stop",  c.label]) }

    pub fn status(c: &ServiceConfig) -> Result<ServiceStatus, PlatformError> {
        if !plist(c)?.exists() { return Ok(ServiceStatus::NotInstalled); }
        let out = std::process::Command::new("launchctl")
            .args(["list", c.label]).output()
            .map_err(|_| PlatformError("launchctl list"))?;
        if !out.status.success() { return Ok(ServiceStatus::Stopped); }
        let s = String::from_utf8_lossy(&out.stdout);
        // "PID" key is only present (and non-zero) when the daemon is running
        Ok(if s.contains("\"PID\"") && !s.contains("\"PID\" = 0") {
            ServiceStatus::Running
        } else {
            ServiceStatus::Stopped
        })
    }
}

#[cfg(target_os = "linux")]
mod imp {
    use super::{PlatformError, ServiceConfig, ServiceStatus};
    use std::path::PathBuf;

    fn unit(c: &ServiceConfig) -> Result<PathBuf, PlatformError> {
        Ok(crate::dirs::home_dir()?
            .join(format!(".config/systemd/user/{}.service", c.name)))
    }

    fn ctl(args: &[&str]) -> Result<(), PlatformError> {
        std::process::Command::new("systemctl").arg("--user").args(args).status()
            .map_err(|_| PlatformError("systemctl"))?
            .success().then_some(()).ok_or(PlatformError("systemctl"))
    }

    pub fn install(c: &ServiceConfig) -> Result<(), PlatformError> {
        let p = unit(c)?;
        std::fs::create_dir_all(p.parent().unwrap()).map_err(|_| PlatformError("systemd user dir"))?;
        let exec = std::iter::once(c.binary.to_string_lossy().into_owned())
            .chain(c.args.iter().map(|s| s.to_string()))
            .collect::<Vec<_>>().join(" ");
        std::fs::write(&p, format!(
            "[Unit]\nDescription={d}\nAfter=network.target\n\n\
             [Service]\nType=simple\nExecStart={exec}\n\
             Restart=on-failure\nRestartSec=5\n\
             StandardOutput=append:{log}\nStandardError=append:{log}\n\n\
             [Install]\nWantedBy=default.target\n",
            d = c.description, log = c.log_file.display()
        )).map_err(|_| PlatformError("write unit file"))?;
        let _ = ctl(&["daemon-reload"]);
        ctl(&["enable", "--now", c.name])
    }

    pub fn uninstall(c: &ServiceConfig) -> Result<(), PlatformError> {
        let _ = ctl(&["disable", "--now", c.name]);
        let p = unit(c)?;
        if p.exists() { std::fs::remove_file(&p).map_err(|_| PlatformError("remove unit file"))?; }
        let _ = ctl(&["daemon-reload"]);
        Ok(())
    }

    pub fn start(c: &ServiceConfig)  -> Result<(), PlatformError> { ctl(&["start", c.name]) }
    pub fn stop(c: &ServiceConfig)   -> Result<(), PlatformError> { ctl(&["stop",  c.name]) }

    pub fn status(c: &ServiceConfig) -> Result<ServiceStatus, PlatformError> {
        if !unit(c)?.exists() { return Ok(ServiceStatus::NotInstalled); }
        Ok(if std::process::Command::new("systemctl")
            .args(["--user", "is-active", c.name]).status()
            .map(|s| s.success()).unwrap_or(false)
        { ServiceStatus::Running } else { ServiceStatus::Stopped })
    }
}

#[cfg(windows)]
mod imp {
    use super::{PlatformError, ServiceConfig, ServiceStatus};

    fn task(c: &ServiceConfig) -> String { format!("RootCX\\{}", c.name) }

    fn sch(args: &[&str]) -> Result<(), PlatformError> {
        std::process::Command::new("schtasks").args(args).status()
            .map_err(|_| PlatformError("schtasks"))?
            .success().then_some(()).ok_or(PlatformError("schtasks"))
    }

    pub fn install(c: &ServiceConfig) -> Result<(), PlatformError> {
        let mut tr = format!("\"{}\"", c.binary.display());
        for a in c.args { tr.push(' '); tr.push_str(a); }
        // ONLOGON + LIMITED: starts at user login with standard privileges
        sch(&["/Create", "/TN", &task(c), "/TR", &tr, "/SC", "ONLOGON", "/RL", "LIMITED", "/F"])
    }

    pub fn uninstall(c: &ServiceConfig) -> Result<(), PlatformError> {
        sch(&["/Delete", "/TN", &task(c), "/F"])
    }

    pub fn start(c: &ServiceConfig)  -> Result<(), PlatformError> { sch(&["/Run", "/TN", &task(c)]) }
    pub fn stop(c: &ServiceConfig)   -> Result<(), PlatformError> { sch(&["/End", "/TN", &task(c)]) }

    pub fn status(c: &ServiceConfig) -> Result<ServiceStatus, PlatformError> {
        let out = std::process::Command::new("schtasks")
            .args(["/Query", "/TN", &task(c), "/FO", "CSV", "/NH"])
            .output().map_err(|_| PlatformError("schtasks /Query"))?;
        if !out.status.success() { return Ok(ServiceStatus::NotInstalled); }
        Ok(if String::from_utf8_lossy(&out.stdout).contains("Running") {
            ServiceStatus::Running
        } else {
            ServiceStatus::Stopped
        })
    }
}
