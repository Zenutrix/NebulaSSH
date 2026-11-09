//! SSH-Verbindungen (russh) inkl. Host-Key-Prüfung (Trust-on-first-use) und SFTP.

use crate::session::{Cmd, Entry, Sessions};
use russh::client::{self, Handle, KeyboardInteractiveAuthResponse, Msg};
use russh::keys::ssh_key::HashAlg;
use russh::keys::PrivateKeyWithHashAlg;
use russh::{cipher, kex, mac, ChannelMsg, ChannelReadHalf, ChannelWriteHalf, Preferred};
use russh_sftp::client::SftpSession;
use serde::Serialize;
use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tauri::ipc::{Channel, InvokeResponseBody};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::mpsc;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(20);
/// Maximale Größe eines gebündelten Ausgabe-Pakets an die Oberfläche.
const MAX_BATCH: usize = 256 * 1024;

pub const HOSTKEY_CHANGED: &str = "HOSTKEY_CHANGED:";

pub struct SshSession {
    handle: Handle<Client>,
    pub tx: mpsc::UnboundedSender<Cmd>,
    sftp: tokio::sync::Mutex<Option<Arc<SftpSession>>>,
}

impl SshSession {
    pub fn close(&self) {
        // Writer-Task endet, sobald alle Sender weg sind; Verbindung sauber trennen.
        let _ = self.tx.send(Cmd::Close);
    }

    pub async fn sftp(&self) -> Result<Arc<SftpSession>, String> {
        let mut guard = self.sftp.lock().await;
        if let Some(s) = guard.as_ref() {
            return Ok(s.clone());
        }
        let ch = self
            .handle
            .channel_open_session()
            .await
            .map_err(|e| format!("SFTP-Kanal: {e}"))?;
        ch.request_subsystem(true, "sftp")
            .await
            .map_err(|e| format!("SFTP-Subsystem: {e}"))?;
        let s = SftpSession::new(ch.into_stream())
            .await
            .map_err(|e| format!("SFTP nicht verfügbar: {e}"))?;
        let s = Arc::new(s);
        *guard = Some(s.clone());
        Ok(s)
    }
}

// ---------------------------------------------------------------------------
// Host-Key-Verwaltung (known_hosts.json neben hosts.json)
// ---------------------------------------------------------------------------

fn known_hosts_path() -> std::path::PathBuf {
    crate::storage::data_dir().join("known_hosts.json")
}

fn load_known_hosts() -> HashMap<String, String> {
    std::fs::read_to_string(known_hosts_path())
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default()
}

fn save_known_hosts(map: &HashMap<String, String>) {
    if let Ok(s) = serde_json::to_string_pretty(map) {
        let _ = std::fs::write(known_hosts_path(), s);
    }
}

static KNOWN_HOSTS_LOCK: Mutex<()> = Mutex::new(());

pub fn forget_host_key(host_id: &str) {
    let _g = KNOWN_HOSTS_LOCK.lock().unwrap();
    let mut map = load_known_hosts();
    map.remove(host_id);
    save_known_hosts(&map);
}

pub struct Client {
    host_id: String,
    mismatch: Arc<Mutex<Option<String>>>,
}

impl client::Handler for Client {
    type Error = russh::Error;

    async fn check_server_key(
        &mut self,
        key: &russh::keys::PublicKeyOrCertificate,
    ) -> Result<bool, Self::Error> {
        let fp = match key {
            russh::keys::PublicKeyOrCertificate::PublicKey { key, .. } => {
                key.fingerprint(HashAlg::Sha256).to_string()
            }
            russh::keys::PublicKeyOrCertificate::Certificate(c) => {
                c.public_key().fingerprint(HashAlg::Sha256).to_string()
            }
        };
        let _g = KNOWN_HOSTS_LOCK.lock().unwrap();
        let mut map = load_known_hosts();
        match map.get(&self.host_id) {
            Some(known) if *known == fp => Ok(true),
            Some(known) => {
                *self.mismatch.lock().unwrap() = Some(format!("{known} -> {fp}"));
                Ok(false)
            }
            None => {
                map.insert(self.host_id.clone(), fp);
                save_known_hosts(&map);
                Ok(true)
            }
        }
    }
}

/// Trennt "host", "host:port", "[v6]:port" und reine IPv6-Adressen.
pub fn split_host_port(input: &str) -> (String, u16) {
    let s = input.trim();
    if let Some(rest) = s.strip_prefix('[') {
        if let Some((h, p)) = rest.split_once(']') {
            let port = p.trim_start_matches(':').parse().unwrap_or(22);
            return (h.to_string(), port);
        }
    }
    if s.matches(':').count() == 1 {
        let (h, p) = s.split_once(':').unwrap();
        if let Ok(port) = p.parse() {
            return (h.to_string(), port);
        }
    }
    (s.to_string(), 22)
}

fn config() -> client::Config {
    // Moderne Algorithmen zuerst, ältere (für Switches / Router) am Ende als Fallback.
    let mut p = Preferred::default();
    let mut kexes = p.kex.to_vec();
    kexes.extend([kex::DH_G14_SHA1, kex::DH_GEX_SHA1, kex::DH_G1_SHA1]);
    p.kex = Cow::Owned(kexes);
    let mut ciphers = p.cipher.to_vec();
    ciphers.extend([
        cipher::AES_128_GCM,
        cipher::AES_256_CBC,
        cipher::AES_192_CBC,
        cipher::AES_128_CBC,
        cipher::TRIPLE_DES_CBC,
    ]);
    p.cipher = Cow::Owned(ciphers);
    let mut macs = p.mac.to_vec();
    macs.extend([mac::HMAC_SHA1_ETM, mac::HMAC_SHA1]);
    p.mac = Cow::Owned(macs);

    client::Config {
        preferred: p,
        keepalive_interval: Some(Duration::from_secs(30)),
        keepalive_max: 4,
        // Kein Nagle-Delay: Tastendrücke gehen sofort raus.
        nodelay: true,
        window_size: 4 * 1024 * 1024,
        ..Default::default()
    }
}

async fn authenticate(
    handle: &mut Handle<Client>,
    user: &str,
    pass: &str,
    key_pem: &str,
) -> Result<(), String> {
    if !key_pem.trim().is_empty() {
        let passphrase = (!pass.is_empty()).then_some(pass);
        let key = russh::keys::decode_secret_key(key_pem, passphrase)
            .map_err(|e| format!("Ungültiger SSH-Key: {e}"))?;
        let hash = handle
            .best_supported_rsa_hash()
            .await
            .ok()
            .flatten()
            .flatten();
        let res = handle
            .authenticate_publickey(user, PrivateKeyWithHashAlg::new(Arc::new(key), hash))
            .await
            .map_err(|e| format!("Auth-Fehler: {e}"))?;
        if res.success() {
            return Ok(());
        }
        if pass.is_empty() {
            return Err("SSH-Key wurde vom Server abgelehnt".into());
        }
    }

    if !pass.is_empty() {
        let res = handle
            .authenticate_password(user, pass)
            .await
            .map_err(|e| format!("Auth-Fehler: {e}"))?;
        if res.success() {
            return Ok(());
        }
        // Viele Switches/Router erlauben nur "keyboard-interactive".
        let mut resp = handle
            .authenticate_keyboard_interactive_start(user, None)
            .await
            .map_err(|e| format!("Auth-Fehler: {e}"))?;
        for _ in 0..5 {
            match resp {
                KeyboardInteractiveAuthResponse::Success => return Ok(()),
                KeyboardInteractiveAuthResponse::Failure { .. } => break,
                KeyboardInteractiveAuthResponse::InfoRequest { prompts, .. } => {
                    let answers = prompts.iter().map(|_| pass.to_string()).collect();
                    resp = handle
                        .authenticate_keyboard_interactive_respond(answers)
                        .await
                        .map_err(|e| format!("Auth-Fehler: {e}"))?;
                }
            }
        }
        return Err("Benutzername oder Passwort falsch".into());
    }

    let res = handle
        .authenticate_none(user)
        .await
        .map_err(|e| format!("Auth-Fehler: {e}"))?;
    if res.success() {
        Ok(())
    } else {
        Err("Kein Passwort/Key angegeben".into())
    }
}

struct Established {
    handle: Handle<Client>,
    read: ChannelReadHalf,
    write: ChannelWriteHalf<Msg>,
}

async fn establish(
    ip: String,
    user: String,
    pass: String,
    key: String,
    cols: u32,
    rows: u32,
) -> Result<Established, String> {
    let (host, port) = split_host_port(&ip);
    let host_id = format!("{host}:{port}");

    let tcp = tokio::time::timeout(CONNECT_TIMEOUT, tokio::net::TcpStream::connect((host.as_str(), port)))
        .await
        .map_err(|_| format!("Fehler beim Verbinden: Zeitüberschreitung ({host_id})"))?
        .map_err(|e| format!("Fehler beim Verbinden: {e}"))?;
    let _ = tcp.set_nodelay(true);

    let mismatch = Arc::new(Mutex::new(None));
    let handler = Client {
        host_id: host_id.clone(),
        mismatch: mismatch.clone(),
    };

    let fut = async {
        let mut handle = client::connect_stream(Arc::new(config()), tcp, handler)
            .await
            .map_err(|e| {
                if let Some(m) = mismatch.lock().unwrap().take() {
                    format!("{HOSTKEY_CHANGED}{host_id}|{m}")
                } else {
                    format!("Handshake Fehler: {e}")
                }
            })?;
        authenticate(&mut handle, &user, &pass, &key).await?;

        let channel = handle
            .channel_open_session()
            .await
            .map_err(|e| format!("Session Fehler: {e}"))?;
        let (read, write) = channel.split();
        let modes = [
            (russh::Pty::ECHO, 1),
            (russh::Pty::TTY_OP_ISPEED, 14400),
            (russh::Pty::TTY_OP_OSPEED, 14400),
        ];
        write
            .request_pty(false, "xterm-256color", cols.max(20), rows.max(5), 0, 0, &modes)
            .await
            .map_err(|e| format!("PTY Fehler: {e}"))?;
        write
            .request_shell(false)
            .await
            .map_err(|e| format!("Shell Fehler: {e}"))?;
        Ok::<_, String>(Established { handle, read, write })
    };
    tokio::time::timeout(HANDSHAKE_TIMEOUT, fut)
        .await
        .map_err(|_| "Zeitüberschreitung beim Anmelden".to_string())?
}

#[derive(Serialize)]
struct FileInfo {
    name: String,
    #[serde(rename = "isDir")]
    is_dir: bool,
    size: u64,
}

/// Baut die Verbindung auf und registriert die Sitzung. Abbrechbar über `disconnect`.
#[allow(clippy::too_many_arguments)]
pub async fn connect(
    sessions: Sessions,
    id: String,
    ip: String,
    user: String,
    pass: String,
    key: String,
    cols: u32,
    rows: u32,
    on_data: Channel<InvokeResponseBody>,
    on_close: Channel<String>,
) -> Result<String, String> {
    let task = tokio::spawn(establish(ip, user, pass, key, cols, rows));
    sessions
        .lock()
        .unwrap()
        .insert(id.clone(), Entry::Pending(task.abort_handle()));

    let est = match task.await {
        Ok(Ok(est)) => est,
        Ok(Err(e)) => {
            sessions.lock().unwrap().remove(&id);
            return Err(e);
        }
        Err(_) => return Err("Verbindung abgebrochen".into()),
    };

    let (tx, rx) = mpsc::unbounded_channel();
    let sess = Arc::new(SshSession {
        handle: est.handle,
        tx,
        sftp: tokio::sync::Mutex::new(None),
    });

    let registered = {
        let mut map = sessions.lock().unwrap();
        if matches!(map.get(&id), Some(Entry::Pending(_))) {
            map.insert(id.clone(), Entry::Ssh(sess.clone()));
            true
        } else {
            false
        }
    };
    if !registered {
        // Tab wurde während des Verbindens geschlossen.
        let _ = sess
            .handle
            .disconnect(russh::Disconnect::ByApplication, "", "de")
            .await;
        return Err("Verbindung abgebrochen".into());
    }

    tokio::spawn(writer_task(sess.clone(), est.write, rx));
    tokio::spawn(reader_task(sessions, id, sess, est.read, on_data, on_close));
    Ok("Verbunden!".into())
}

async fn writer_task(
    sess: Arc<SshSession>,
    write: ChannelWriteHalf<Msg>,
    mut rx: mpsc::UnboundedReceiver<Cmd>,
) {
    let mut next: Option<Cmd> = None;
    loop {
        let cmd = match next.take() {
            Some(c) => c,
            None => match rx.recv().await {
                Some(c) => c,
                None => break,
            },
        };
        match cmd {
            Cmd::Data(mut d) => {
                // Mehrere schnell hintereinander kommende Eingaben (z.B. Einfügen) bündeln.
                while d.len() < 32 * 1024 {
                    match rx.try_recv() {
                        Ok(Cmd::Data(more)) => d.extend_from_slice(&more),
                        Ok(other) => {
                            next = Some(other);
                            break;
                        }
                        Err(_) => break,
                    }
                }
                if write.data_bytes(d).await.is_err() {
                    break;
                }
            }
            Cmd::Resize(cols, rows) => {
                let _ = write.window_change(cols, rows, 0, 0).await;
            }
            Cmd::Close => break,
        }
    }
    let _ = write.close().await;
    let _ = sess
        .handle
        .disconnect(russh::Disconnect::ByApplication, "", "de")
        .await;
}

async fn reader_task(
    sessions: Sessions,
    id: String,
    sess: Arc<SshSession>,
    mut read: ChannelReadHalf,
    on_data: Channel<InvokeResponseBody>,
    on_close: Channel<String>,
) {
    let mut buf: Vec<u8> = Vec::with_capacity(64 * 1024);
    let mut closed = false;
    while !closed {
        match read.wait().await {
            Some(ChannelMsg::Data { data }) | Some(ChannelMsg::ExtendedData { data, .. }) => {
                buf.extend_from_slice(&data);
            }
            Some(ChannelMsg::Eof) | Some(ChannelMsg::Close) | None => closed = true,
            Some(_) => continue,
        }
        // Alles, was bereits anliegt, in einem Rutsch an die Oberfläche schicken:
        // deutlich weniger IPC-Nachrichten bei viel Output (cat, show running-config, ...).
        while !closed && buf.len() < MAX_BATCH {
            tokio::select! {
                biased;
                m = read.wait() => match m {
                    Some(ChannelMsg::Data { data }) | Some(ChannelMsg::ExtendedData { data, .. }) => {
                        buf.extend_from_slice(&data)
                    }
                    Some(ChannelMsg::Eof) | Some(ChannelMsg::Close) | None => closed = true,
                    Some(_) => {}
                },
                _ = std::future::ready(()) => break,
            }
        }
        if !buf.is_empty() {
            let chunk = std::mem::replace(&mut buf, Vec::with_capacity(64 * 1024));
            if on_data.send(InvokeResponseBody::Raw(chunk)).is_err() {
                break;
            }
        }
    }

    {
        let mut map = sessions.lock().unwrap();
        if matches!(map.get(&id), Some(Entry::Ssh(s)) if Arc::ptr_eq(s, &sess)) {
            map.remove(&id);
        }
    }
    sess.close();
    let _ = on_close.send("\r\n\x1b[90m[Verbindung getrennt]\x1b[0m\r\n".into());
}

// ---------------------------------------------------------------------------
// SFTP
// ---------------------------------------------------------------------------

pub async fn list_directory(sess: &SshSession, path: &str) -> Result<String, String> {
    let sftp = sess.sftp().await?;
    let entries = sftp.read_dir(path).await.map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    if path != "/" {
        out.push(FileInfo {
            name: "..".into(),
            is_dir: true,
            size: 0,
        });
    }
    for e in entries {
        let name = e.file_name();
        if name == "." || name == ".." {
            continue;
        }
        let meta = e.metadata();
        out.push(FileInfo {
            name,
            is_dir: meta.is_dir(),
            size: meta.size.unwrap_or(0),
        });
    }
    serde_json::to_string(&out).map_err(|e| e.to_string())
}

pub async fn read_file(sess: &SshSession, path: &str) -> Result<String, String> {
    let sftp = sess.sftp().await?;
    let data = sftp.read(path).await.map_err(|e| e.to_string())?;
    String::from_utf8(data).map_err(|_| "Binärdatei – bitte den Download nutzen".to_string())
}

pub async fn write_file(sess: &SshSession, path: &str, content: &str) -> Result<(), String> {
    let sftp = sess.sftp().await?;
    let mut f = sftp.create(path).await.map_err(|e| e.to_string())?;
    f.write_all(content.as_bytes())
        .await
        .map_err(|e| e.to_string())?;
    f.shutdown().await.map_err(|e| e.to_string())
}

pub async fn delete(sess: &SshSession, path: &str) -> Result<(), String> {
    let sftp = sess.sftp().await?;
    let meta = sftp.metadata(path).await.map_err(|e| e.to_string())?;
    if meta.is_dir() {
        sftp.remove_dir(path).await.map_err(|e| e.to_string())
    } else {
        sftp.remove_file(path).await.map_err(|e| e.to_string())
    }
}

pub async fn rename(sess: &SshSession, old: &str, new: &str) -> Result<(), String> {
    let sftp = sess.sftp().await?;
    sftp.rename(old, new).await.map_err(|e| e.to_string())
}

pub async fn mkdir(sess: &SshSession, path: &str) -> Result<(), String> {
    let sftp = sess.sftp().await?;
    sftp.create_dir(path).await.map_err(|e| e.to_string())
}

const COPY_BUF: usize = 256 * 1024;

pub async fn upload(sess: &SshSession, remote_dir: &str, local: &str) -> Result<(), String> {
    let sftp = sess.sftp().await?;
    let name = std::path::Path::new(local)
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or("Ungültiger Dateiname")?;
    let remote = format!("{}/{}", remote_dir.trim_end_matches('/'), name);
    let mut src = tokio::fs::File::open(local).await.map_err(|e| e.to_string())?;
    let mut dst = sftp.create(remote).await.map_err(|e| e.to_string())?;
    let mut buf = vec![0u8; COPY_BUF];
    loop {
        let n = src.read(&mut buf).await.map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        dst.write_all(&buf[..n]).await.map_err(|e| e.to_string())?;
    }
    dst.shutdown().await.map_err(|e| e.to_string())
}

pub async fn download(sess: &SshSession, remote: &str, local: &str) -> Result<(), String> {
    let sftp = sess.sftp().await?;
    let mut src = sftp.open(remote).await.map_err(|e| e.to_string())?;
    let mut dst = tokio::fs::File::create(local)
        .await
        .map_err(|e| e.to_string())?;
    let mut buf = vec![0u8; COPY_BUF];
    loop {
        let n = src.read(&mut buf).await.map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        dst.write_all(&buf[..n]).await.map_err(|e| e.to_string())?;
    }
    dst.flush().await.map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::split_host_port;

    #[test]
    fn host_port_parsing() {
        assert_eq!(split_host_port("10.0.0.1"), ("10.0.0.1".into(), 22));
        assert_eq!(split_host_port("10.0.0.1:2222"), ("10.0.0.1".into(), 2222));
        assert_eq!(split_host_port("example.com"), ("example.com".into(), 22));
        assert_eq!(split_host_port("[::1]:2200"), ("::1".into(), 2200));
        assert_eq!(split_host_port("fe80::1"), ("fe80::1".into(), 22));
        assert_eq!(split_host_port(" host:abc "), ("host:abc".into(), 22));
    }
}

#[cfg(test)]
mod integration {
    //! Läuft gegen einen lokalen Test-Server:
    //! NEBULA_TEST_SSH="127.0.0.1:2222|user|pass" cargo test integration -- --ignored
    use super::*;

    fn target() -> (String, String, String) {
        let v = std::env::var("NEBULA_TEST_SSH").expect("NEBULA_TEST_SSH fehlt");
        let p: Vec<&str> = v.split('|').collect();
        (p[0].into(), p[1].into(), p[2].into())
    }

    #[tokio::test]
    #[ignore]
    async fn shell_and_sftp() {
        let (ip, user, pass) = target();
        let est = establish(ip, user, pass, String::new(), 100, 30).await.unwrap();
        let (tx, rx) = mpsc::unbounded_channel();
        let sess = Arc::new(SshSession { handle: est.handle, tx, sftp: tokio::sync::Mutex::new(None) });
        tokio::spawn(writer_task(sess.clone(), est.write, rx));
        let mut read = est.read;

        sess.tx.send(Cmd::Resize(120, 40)).unwrap();
        sess.tx.send(Cmd::Data(b"stty size; echo NEBULA_$((6*7))\n".to_vec())).unwrap();
        let mut out = Vec::new();
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        while !String::from_utf8_lossy(&out).contains("NEBULA_42") {
            let m = tokio::time::timeout_at(deadline, read.wait()).await.expect("Timeout");
            if let Some(ChannelMsg::Data { data }) = m { out.extend_from_slice(&data); }
        }
        let text = String::from_utf8_lossy(&out);
        assert!(text.contains("40 120"), "PTY-Größe falsch: {text}");

        // SFTP
        write_file(&sess, "/tmp/nebula.txt", "hallo äöü").await.unwrap();
        assert_eq!(read_file(&sess, "/tmp/nebula.txt").await.unwrap(), "hallo äöü");
        let list = list_directory(&sess, "/tmp").await.unwrap();
        assert!(list.contains("nebula.txt"));
        mkdir(&sess, "/tmp/nebula_dir").await.unwrap();
        rename(&sess, "/tmp/nebula.txt", "/tmp/nebula_dir/x.txt").await.unwrap();
        let local = std::env::temp_dir().join("nebula_dl.txt");
        download(&sess, "/tmp/nebula_dir/x.txt", local.to_str().unwrap()).await.unwrap();
        assert_eq!(std::fs::read_to_string(&local).unwrap(), "hallo äöü");
        upload(&sess, "/tmp", local.to_str().unwrap()).await.unwrap();
        assert_eq!(read_file(&sess, "/tmp/nebula_dl.txt").await.unwrap(), "hallo äöü");
        delete(&sess, "/tmp/nebula_dir/x.txt").await.unwrap();
        delete(&sess, "/tmp/nebula_dir").await.unwrap();
        delete(&sess, "/tmp/nebula_dl.txt").await.unwrap();

        // Durchsatz: 20 MB Ausgabe
        let start = std::time::Instant::now();
        sess.tx.send(Cmd::Data(b"head -c 20000000 /dev/zero | tr '\0' 'x'; echo; echo DONE_$((1+1))\n".to_vec())).unwrap();
        let mut total = 0usize;
        let mut tail = String::new();
        while !tail.contains("DONE_2") {
            match tokio::time::timeout(Duration::from_secs(30), read.wait()).await.expect("Timeout") {
                Some(ChannelMsg::Data { data }) => {
                    total += data.len();
                    tail = String::from_utf8_lossy(&data[data.len().saturating_sub(64)..]).into();
                }
                Some(_) => {}
                None => panic!("Kanal zu"),
            }
        }
        let secs = start.elapsed().as_secs_f64();
        println!("Durchsatz: {:.1} MB in {:.2}s = {:.1} MB/s", total as f64 / 1e6, secs, total as f64 / 1e6 / secs);
        sess.close();
    }

    #[tokio::test]
    #[ignore]
    async fn wrong_password_is_rejected() {
        let (ip, user, _) = target();
        let err = establish(ip, user, "falsch".into(), String::new(), 80, 24).await.err().unwrap();
        assert!(err.contains("falsch"), "{err}");
    }
}

#[cfg(test)]
mod key_auth {
    use super::*;
    #[tokio::test]
    #[ignore]
    async fn login_with_keys() {
        let v = std::env::var("NEBULA_TEST_SSH").unwrap();
        let p: Vec<&str> = v.split('|').collect();
        for f in std::env::var("NEBULA_TEST_KEYS").unwrap().split(';') {
            let pem = std::fs::read_to_string(f).unwrap();
            establish(p[0].into(), p[1].into(), String::new(), pem, 80, 24)
                .await
                .unwrap_or_else(|e| panic!("{f}: {e}"));
            println!("Key-Login OK: {f}");
        }
    }
}
