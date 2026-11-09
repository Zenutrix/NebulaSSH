//! Serielle Verbindungen (COM-Ports).

use crate::session::{Cmd, Entry, Sessions};
use std::io::{ErrorKind, Read, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::Arc;
use std::time::Duration;
use tauri::ipc::{Channel, InvokeResponseBody};

pub struct SerialSession {
    pub tx: mpsc::Sender<Cmd>,
    stop: Arc<AtomicBool>,
}

impl SerialSession {
    pub fn close(&self) {
        self.stop.store(true, Ordering::SeqCst);
        let _ = self.tx.send(Cmd::Close);
    }
}

pub fn list_ports() -> Vec<String> {
    let mut ports: Vec<String> = serialport::available_ports()
        .map(|p| p.into_iter().map(|p| p.port_name).collect())
        .unwrap_or_default();
    // COM3 vor COM10 sortieren
    ports.sort_by_key(|p| {
        let num: u32 = p.trim_start_matches(|c: char| !c.is_ascii_digit()).parse().unwrap_or(0);
        (p.trim_end_matches(|c: char| c.is_ascii_digit()).to_string(), num)
    });
    ports
}

pub fn connect(
    sessions: Sessions,
    id: String,
    port_name: String,
    baud: u32,
    on_data: Channel<InvokeResponseBody>,
    on_close: Channel<String>,
) -> Result<String, String> {
    let port = serialport::new(&port_name, baud)
        .timeout(Duration::from_millis(20))
        .open()
        .map_err(|e| format!("Serial Fehler: {e}"))?;
    let mut reader = port
        .try_clone()
        .map_err(|e| format!("Serial Fehler: {e}"))?;
    let mut writer = port;

    let (tx, rx) = mpsc::channel::<Cmd>();
    let stop = Arc::new(AtomicBool::new(false));
    let sess = Arc::new(SerialSession {
        tx,
        stop: stop.clone(),
    });
    sessions
        .lock()
        .unwrap()
        .insert(id.clone(), Entry::Serial(sess.clone()));

    std::thread::Builder::new()
        .name(format!("serial-w-{port_name}"))
        .spawn(move || {
            while let Ok(cmd) = rx.recv() {
                match cmd {
                    Cmd::Data(d) => {
                        if writer.write_all(&d).is_err() {
                            break;
                        }
                        let _ = writer.flush();
                    }
                    Cmd::Resize(..) => {}
                    Cmd::Close => break,
                }
            }
        })
        .map_err(|e| e.to_string())?;

    std::thread::Builder::new()
        .name(format!("serial-r-{port_name}"))
        .spawn(move || {
            let mut buf = vec![0u8; 16 * 1024];
            while !stop.load(Ordering::SeqCst) {
                match reader.read(&mut buf) {
                    Ok(0) => {}
                    Ok(n) => {
                        if on_data
                            .send(InvokeResponseBody::Raw(buf[..n].to_vec()))
                            .is_err()
                        {
                            break;
                        }
                    }
                    Err(e) if e.kind() == ErrorKind::TimedOut || e.kind() == ErrorKind::Interrupted => {}
                    Err(_) => break,
                }
            }
            {
                let mut map = sessions.lock().unwrap();
                if matches!(map.get(&id), Some(Entry::Serial(s)) if Arc::ptr_eq(s, &sess)) {
                    map.remove(&id);
                }
            }
            sess.close();
            let _ = on_close.send("\r\n\x1b[90m[Port geschlossen]\x1b[0m\r\n".into());
        })
        .map_err(|e| e.to_string())?;

    Ok("Verbunden!".into())
}
