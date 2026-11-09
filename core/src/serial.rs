//! Serielle Verbindungen (COM-Ports).

use crate::{Cmd, Connection, Event, Sink};
use std::io::{ErrorKind, Read, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::Duration;

pub struct SerialSession {
    tx: Mutex<mpsc::Sender<Cmd>>,
    stop: Arc<AtomicBool>,
}

impl Connection for SerialSession {
    fn send(&self, cmd: Cmd) {
        if matches!(cmd, Cmd::Close) {
            self.stop.store(true, Ordering::SeqCst);
        }
        let _ = self.tx.lock().unwrap().send(cmd);
    }
}

pub fn list_ports() -> Vec<String> {
    let mut ports: Vec<String> = serialport::available_ports()
        .map(|p| p.into_iter().map(|p| p.port_name).collect())
        .unwrap_or_default();
    // COM3 vor COM10 sortieren
    ports.sort_by_key(|p| {
        let num: u32 = p
            .trim_start_matches(|c: char| !c.is_ascii_digit())
            .parse()
            .unwrap_or(0);
        (p.trim_end_matches(|c: char| c.is_ascii_digit()).to_string(), num)
    });
    ports
}

pub fn connect(port_name: &str, baud: u32, sink: Sink) -> Result<Arc<SerialSession>, String> {
    let port = serialport::new(port_name, baud)
        .timeout(Duration::from_millis(20))
        .open()
        .map_err(|e| format!("Serial Fehler: {e}"))?;
    let mut reader = port.try_clone().map_err(|e| format!("Serial Fehler: {e}"))?;
    let mut writer = port;

    let (tx, rx) = mpsc::channel::<Cmd>();
    let stop = Arc::new(AtomicBool::new(false));
    let sess = Arc::new(SerialSession {
        tx: Mutex::new(tx),
        stop: stop.clone(),
    });

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

    let sess2 = sess.clone();
    std::thread::Builder::new()
        .name(format!("serial-r-{port_name}"))
        .spawn(move || {
            let mut buf = vec![0u8; 16 * 1024];
            while !stop.load(Ordering::SeqCst) {
                match reader.read(&mut buf) {
                    Ok(0) => {}
                    Ok(n) => sink(Event::Data(buf[..n].to_vec())),
                    Err(e)
                        if e.kind() == ErrorKind::TimedOut || e.kind() == ErrorKind::Interrupted => {}
                    Err(_) => break,
                }
            }
            sess2.close();
            sink(Event::Closed("[Port geschlossen]".into()));
        })
        .map_err(|e| e.to_string())?;

    Ok(sess)
}
