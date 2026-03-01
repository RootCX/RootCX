mod install;
mod pack_runtime;

use std::path::PathBuf;
use std::sync::Arc;

use rootcx_core::{Runtime, server};
use rootcx_platform::service::ServiceConfig;
use rootcx_postgres_mgmt::{PostgresManager, data_base_dir};
use tokio::sync::Mutex;
use tracing_subscriber::EnvFilter;

const PG_PORT:  u16 = 5480;
const API_PORT: u16 = rootcx_platform::DEFAULT_API_PORT;

pub(crate) const SVC_NAME:  &str = "rootcx-core";
pub(crate) const SVC_LABEL: &str = "com.rootcx.core";
pub(crate) const SVC_DESC:  &str = "RootCX Core Runtime Daemon";

pub(crate) fn die(msg: impl std::fmt::Display) -> ! { eprintln!("error: {msg}"); std::process::exit(1) }

fn home() -> PathBuf {
    rootcx_platform::dirs::rootcx_home().unwrap_or_else(|e| die(e))
}

fn resources() -> PathBuf {
    rootcx_platform::dirs::resources_dir(env!("CARGO_MANIFEST_DIR"))
        .unwrap_or_else(|e| die(e))
}

fn resolve_pg() -> PathBuf {
    let res = resources();
    std::fs::read_dir(&res)
        .unwrap_or_else(|e| die(format!("resources {}: {e}", res.display())))
        .flatten()
        .find_map(|e| {
            let p = e.path();
            (p.is_dir() && rootcx_platform::bin::binary_path(&p.join("bin"), "pg_ctl").exists())
                .then_some(p)
        })
        .unwrap_or_else(|| die(format!("no PostgreSQL bundle in {}", res.display())))
}

fn resolve_bun() -> PathBuf {
    let p = rootcx_platform::bin::binary_path(&resources(), "bun");
    if !p.is_file() { die(format!("Bun not found at {}", p.display())) }
    p
}

fn service_config() -> ServiceConfig {
    let h = home();
    ServiceConfig {
        name: SVC_NAME, label: SVC_LABEL, description: SVC_DESC,
        binary:   rootcx_platform::bin::binary_path(&h.join("bin"), SVC_NAME),
        args:     &["--daemon"],
        log_file: h.join("logs/runtime.log"),
    }
}

fn handle_service(sub: &str) {
    let cfg = service_config();
    match sub {
        "status" => println!("{}", rootcx_platform::service::status(&cfg).unwrap_or_else(|e| die(e))),
        "start"  => { rootcx_platform::service::start(&cfg).unwrap_or_else(|e| die(e)); println!("started"); }
        "stop"   => { rootcx_platform::service::stop(&cfg).unwrap_or_else(|e| die(e)); println!("stopped"); }
        "uninstall" => { rootcx_platform::service::uninstall(&cfg).unwrap_or_else(|e| die(e)); println!("uninstalled"); }
        _ => { eprintln!("usage: rootcx-core service <status|start|stop|uninstall>"); std::process::exit(1); }
    }
}

#[tokio::main]
async fn main() {
    let args: Vec<String> = std::env::args().collect();
    match args.get(1).map(String::as_str) {
        Some("install") => {
            install::run(home(), resolve_pg(), resolve_bun(), args.iter().any(|a| a == "--service"));
            return;
        }
        Some("service") => {
            handle_service(args.get(2).map(String::as_str).unwrap_or("status"));
            return;
        }
        Some("bundle") => {
            let app_dir = args.get(2).map(PathBuf::from)
                .unwrap_or_else(|| die("usage: rootcx-core bundle <app-dir>"));
            match rootcx_platform::bundle::run(app_dir, &|msg| eprintln!("{msg}")) {
                Ok(p) => eprintln!("[bundle] done → {}", p.display()),
                Err(e) => die(e),
            }
            return;
        }
        Some("pack-runtime") => {
            pack_runtime::run(
                std::env::current_exe().unwrap_or_else(|e| die(e)),
                resources(),
            );
            return;
        }
        _ => {}
    }

    let daemon   = args.iter().any(|a| a == "--daemon");
    let pid_file = home().join("runtime.pid");

    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .init();

    let pg_root  = resolve_pg();
    let data_dir = data_base_dir().unwrap_or_else(|e| die(e));
    let pg = PostgresManager::new(pg_root.join("bin"), data_dir.join("data/pg"), PG_PORT)
        .with_lib_dir(pg_root.join("lib"));
    let rt = Arc::new(Mutex::new(Runtime::new(pg, data_dir, resolve_bun())));

    rt.lock().await.boot(API_PORT).await.unwrap_or_else(|e| {
        tracing::error!("boot: {e}"); std::process::exit(1);
    });

    if daemon { let _ = std::fs::write(&pid_file, std::process::id().to_string()); }

    let (rt2, pf2) = (Arc::clone(&rt), pid_file.clone());
    tokio::spawn(async move {
        tokio::signal::ctrl_c().await.ok();
        if daemon { let _ = std::fs::remove_file(&pf2); }
        rt2.lock().await.shutdown().await.ok();
        std::process::exit(0);
    });

    if let Err(e) = server::serve(rt, API_PORT).await {
        tracing::error!("server: {e}");
        if daemon { let _ = std::fs::remove_file(&pid_file); }
        std::process::exit(1);
    }
}
