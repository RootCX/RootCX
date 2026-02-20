use std::io::{Read, Write};
use std::sync::Arc;

use portable_pty::{CommandBuilder, MasterPty, PtySize, native_pty_system};
use tokio::sync::Mutex;

#[derive(Default)]
pub struct TerminalState {
    writer: Option<Arc<Mutex<Box<dyn Write + Send>>>>,
    master: Option<Arc<Mutex<Box<dyn MasterPty + Send>>>>,
}

impl TerminalState {
    pub fn spawn(
        &mut self,
        cwd: Option<&str>,
        rows: u16,
        cols: u16,
        channel: tauri::ipc::Channel<Vec<u8>>,
    ) -> Result<(), String> {
        self.writer = None;
        self.master = None;

        let size = PtySize { rows, cols, pixel_width: 0, pixel_height: 0 };
        let pair = native_pty_system().openpty(size).map_err(|e| e.to_string())?;

        let mut cmd = CommandBuilder::new_default_prog();
        if let Some(dir) = cwd {
            cmd.cwd(dir);
        }

        let _child = pair.slave.spawn_command(cmd).map_err(|e| e.to_string())?;
        drop(pair.slave);

        let mut reader = pair.master.try_clone_reader().map_err(|e| e.to_string())?;
        let writer = pair.master.take_writer().map_err(|e| e.to_string())?;

        self.writer = Some(Arc::new(Mutex::new(writer)));
        self.master = Some(Arc::new(Mutex::new(pair.master)));

        tokio::task::spawn_blocking(move || {
            let mut buf = [0u8; 4096];
            loop {
                match reader.read(&mut buf) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        if channel.send(buf[..n].to_vec()).is_err() {
                            break;
                        }
                    }
                }
            }
        });

        Ok(())
    }

    pub async fn write(&self, data: &[u8]) -> Result<(), String> {
        let writer = self.writer.as_ref().ok_or("no terminal session")?;
        let mut w = writer.lock().await;
        w.write_all(data).map_err(|e| e.to_string())?;
        w.flush().map_err(|e| e.to_string())
    }

    pub async fn resize(&self, rows: u16, cols: u16) -> Result<(), String> {
        let master = self.master.as_ref().ok_or("no terminal session")?;
        master.lock().await.resize(PtySize { rows, cols, pixel_width: 0, pixel_height: 0 }).map_err(|e| e.to_string())
    }
}
