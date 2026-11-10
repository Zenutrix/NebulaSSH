//! Telnet (RFC 854) für ältere Switches, Router und Konsolenserver.
//! Unterstützt ECHO, SUPPRESS-GO-AHEAD, TERMINAL-TYPE und NAWS (Fenstergröße).

use crate::{tf, Cmd, Connection, Event, Sink};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::mpsc;

const IAC: u8 = 255;
const DONT: u8 = 254;
const DO: u8 = 253;
const WONT: u8 = 252;
const WILL: u8 = 251;
const SB: u8 = 250;
const SE: u8 = 240;
const OPT_ECHO: u8 = 1;
const OPT_SGA: u8 = 3;
const OPT_TTYPE: u8 = 24;
const OPT_NAWS: u8 = 31;

pub struct TelnetSession {
    tx: mpsc::UnboundedSender<Out>,
}

enum Out {
    Cmd(Cmd),
    Raw(Vec<u8>),
}

impl Connection for TelnetSession {
    fn send(&self, cmd: Cmd) {
        let _ = self.tx.send(Out::Cmd(cmd));
    }
}

fn naws(cols: u32, rows: u32) -> Vec<u8> {
    let mut v = vec![IAC, SB, OPT_NAWS];
    for b in [(cols as u16).to_be_bytes(), (rows as u16).to_be_bytes()].concat() {
        v.push(b);
        if b == IAC {
            v.push(IAC);
        }
    }
    v.extend([IAC, SE]);
    v
}

/// Zustandsautomat für eingehende Daten: trennt Nutzdaten und Verhandlung.
pub struct Parser {
    state: u8,
    sb: Vec<u8>,
    /// Optionen, die wir eingeschaltet haben (WILL gesendet)
    ours: [bool; 256],
    /// Optionen, die der Server eingeschaltet hat (DO gesendet)
    theirs: [bool; 256],
    pub naws_on: bool,
}

impl Default for Parser {
    fn default() -> Self {
        Self { state: 0, sb: Vec::new(), ours: [false; 256], theirs: [false; 256], naws_on: false }
    }
}

impl Parser {
    /// Liefert (Nutzdaten, Antworten an den Server).
    pub fn feed(&mut self, data: &[u8], cols: u32, rows: u32) -> (Vec<u8>, Vec<u8>) {
        let mut out = Vec::with_capacity(data.len());
        let mut reply = Vec::new();
        for &b in data {
            match self.state {
                0 => {
                    if b == IAC {
                        self.state = 1;
                    } else {
                        out.push(b);
                    }
                }
                1 => {
                    self.state = match b {
                        IAC => {
                            out.push(IAC);
                            0
                        }
                        DO | DONT | WILL | WONT => b,
                        SB => {
                            self.sb.clear();
                            10
                        }
                        _ => 0, // NOP, GA usw.
                    }
                }
                DO => {
                    self.state = 0;
                    let ok = matches!(b, OPT_TTYPE | OPT_NAWS | OPT_SGA);
                    if ok && !self.ours[b as usize] {
                        self.ours[b as usize] = true;
                        reply.extend([IAC, WILL, b]);
                    } else if !ok {
                        reply.extend([IAC, WONT, b]);
                    }
                    if b == OPT_NAWS && ok {
                        self.naws_on = true;
                        reply.extend(naws(cols, rows));
                    }
                }
                DONT => {
                    self.state = 0;
                    if self.ours[b as usize] {
                        self.ours[b as usize] = false;
                        reply.extend([IAC, WONT, b]);
                    }
                    if b == OPT_NAWS {
                        self.naws_on = false;
                    }
                }
                WILL => {
                    self.state = 0;
                    let ok = matches!(b, OPT_ECHO | OPT_SGA);
                    if ok && !self.theirs[b as usize] {
                        self.theirs[b as usize] = true;
                        reply.extend([IAC, DO, b]);
                    } else if !ok {
                        reply.extend([IAC, DONT, b]);
                    }
                }
                WONT => {
                    self.state = 0;
                    if self.theirs[b as usize] {
                        self.theirs[b as usize] = false;
                        reply.extend([IAC, DONT, b]);
                    }
                }
                10 => {
                    if b == IAC {
                        self.state = 11;
                    } else if self.sb.len() < 512 {
                        self.sb.push(b);
                    }
                }
                11 => {
                    if b == SE {
                        self.state = 0;
                        // TERMINAL-TYPE SEND
                        if self.sb.first() == Some(&OPT_TTYPE) && self.sb.get(1) == Some(&1) {
                            reply.extend([IAC, SB, OPT_TTYPE, 0]);
                            reply.extend(b"XTERM-256COLOR");
                            reply.extend([IAC, SE]);
                        }
                    } else {
                        if b == IAC {
                            self.sb.push(IAC);
                        }
                        self.state = 10;
                    }
                }
                _ => self.state = 0,
            }
        }
        (out, reply)
    }
}

/// Eingaben für Telnet aufbereiten: 0xFF verdoppeln, Enter als CR LF senden.
pub fn encode_input(data: &[u8]) -> Vec<u8> {
    let mut v = Vec::with_capacity(data.len() + 4);
    for &b in data {
        match b {
            IAC => v.extend([IAC, IAC]),
            b'\r' => v.extend(b"\r\n"),
            _ => v.push(b),
        }
    }
    v
}

pub async fn connect(addr: String, cols: u32, rows: u32, sink: Sink) -> Result<Arc<TelnetSession>, String> {
    let (host, port) = crate::ssh::split_host_port_or(&addr, 23);
    let tcp = tokio::time::timeout(Duration::from_secs(10), tokio::net::TcpStream::connect((host.as_str(), port)))
        .await
        .map_err(|_| tf!("Zeitüberschreitung ({host}:{port})", "Timeout ({host}:{port})"))?
        .map_err(|e| tf!("Fehler beim Verbinden: {e}", "Connection failed: {e}"))?;
    let _ = tcp.set_nodelay(true);
    let (mut rd, mut wr) = tcp.into_split();
    let (tx, mut rx) = mpsc::unbounded_channel::<Out>();
    let sess = Arc::new(TelnetSession { tx: tx.clone() });
    let size = Arc::new(std::sync::Mutex::new((cols, rows, false)));

    let size_w = size.clone();
    tokio::spawn(async move {
        while let Some(o) = rx.recv().await {
            let bytes = match o {
                Out::Raw(b) => b,
                Out::Cmd(Cmd::Data(d)) => encode_input(&d),
                Out::Cmd(Cmd::Resize(c, r)) => {
                    let mut s = size_w.lock().unwrap();
                    s.0 = c;
                    s.1 = r;
                    if !s.2 {
                        continue;
                    }
                    naws(c, r)
                }
                Out::Cmd(Cmd::Close) => break,
            };
            if wr.write_all(&bytes).await.is_err() {
                break;
            }
        }
        let _ = wr.shutdown().await;
    });

    tokio::spawn(async move {
        let mut parser = Parser::default();
        let mut buf = vec![0u8; 32 * 1024];
        loop {
            match rd.read(&mut buf).await {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    let (c, r) = {
                        let s = size.lock().unwrap();
                        (s.0, s.1)
                    };
                    let (data, reply) = parser.feed(&buf[..n], c, r);
                    size.lock().unwrap().2 = parser.naws_on;
                    if !reply.is_empty() {
                        let _ = tx.send(Out::Raw(reply));
                    }
                    if !data.is_empty() {
                        sink(Event::Data(data));
                    }
                }
            }
        }
        let _ = tx.send(Out::Cmd(Cmd::Close));
        sink(Event::Closed(crate::t!("[Verbindung getrennt]", "[Disconnected]").into()));
    });
    Ok(sess)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn negotiation() {
        let mut p = Parser::default();
        // Server: WILL ECHO, DO NAWS, DO LINEMODE(34), Text mit verdoppeltem IAC
        let (data, reply) = p.feed(&[IAC, WILL, OPT_ECHO, IAC, DO, OPT_NAWS, IAC, DO, 34, b'h', b'i', IAC, IAC], 80, 24);
        assert_eq!(data, vec![b'h', b'i', IAC]);
        assert_eq!(&reply[..3], &[IAC, DO, OPT_ECHO]);
        assert_eq!(&reply[3..6], &[IAC, WILL, OPT_NAWS]);
        assert_eq!(&reply[6..15], &[IAC, SB, OPT_NAWS, 0, 80, 0, 24, IAC, SE]);
        assert_eq!(&reply[15..], &[IAC, WONT, 34]);
        assert!(p.naws_on);
        // Wiederholtes WILL ECHO wird nicht erneut beantwortet (keine Schleifen)
        let (_, reply) = p.feed(&[IAC, WILL, OPT_ECHO], 80, 24);
        assert!(reply.is_empty());
        // Terminaltyp, über zwei Pakete verteilt
        let (_, r1) = p.feed(&[IAC, DO, OPT_TTYPE, IAC, SB, OPT_TTYPE], 80, 24);
        assert_eq!(r1, vec![IAC, WILL, OPT_TTYPE]);
        let (_, r2) = p.feed(&[1, IAC, SE], 80, 24);
        assert!(r2.starts_with(&[IAC, SB, OPT_TTYPE, 0]) && r2.ends_with(&[IAC, SE]));
    }

    #[test]
    fn input_encoding() {
        assert_eq!(encode_input(b"ls\r"), b"ls\r\n".to_vec());
        assert_eq!(encode_input(&[1, IAC]), vec![1, IAC, IAC]);
    }
}
