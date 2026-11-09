//! SSH-Verbindungen (russh) inkl. Host-Key-Prüfung und SFTP.

use crate::{Cmd, Connection, Event, Sink};
use russh::client::{self, Handle, KeyboardInteractiveAuthResponse, Msg};
use russh::keys::ssh_key::HashAlg;
use russh::keys::PrivateKeyWithHashAlg;
use russh::{cipher, kex, mac, ChannelMsg, ChannelReadHalf, ChannelWriteHalf, Preferred};
use russh_sftp::client::SftpSession;
use serde::Serialize;
use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::mpsc;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// Großzügig, damit eine Host-Key-Rückfrage an den Benutzer nicht abbricht.
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(180);
/// Maximale Größe eines gebündelten Ausgabe-Pakets an die Oberfläche.
const MAX_BATCH: usize = 256 * 1024;
const COPY_BUF: usize = 256 * 1024;

pub const HOSTKEY_CHANGED: &str = "HOSTKEY_CHANGED:";
const REJECTED: &str = "\u{0}REJECTED";

/// Rückfrage bei einem noch unbekannten Server: (host:port, Fingerprint) -> vertrauen?
pub type Verifier = Arc<dyn Fn(String, String) -> bool + Send + Sync>;

#[derive(Clone)]
pub struct Options {
    /// Keepalive-Intervall in Sekunden (0 = aus)
    pub keepalive_secs: u64,
    /// Ältere Algorithmen (SHA1, CBC, 3DES) als Rückfall für alte Geräte erlauben
    pub legacy_algorithms: bool,
    /// Wenn gesetzt, wird bei neuen Hosts nachgefragt statt automatisch vertraut.
    pub verify_new_host: Option<Verifier>,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            keepalive_secs: 30,
            legacy_algorithms: true,
            verify_new_host: None,
        }
    }
}

pub struct SshSession {
    handle: Handle<Client>,
    tx: mpsc::UnboundedSender<Cmd>,
    sftp: tokio::sync::Mutex<Option<Arc<SftpSession>>>,
}

impl Connection for SshSession {
    fn send(&self, cmd: Cmd) {
        let _ = self.tx.send(cmd);
    }
}

impl SshSession {
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
// Host-Key-Verwaltung (known_hosts.json)
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

/// Alle bekannten Hosts (host:port, Fingerprint), sortiert.
pub fn known_hosts() -> Vec<(String, String)> {
    let _g = KNOWN_HOSTS_LOCK.lock().unwrap();
    let mut v: Vec<_> = load_known_hosts().into_iter().collect();
    v.sort();
    v
}

pub struct Client {
    host_id: String,
    mismatch: Arc<Mutex<Option<String>>>,
    verify: Option<Verifier>,
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
        let known = {
            let _g = KNOWN_HOSTS_LOCK.lock().unwrap();
            load_known_hosts().get(&self.host_id).cloned()
        };
        match known {
            Some(known) if known == fp => Ok(true),
            Some(known) => {
                *self.mismatch.lock().unwrap() = Some(format!("{known} -> {fp}"));
                Ok(false)
            }
            None => {
                let trusted = match self.verify.clone() {
                    Some(v) => {
                        let (h, f) = (self.host_id.clone(), fp.clone());
                        tokio::task::spawn_blocking(move || v(h, f))
                            .await
                            .unwrap_or(false)
                    }
                    None => true,
                };
                if trusted {
                    let _g = KNOWN_HOSTS_LOCK.lock().unwrap();
                    let mut map = load_known_hosts();
                    map.insert(self.host_id.clone(), fp);
                    save_known_hosts(&map);
                } else {
                    *self.mismatch.lock().unwrap() = Some(REJECTED.into());
                }
                Ok(trusted)
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

fn config(opts: &Options) -> client::Config {
    let mut p = Preferred::default();
    if opts.legacy_algorithms {
        // Moderne Algorithmen zuerst, ältere (für Switches / Router) am Ende als Rückfall.
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
    }
    client::Config {
        preferred: p,
        keepalive_interval: (opts.keepalive_secs > 0).then(|| Duration::from_secs(opts.keepalive_secs)),
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
        let encrypted = crate::keys::is_encrypted(key_pem);
        if encrypted && pass.is_empty() {
            return Err("Der SSH-Key ist mit einer Passphrase geschützt.".into());
        }
        let passphrase = (!pass.is_empty()).then_some(pass);
        let key = russh::keys::decode_secret_key(key_pem, passphrase).map_err(|e| {
            if encrypted {
                "Passphrase für den SSH-Key ist falsch.".to_string()
            } else {
                format!("Ungültiger SSH-Key: {e}")
            }
        })?;
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
        return Err("SSH-Key wurde vom Server abgelehnt (Public Key hinterlegt?)".into());
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

#[allow(clippy::too_many_arguments)]
async fn establish(
    ip: String,
    user: String,
    pass: String,
    key: String,
    cols: u32,
    rows: u32,
    opts: Options,
) -> Result<Established, String> {
    let (host, port) = split_host_port(&ip);
    let host_id = format!("{host}:{port}");

    let tcp = tokio::time::timeout(
        CONNECT_TIMEOUT,
        tokio::net::TcpStream::connect((host.as_str(), port)),
    )
    .await
    .map_err(|_| format!("Fehler beim Verbinden: Zeitüberschreitung ({host_id})"))?
    .map_err(|e| format!("Fehler beim Verbinden: {e}"))?;
    let _ = tcp.set_nodelay(true);

    let mismatch = Arc::new(Mutex::new(None));
    let handler = Client {
        host_id: host_id.clone(),
        mismatch: mismatch.clone(),
        verify: opts.verify_new_host.clone(),
    };

    let fut = async {
        let mut handle = client::connect_stream(Arc::new(config(&opts)), tcp, handler)
            .await
            .map_err(|e| match mismatch.lock().unwrap().take() {
                Some(m) if m == REJECTED => {
                    "Host-Key wurde nicht bestätigt – Verbindung abgebrochen.".to_string()
                }
                Some(m) => format!("{HOSTKEY_CHANGED}{host_id}|{m}"),
                None => format!("Handshake Fehler: {e}"),
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

/// Baut die Verbindung auf. Die Ausgabe landet im `sink`.
/// Abbrechen: den Future (bzw. Task) einfach abbrechen.
#[allow(clippy::too_many_arguments)]
pub async fn connect(
    ip: String,
    user: String,
    pass: String,
    key: String,
    cols: u32,
    rows: u32,
    opts: Options,
    sink: Sink,
) -> Result<Arc<SshSession>, String> {
    let est = establish(ip, user, pass, key, cols, rows, opts).await?;
    let (tx, rx) = mpsc::unbounded_channel();
    let sess = Arc::new(SshSession {
        handle: est.handle,
        tx,
        sftp: tokio::sync::Mutex::new(None),
    });
    tokio::spawn(writer_task(sess.clone(), est.write, rx));
    tokio::spawn(reader_task(sess.clone(), est.read, sink));
    Ok(sess)
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
                // Schnell hintereinander kommende Eingaben (z.B. Einfügen) bündeln.
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

async fn reader_task(sess: Arc<SshSession>, mut read: ChannelReadHalf, sink: Sink) {
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
        // Alles, was bereits anliegt, gebündelt weitergeben (weniger Arbeit bei viel Output).
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
            sink(Event::Data(std::mem::replace(
                &mut buf,
                Vec::with_capacity(64 * 1024),
            )));
        }
    }
    sess.close();
    sink(Event::Closed("[Verbindung getrennt]".into()));
}

// ---------------------------------------------------------------------------
// SFTP
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize)]
pub struct FileInfo {
    pub name: String,
    pub is_dir: bool,
    pub size: u64,
    /// Änderungszeit (Unix-Sekunden)
    pub modified: Option<u32>,
    /// z.B. "drwxr-xr-x"
    pub perms: String,
    pub is_link: bool,
}

fn perm_string(mode: Option<u32>, is_dir: bool, is_link: bool) -> String {
    let Some(m) = mode else { return String::new() };
    let mut s = String::with_capacity(10);
    s.push(if is_link {
        'l'
    } else if is_dir {
        'd'
    } else {
        '-'
    });
    for shift in [6, 3, 0] {
        let b = (m >> shift) & 7;
        s.push(if b & 4 != 0 { 'r' } else { '-' });
        s.push(if b & 2 != 0 { 'w' } else { '-' });
        s.push(if b & 1 != 0 { 'x' } else { '-' });
    }
    s
}

pub fn join(dir: &str, name: &str) -> String {
    if dir.ends_with('/') {
        format!("{dir}{name}")
    } else {
        format!("{dir}/{name}")
    }
}

pub async fn list_directory(sess: &SshSession, path: &str) -> Result<Vec<FileInfo>, String> {
    let sftp = sess.sftp().await?;
    let entries = sftp.read_dir(path).await.map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    if path != "/" {
        out.push(FileInfo {
            name: "..".into(),
            is_dir: true,
            size: 0,
            modified: None,
            perms: String::new(),
            is_link: false,
        });
    }
    for e in entries {
        let name = e.file_name();
        if name == "." || name == ".." {
            continue;
        }
        let meta = e.metadata();
        let mut is_dir = meta.is_dir();
        let is_link = meta.is_symlink();
        if is_link {
            // Ordner-Links sollen sich wie Ordner verhalten
            if let Ok(m) = sftp.metadata(join(path, &name)).await {
                is_dir = m.is_dir();
            }
        }
        out.push(FileInfo {
            perms: perm_string(meta.permissions, is_dir, is_link),
            name,
            is_dir,
            size: meta.size.unwrap_or(0),
            modified: meta.mtime,
            is_link,
        });
    }
    out.sort_by(|a, b| {
        (a.name != "..")
            .cmp(&(b.name != ".."))
            .then(b.is_dir.cmp(&a.is_dir))
            .then(a.name.to_lowercase().cmp(&b.name.to_lowercase()))
    });
    Ok(out)
}

/// Home-Verzeichnis des angemeldeten Benutzers.
pub async fn home(sess: &SshSession) -> Result<String, String> {
    let sftp = sess.sftp().await?;
    sftp.canonicalize(".").await.map_err(|e| e.to_string())
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

/// Löscht Dateien oder ganze Ordner (rekursiv). Symlinks werden nicht verfolgt.
pub async fn delete_recursive(sess: &SshSession, path: &str) -> Result<(), String> {
    let sftp = sess.sftp().await?;
    let meta = sftp
        .symlink_metadata(path)
        .await
        .map_err(|e| e.to_string())?;
    if !meta.is_dir() || meta.is_symlink() {
        return sftp.remove_file(path).await.map_err(|e| e.to_string());
    }
    let mut stack = vec![(path.to_string(), false)];
    while let Some((dir, visited)) = stack.pop() {
        if visited {
            sftp.remove_dir(&dir)
                .await
                .map_err(|e| format!("{dir}: {e}"))?;
            continue;
        }
        stack.push((dir.clone(), true));
        for e in sftp
            .read_dir(&dir)
            .await
            .map_err(|e| format!("{dir}: {e}"))?
        {
            let name = e.file_name();
            if name == "." || name == ".." {
                continue;
            }
            let p = join(&dir, &name);
            let m = e.metadata();
            if m.is_dir() && !m.is_symlink() {
                stack.push((p, false));
            } else {
                sftp.remove_file(&p)
                    .await
                    .map_err(|e| format!("{p}: {e}"))?;
            }
        }
    }
    Ok(())
}

pub async fn rename(sess: &SshSession, old: &str, new: &str) -> Result<(), String> {
    let sftp = sess.sftp().await?;
    sftp.rename(old, new).await.map_err(|e| e.to_string())
}

pub async fn mkdir(sess: &SshSession, path: &str) -> Result<(), String> {
    let sftp = sess.sftp().await?;
    sftp.create_dir(path).await.map_err(|e| e.to_string())
}

/// Ändert die Rechte (z.B. 0o644).
pub async fn chmod(sess: &SshSession, path: &str, mode: u32) -> Result<(), String> {
    let sftp = sess.sftp().await?;
    let mut attrs = russh_sftp::protocol::FileAttributes::empty();
    attrs.permissions = Some(mode);
    sftp.set_metadata(path, attrs)
        .await
        .map_err(|e| e.to_string())
}

/// Hinterlegt einen Public Key in ~/.ssh/authorized_keys (mit korrekten Rechten).
/// Gibt `false` zurück, wenn der Key schon vorhanden war.
pub async fn install_public_key(sess: &SshSession, public_line: &str) -> Result<bool, String> {
    let sftp = sess.sftp().await?;
    let home = sftp.canonicalize(".").await.map_err(|e| e.to_string())?;
    let dir = join(&home, ".ssh");
    if !sftp.try_exists(dir.clone()).await.unwrap_or(false) {
        sftp.create_dir(dir.clone())
            .await
            .map_err(|e| format!(".ssh anlegen: {e}"))?;
    }
    let mut attrs = russh_sftp::protocol::FileAttributes::empty();
    attrs.permissions = Some(0o700);
    let _ = sftp.set_metadata(dir.clone(), attrs).await;

    let file = join(&dir, "authorized_keys");
    let existing = match sftp.read(file.clone()).await {
        Ok(b) => String::from_utf8_lossy(&b).to_string(),
        Err(_) => String::new(),
    };
    let key_part: String = public_line
        .split_whitespace()
        .take(2)
        .collect::<Vec<_>>()
        .join(" ");
    if existing.lines().any(|l| l.contains(&key_part)) {
        return Ok(false);
    }
    let mut content = existing;
    if !content.is_empty() && !content.ends_with('\n') {
        content.push('\n');
    }
    content.push_str(public_line.trim());
    content.push('\n');
    sftp.write(file.clone(), content.as_bytes())
        .await
        .map_err(|e| e.to_string())?;
    let mut attrs = russh_sftp::protocol::FileAttributes::empty();
    attrs.permissions = Some(0o600);
    let _ = sftp.set_metadata(file, attrs).await;
    Ok(true)
}

/// Fortschritt einer Übertragung in Bytes.
pub type Progress = Option<Arc<AtomicU64>>;

fn bump(p: &Progress, n: usize) {
    if let Some(p) = p {
        p.fetch_add(n as u64, Ordering::Relaxed);
    }
}

pub async fn upload(sess: &SshSession, remote_dir: &str, local: &str) -> Result<(), String> {
    upload_with(sess, remote_dir, local, None).await
}

pub async fn download(sess: &SshSession, remote: &str, local: &str) -> Result<(), String> {
    download_with(sess, remote, local, None).await
}

/// Lädt eine Datei oder einen ganzen Ordner (rekursiv) hoch.
pub async fn upload_with(
    sess: &SshSession,
    remote_dir: &str,
    local: &str,
    progress: Progress,
) -> Result<(), String> {
    let lp = std::path::Path::new(local);
    if lp.is_dir() {
        let sftp = sess.sftp().await?;
        let name = lp
            .file_name()
            .and_then(|n| n.to_str())
            .ok_or("Ungültiger Ordnername")?;
        let target = join(remote_dir, name);
        if !sftp.try_exists(target.clone()).await.unwrap_or(false) {
            sftp.create_dir(target.clone())
                .await
                .map_err(|e| format!("{target}: {e}"))?;
        }
        let entries = std::fs::read_dir(lp).map_err(|e| e.to_string())?;
        for e in entries.flatten() {
            let p = e.path();
            Box::pin(upload_with(sess, &target, &p.to_string_lossy(), progress.clone())).await?;
        }
        return Ok(());
    }
    upload_file(sess, remote_dir, local, progress).await
}

async fn upload_file(
    sess: &SshSession,
    remote_dir: &str,
    local: &str,
    progress: Progress,
) -> Result<(), String> {
    let sftp = sess.sftp().await?;
    let name = std::path::Path::new(local)
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or("Ungültiger Dateiname")?;
    let remote = join(remote_dir, name);
    let mut src = tokio::fs::File::open(local)
        .await
        .map_err(|e| e.to_string())?;
    let mut dst = sftp.create(remote).await.map_err(|e| e.to_string())?;
    let mut buf = vec![0u8; COPY_BUF];
    loop {
        let n = src.read(&mut buf).await.map_err(|e| e.to_string())?;
        if n == 0 {
            break;
        }
        dst.write_all(&buf[..n]).await.map_err(|e| e.to_string())?;
        bump(&progress, n);
    }
    dst.shutdown().await.map_err(|e| e.to_string())
}

pub async fn download_with(
    sess: &SshSession,
    remote: &str,
    local: &str,
    progress: Progress,
) -> Result<(), String> {
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
        bump(&progress, n);
    }
    dst.flush().await.map_err(|e| e.to_string())
}

/// Gesamtgröße einer lokalen Datei bzw. eines Ordners (für die Fortschrittsanzeige).
pub fn local_size(path: &str) -> u64 {
    let p = std::path::Path::new(path);
    if p.is_dir() {
        std::fs::read_dir(p)
            .map(|rd| {
                rd.flatten()
                    .map(|e| local_size(&e.path().to_string_lossy()))
                    .sum()
            })
            .unwrap_or(0)
    } else {
        std::fs::metadata(p).map(|m| m.len()).unwrap_or(0)
    }
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
    //! NEBULA_TEST_SSH="127.0.0.1:2222|user|pass" cargo test -p nebula-core integration -- --ignored
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
        let (dtx, mut read) = mpsc::unbounded_channel::<Vec<u8>>();
        let sink: Sink = Arc::new(move |e| {
            if let Event::Data(d) = e {
                let _ = dtx.send(d);
            }
        });
        let sess = connect(ip, user, pass, String::new(), 100, 30, Options::default(), sink)
            .await
            .unwrap();

        sess.send(Cmd::Resize(120, 40));
        sess.send(Cmd::Data(b"stty size; echo NEBULA_$((6*7))\n".to_vec()));
        let mut out = Vec::new();
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        while !String::from_utf8_lossy(&out).contains("NEBULA_42") {
            let m = tokio::time::timeout_at(deadline, read.recv()).await.expect("Timeout");
            if let Some(data) = m {
                out.extend_from_slice(&data);
            }
        }
        let text = String::from_utf8_lossy(&out);
        assert!(text.contains("40 120"), "PTY-Größe falsch: {text}");

        // SFTP
        let home = home(&sess).await.unwrap();
        assert!(home.starts_with('/'));
        write_file(&sess, "/tmp/nebula.txt", "hallo äöü").await.unwrap();
        assert_eq!(read_file(&sess, "/tmp/nebula.txt").await.unwrap(), "hallo äöü");
        let list = list_directory(&sess, "/tmp").await.unwrap();
        let f = list.iter().find(|f| f.name == "nebula.txt").expect("Datei fehlt");
        assert!(f.perms.starts_with('-') && f.modified.is_some());
        chmod(&sess, "/tmp/nebula.txt", 0o600).await.unwrap();
        mkdir(&sess, "/tmp/nebula_dir").await.unwrap();
        mkdir(&sess, "/tmp/nebula_dir/sub").await.unwrap();
        rename(&sess, "/tmp/nebula.txt", "/tmp/nebula_dir/sub/x.txt").await.unwrap();
        let local = std::env::temp_dir().join("nebula_dl.txt");
        let prog = Arc::new(AtomicU64::new(0));
        download_with(&sess, "/tmp/nebula_dir/sub/x.txt", local.to_str().unwrap(), Some(prog.clone()))
            .await
            .unwrap();
        assert_eq!(prog.load(Ordering::Relaxed), "hallo äöü".len() as u64);
        assert_eq!(std::fs::read_to_string(&local).unwrap(), "hallo äöü");

        // Ordner-Upload rekursiv
        let ldir = std::env::temp_dir().join("nebula_up");
        let _ = std::fs::remove_dir_all(&ldir);
        std::fs::create_dir_all(ldir.join("a")).unwrap();
        std::fs::write(ldir.join("a/b.txt"), "B").unwrap();
        std::fs::write(ldir.join("c.txt"), "C").unwrap();
        upload_with(&sess, "/tmp/nebula_dir", ldir.to_str().unwrap(), None).await.unwrap();
        assert_eq!(read_file(&sess, "/tmp/nebula_dir/nebula_up/a/b.txt").await.unwrap(), "B");

        // rekursiv löschen
        delete_recursive(&sess, "/tmp/nebula_dir").await.unwrap();
        assert!(list_directory(&sess, "/tmp").await.unwrap().iter().all(|f| f.name != "nebula_dir"));

        // Public Key hinterlegen (zweimal: zweites Mal "schon vorhanden")
        let k = crate::keys::generate(crate::keys::KeyType::Ed25519, "nebula-test", "").unwrap();
        assert!(install_public_key(&sess, &k.public_line).await.unwrap());
        assert!(!install_public_key(&sess, &k.public_line).await.unwrap());

        // Durchsatz: 20 MB Ausgabe
        let start = std::time::Instant::now();
        sess.send(Cmd::Data(
            b"head -c 20000000 /dev/zero | tr '\\0' 'x'; echo; echo DONE_$((1+1))\n".to_vec(),
        ));
        let mut total = 0usize;
        let mut tail = String::new();
        while !tail.contains("DONE_2") {
            match tokio::time::timeout(Duration::from_secs(30), read.recv())
                .await
                .expect("Timeout")
            {
                Some(data) => {
                    total += data.len();
                    tail = String::from_utf8_lossy(&data[data.len().saturating_sub(64)..]).into();
                }
                None => panic!("Kanal zu"),
            }
        }
        let secs = start.elapsed().as_secs_f64();
        println!(
            "Durchsatz: {:.1} MB in {:.2}s = {:.1} MB/s",
            total as f64 / 1e6,
            secs,
            total as f64 / 1e6 / secs
        );
        sess.close();

        // Mit dem eben hinterlegten Key anmelden
        let (ip, user, _) = target();
        establish(ip, user, String::new(), k.private_openssh, 80, 24, Options::default())
            .await
            .expect("Login mit hinterlegtem Key");
    }

    #[tokio::test]
    #[ignore]
    async fn wrong_password_is_rejected() {
        let (ip, user, _) = target();
        let err = establish(ip, user, "falsch".into(), String::new(), 80, 24, Options::default())
            .await
            .err()
            .unwrap();
        assert!(err.contains("falsch"), "{err}");
    }

    #[tokio::test]
    #[ignore]
    async fn new_host_can_be_rejected() {
        let (ip, user, pass) = target();
        let (h, p) = split_host_port(&ip);
        forget_host_key(&format!("{h}:{p}"));
        let opts = Options {
            verify_new_host: Some(Arc::new(|_, _| false)),
            ..Default::default()
        };
        let err = establish(ip.clone(), user.clone(), pass.clone(), String::new(), 80, 24, opts)
            .await
            .err()
            .unwrap();
        assert!(err.contains("nicht bestätigt"), "{err}");
        // Danach bestätigen -> klappt und wird gespeichert
        let opts = Options {
            verify_new_host: Some(Arc::new(|_, _| true)),
            ..Default::default()
        };
        establish(ip, user, pass, String::new(), 80, 24, opts).await.unwrap();
        assert!(known_hosts().iter().any(|(k, _)| *k == format!("{h}:{p}")));
    }

    #[tokio::test]
    #[ignore]
    async fn login_with_keys() {
        let (ip, user, _) = target();
        for f in std::env::var("NEBULA_TEST_KEYS").unwrap().split(';') {
            let pem = std::fs::read_to_string(f).unwrap();
            establish(ip.clone(), user.clone(), String::new(), pem, 80, 24, Options::default())
                .await
                .unwrap_or_else(|e| panic!("{f}: {e}"));
            println!("Key-Login OK: {f}");
        }
    }
}
