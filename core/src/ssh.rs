//! SSH-Verbindungen (russh): Host-Key-Prüfung, Jump-Host, SSH-Agent, Port-Weiterleitung,
//! X11-Weiterleitung und SFTP.

use crate::{t, tf, Cmd, Connection, Event, Sink};
use russh::client::{self, ChannelOpenHandle, Handle, KeyboardInteractiveAuthResponse, Msg, Session};
use russh::keys::agent::client::AgentClient;
use russh::keys::ssh_key::HashAlg;
use russh::keys::PrivateKeyWithHashAlg;
use russh::{
    cipher, kex, mac, Channel, ChannelMsg, ChannelOpenFailure, ChannelReadHalf, ChannelWriteHalf,
    Preferred,
};
use russh_sftp::client::SftpSession;
use serde::{Deserialize, Serialize};
use std::borrow::Cow;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncSeekExt, AsyncWrite, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio::sync::mpsc;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
/// Großzügig, damit eine Host-Key-Rückfrage an den Benutzer nicht abbricht.
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(180);
/// Maximale Größe eines gebündelten Ausgabe-Pakets an die Oberfläche.
const MAX_BATCH: usize = 256 * 1024;
const COPY_BUF: usize = 256 * 1024;

pub const HOSTKEY_CHANGED: &str = "HOSTKEY_CHANGED:";
const REJECTED: &str = "\u{0}REJECTED";
/// Kennzeichnet Anmeldefehler (falsches Passwort / Passphrase), damit die Oberfläche
/// gespeicherte Zugangsdaten verwerfen kann.
pub const AUTH_FAILED: &str = "\u{1}";

/// Rückfrage bei einem noch unbekannten Server: (host:port, Fingerprint) -> vertrauen?
pub type Verifier = Arc<dyn Fn(String, String) -> bool + Send + Sync>;

/// Zugangsdaten für einen Server.
#[derive(Clone, Default)]
pub struct Auth {
    pub user: String,
    /// Passwort bzw. Passphrase des Keys
    pub pass: String,
    /// Private Key (PEM/OpenSSH), leer = kein Key
    pub key: String,
    /// Schlüssel aus dem SSH-Agent (Pageant / Windows OpenSSH-Agent) verwenden
    pub agent: bool,
}

/// Ein Server mit Adresse ("host", "host:port", "[v6]:port") und Zugangsdaten.
#[derive(Clone, Default)]
pub struct Target {
    pub addr: String,
    pub auth: Auth,
}

impl Target {
    pub fn password(addr: impl Into<String>, user: impl Into<String>, pass: impl Into<String>) -> Self {
        Self {
            addr: addr.into(),
            auth: Auth { user: user.into(), pass: pass.into(), ..Default::default() },
        }
    }
}

/// Port-Weiterleitung (wie `ssh -L`, `-R`, `-D`).
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct Forward {
    /// "local" (-L), "remote" (-R) oder "dynamic" (-D, SOCKS-Proxy)
    pub kind: String,
    /// local/dynamic: `[adresse:]port` auf diesem PC; remote: `[adresse:]port` auf dem Server
    pub bind: String,
    /// Ziel `host:port` (bei dynamic leer); bei remote vom PC aus gesehen
    pub target: String,
    pub enabled: bool,
}

impl Default for Forward {
    fn default() -> Self {
        Self { kind: "local".into(), bind: String::new(), target: String::new(), enabled: true }
    }
}

impl Forward {
    pub fn label(&self) -> String {
        match self.kind.as_str() {
            "remote" => tf!("Server {} → hier {}", "server {} → here {}", self.bind, self.target),
            "dynamic" => tf!("SOCKS-Proxy {}", "SOCKS proxy {}", self.bind),
            _ => tf!("hier {} → {}", "here {} → {}", self.bind, self.target),
        }
    }
    pub fn short(&self) -> String {
        match self.kind.as_str() {
            "remote" => format!("-R {}:{}", self.bind, self.target),
            "dynamic" => format!("-D {}", self.bind),
            _ => format!("-L {}:{}", self.bind, self.target),
        }
    }
}

#[derive(Clone)]
pub struct Options {
    /// Keepalive-Intervall in Sekunden (0 = aus)
    pub keepalive_secs: u64,
    /// Ältere Algorithmen (SHA1, CBC, 3DES) als Rückfall für alte Geräte erlauben
    pub legacy_algorithms: bool,
    /// Wenn gesetzt, wird bei neuen Hosts nachgefragt statt automatisch vertraut.
    pub verify_new_host: Option<Verifier>,
    /// Verbindung über diesen Zwischenserver aufbauen (ProxyJump)
    pub jump: Option<Target>,
    /// SSH-Agent an den Server weiterreichen
    pub agent_forward: bool,
    /// X11-Weiterleitung an einen lokalen X-Server (z.B. VcXsrv) auf :`x11_display`
    pub x11: bool,
    pub x11_display: u32,
    /// Beim Verbinden zu öffnende Tunnel
    pub forwards: Vec<Forward>,
}

impl Default for Options {
    fn default() -> Self {
        Self {
            keepalive_secs: 30,
            legacy_algorithms: true,
            verify_new_host: None,
            jump: None,
            agent_forward: false,
            x11: false,
            x11_display: 0,
            forwards: Vec::new(),
        }
    }
}

/// Zustand, den der russh-Handler und die Sitzung teilen.
#[derive(Default)]
struct Shared {
    /// Remote-Weiterleitungen: Port auf dem Server -> lokales Ziel host:port
    remote: Mutex<HashMap<u32, (String, Arc<AtomicU64>)>>,
    x11_display: Option<u32>,
    agent_forward: bool,
}

/// Ein laufender Tunnel.
struct Tunnel {
    fwd: Forward,
    error: Option<String>,
    conns: Arc<AtomicU64>,
    task: Option<tokio::task::AbortHandle>,
    remote: Option<(String, u32)>,
}

/// Anzeige-Infos zu einem Tunnel.
#[derive(Clone, Debug)]
pub struct TunnelInfo {
    pub fwd: Forward,
    pub error: Option<String>,
    /// Bisher weitergeleitete Verbindungen
    pub conns: u64,
}

pub struct SshSession {
    handle: Handle<Client>,
    jump: Option<Handle<Client>>,
    tx: mpsc::UnboundedSender<Cmd>,
    sftp: tokio::sync::Mutex<Option<Arc<SftpSession>>>,
    shared: Arc<Shared>,
    tunnels: Mutex<Vec<Tunnel>>,
    closed: AtomicBool,
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
            .map_err(|e| tf!("SFTP-Kanal: {e}", "SFTP channel: {e}"))?;
        ch.request_subsystem(true, "sftp")
            .await
            .map_err(|e| tf!("SFTP-Subsystem: {e}", "SFTP subsystem: {e}"))?;
        let s = SftpSession::new(ch.into_stream())
            .await
            .map_err(|e| tf!("SFTP nicht verfügbar: {e}", "SFTP not available: {e}"))?;
        let s = Arc::new(s);
        *guard = Some(s.clone());
        Ok(s)
    }

    pub fn is_closed(&self) -> bool {
        self.closed.load(Ordering::Relaxed)
    }

    /// Aktuelle Tunnel mit Status.
    pub fn tunnels(&self) -> Vec<TunnelInfo> {
        self.tunnels
            .lock()
            .unwrap()
            .iter()
            .map(|t| TunnelInfo { fwd: t.fwd.clone(), error: t.error.clone(), conns: t.conns.load(Ordering::Relaxed) })
            .collect()
    }

    /// Öffnet einen Tunnel. Ein Fehler (z.B. Port belegt) wird zurückgegeben und
    /// auch beim Tunnel vermerkt.
    pub async fn open_tunnel(self: &Arc<Self>, fwd: Forward) -> Result<(), String> {
        let conns = Arc::new(AtomicU64::new(0));
        let res = self.start_tunnel(&fwd, conns.clone()).await;
        let (task, remote, error) = match res {
            Ok((task, remote)) => (task, remote, None),
            Err(e) => (None, None, Some(e)),
        };
        let err = error.clone();
        self.tunnels.lock().unwrap().push(Tunnel { fwd, error, conns, task, remote });
        match err {
            Some(e) => Err(e),
            None => Ok(()),
        }
    }

    async fn start_tunnel(
        self: &Arc<Self>,
        fwd: &Forward,
        conns: Arc<AtomicU64>,
    ) -> Result<(Option<tokio::task::AbortHandle>, Option<(String, u32)>), String> {
        match fwd.kind.as_str() {
            "remote" => {
                let (addr, port) = parse_bind(&fwd.bind, "localhost")?;
                parse_target(&fwd.target)?;
                let got = self
                    .handle
                    .tcpip_forward(addr.clone(), port as u32)
                    .await
                    .map_err(|e| tf!("Server verweigert die Weiterleitung: {e}", "Server refused the forwarding: {e}"))?;
                let real = if port == 0 { got } else { port as u32 };
                self.shared.remote.lock().unwrap().insert(real, (fwd.target.clone(), conns));
                Ok((None, Some((addr, real))))
            }
            kind => {
                let (addr, port) = parse_bind(&fwd.bind, "127.0.0.1")?;
                let dynamic = kind == "dynamic";
                let target = if dynamic { None } else { Some(parse_target(&fwd.target)?) };
                let listener = tokio::net::TcpListener::bind((addr.as_str(), port))
                    .await
                    .map_err(|e| tf!("Port {addr}:{port} kann nicht geöffnet werden: {e}", "Cannot open port {addr}:{port}: {e}"))?;
                let weak = Arc::downgrade(self);
                let task = tokio::spawn(async move {
                    while let Ok((sock, peer)) = listener.accept().await {
                        let Some(sess) = weak.upgrade() else { break };
                        if sess.is_closed() {
                            break;
                        }
                        let _ = sock.set_nodelay(true);
                        conns.fetch_add(1, Ordering::Relaxed);
                        let target = target.clone();
                        tokio::spawn(async move {
                            let _ = forward_one(&sess, sock, peer, target).await;
                        });
                    }
                });
                Ok((Some(task.abort_handle()), None))
            }
        }
    }

    /// Schließt einen Tunnel.
    pub async fn close_tunnel(&self, index: usize) {
        let t = {
            let mut list = self.tunnels.lock().unwrap();
            if index >= list.len() {
                return;
            }
            list.remove(index)
        };
        if let Some(task) = t.task {
            task.abort();
        }
        if let Some((addr, port)) = t.remote {
            self.shared.remote.lock().unwrap().remove(&port);
            let _ = self.handle.cancel_tcpip_forward(addr, port).await;
        }
    }

    fn abort_tunnels(&self) {
        for t in self.tunnels.lock().unwrap().iter_mut() {
            if let Some(task) = t.task.take() {
                task.abort();
            }
        }
    }
}

/// Eine weitergeleitete Verbindung (lokal oder SOCKS) durch den SSH-Tunnel schicken.
async fn forward_one(
    sess: &SshSession,
    mut sock: TcpStream,
    peer: std::net::SocketAddr,
    target: Option<(String, u16)>,
) -> Result<(), String> {
    let (socks, host, port) = match target {
        Some((h, p)) => (None, h, p),
        None => {
            let (v, h, p) = socks_handshake(&mut sock).await?;
            (Some(v), h, p)
        }
    };
    let ch = sess
        .handle
        .channel_open_direct_tcpip(host, port as u32, peer.ip().to_string(), peer.port() as u32)
        .await;
    match (ch, socks) {
        (Ok(ch), socks) => {
            if let Some(v) = socks {
                let _ = sock.write_all(&socks_reply(v, true)).await;
            }
            pump(ch, sock).await;
            Ok(())
        }
        (Err(e), socks) => {
            if let Some(v) = socks {
                let _ = sock.write_all(&socks_reply(v, false)).await;
            }
            Err(e.to_string())
        }
    }
}

async fn pump<S: AsyncRead + AsyncWrite + Unpin>(ch: Channel<Msg>, mut other: S) {
    let mut cs = ch.into_stream();
    let _ = tokio::io::copy_bidirectional(&mut cs, &mut other).await;
    let _ = cs.shutdown().await;
}

/// SOCKS4/4a/5 (nur CONNECT, ohne Anmeldung). Liefert (Version, Host, Port).
async fn socks_handshake(s: &mut TcpStream) -> Result<(u8, String, u16), String> {
    let e = |e: std::io::Error| e.to_string();
    let ver = s.read_u8().await.map_err(e)?;
    match ver {
        5 => {
            let n = s.read_u8().await.map_err(e)? as usize;
            let mut methods = vec![0u8; n];
            s.read_exact(&mut methods).await.map_err(e)?;
            if !methods.contains(&0) {
                let _ = s.write_all(&[5, 0xff]).await;
                return Err("SOCKS: keine passende Anmeldemethode".into());
            }
            s.write_all(&[5, 0]).await.map_err(e)?;
            let mut head = [0u8; 4];
            s.read_exact(&mut head).await.map_err(e)?;
            if head[1] != 1 {
                let _ = s.write_all(&[5, 7, 0, 1, 0, 0, 0, 0, 0, 0]).await;
                return Err("SOCKS: nur CONNECT wird unterstützt".into());
            }
            let host = match head[3] {
                1 => {
                    let mut a = [0u8; 4];
                    s.read_exact(&mut a).await.map_err(e)?;
                    std::net::Ipv4Addr::from(a).to_string()
                }
                3 => {
                    let n = s.read_u8().await.map_err(e)? as usize;
                    let mut b = vec![0u8; n];
                    s.read_exact(&mut b).await.map_err(e)?;
                    String::from_utf8_lossy(&b).to_string()
                }
                4 => {
                    let mut a = [0u8; 16];
                    s.read_exact(&mut a).await.map_err(e)?;
                    std::net::Ipv6Addr::from(a).to_string()
                }
                _ => return Err("SOCKS: unbekannter Adresstyp".into()),
            };
            let port = s.read_u16().await.map_err(e)?;
            Ok((5, host, port))
        }
        4 => {
            let cmd = s.read_u8().await.map_err(e)?;
            let port = s.read_u16().await.map_err(e)?;
            let mut ip = [0u8; 4];
            s.read_exact(&mut ip).await.map_err(e)?;
            read_cstr(s).await?; // Benutzer-ID
            if cmd != 1 {
                let _ = s.write_all(&socks_reply(4, false)).await;
                return Err("SOCKS4: nur CONNECT".into());
            }
            let host = if ip[0] == 0 && ip[1] == 0 && ip[2] == 0 && ip[3] != 0 {
                read_cstr(s).await? // SOCKS4a: Hostname folgt
            } else {
                std::net::Ipv4Addr::from(ip).to_string()
            };
            Ok((4, host, port))
        }
        _ => Err("Kein SOCKS-Client".into()),
    }
}

async fn read_cstr(s: &mut TcpStream) -> Result<String, String> {
    let mut out = Vec::new();
    loop {
        let b = s.read_u8().await.map_err(|e| e.to_string())?;
        if b == 0 || out.len() > 255 {
            break;
        }
        out.push(b);
    }
    Ok(String::from_utf8_lossy(&out).to_string())
}

fn socks_reply(ver: u8, ok: bool) -> Vec<u8> {
    if ver == 4 {
        vec![0, if ok { 0x5a } else { 0x5b }, 0, 0, 0, 0, 0, 0]
    } else {
        vec![5, if ok { 0 } else { 5 }, 0, 1, 0, 0, 0, 0, 0, 0]
    }
}

/// "8080", "127.0.0.1:8080", "[::1]:8080", "*:8080"
pub fn parse_bind(bind: &str, default_addr: &str) -> Result<(String, u16), String> {
    let b = bind.trim();
    let err = || tf!("Ungültiger Port: „{b}“", "Invalid port: \"{b}\"");
    if let Ok(p) = b.parse::<u16>() {
        return Ok((default_addr.to_string(), p));
    }
    let (h, p) = if let Some(rest) = b.strip_prefix('[') {
        rest.split_once("]:").ok_or_else(err)?
    } else {
        b.rsplit_once(':').ok_or_else(err)?
    };
    let port = p.parse::<u16>().map_err(|_| err())?;
    let h = match h {
        "*" | "" => "0.0.0.0",
        h => h,
    };
    Ok((h.to_string(), port))
}

/// "host:port" (Port Pflicht)
pub fn parse_target(target: &str) -> Result<(String, u16), String> {
    let t = target.trim();
    let err = || tf!("Ziel muss host:port sein: „{t}“", "Target must be host:port: \"{t}\"");
    let (h, p) = if let Some(rest) = t.strip_prefix('[') {
        rest.split_once("]:").ok_or_else(err)?
    } else {
        t.rsplit_once(':').ok_or_else(err)?
    };
    let port = p.parse::<u16>().map_err(|_| err())?;
    if h.is_empty() || port == 0 {
        return Err(err());
    }
    Ok((h.to_string(), port))
}

// ---------------------------------------------------------------------------
// SSH-Agent (Pageant, Windows OpenSSH-Agent, 1Password, KeePassXC …)
// ---------------------------------------------------------------------------

trait AgentRw: AsyncRead + AsyncWrite + Send + Unpin {}
impl<T: AsyncRead + AsyncWrite + Send + Unpin> AgentRw for T {}
type AgentStream = Box<dyn AgentRw>;

/// Verbindet zum laufenden SSH-Agent. Reihenfolge: Windows-OpenSSH-Agent (Named Pipe), Pageant.
async fn agent_stream() -> Result<AgentStream, String> {
    #[cfg(windows)]
    {
        if let Ok(c) = tokio::net::windows::named_pipe::ClientOptions::new().open(r"\\.\pipe\openssh-ssh-agent") {
            return Ok(Box::new(c));
        }
        if let Ok(s) = pageant::PageantStream::new().await {
            return Ok(Box::new(s));
        }
    }
    #[cfg(unix)]
    {
        if let Ok(p) = std::env::var("SSH_AUTH_SOCK") {
            if let Ok(s) = tokio::net::UnixStream::connect(p).await {
                return Ok(Box::new(s));
            }
        }
    }
    Err(t!(
        "Kein SSH-Agent gefunden. Bitte Pageant, den Windows-OpenSSH-Agent (Dienst „ssh-agent“), 1Password oder KeePassXC starten.",
        "No SSH agent found. Please start Pageant, the Windows OpenSSH agent (service \"ssh-agent\"), 1Password or KeePassXC."
    )
    .into())
}

/// Schlüssel im Agent (für die Anzeige): (Typ, Kommentar, Fingerprint)
pub async fn agent_keys() -> Result<Vec<(String, String, String)>, String> {
    let mut agent = AgentClient::connect(agent_stream().await?);
    let ids = agent.request_identities().await.map_err(|e| e.to_string())?;
    Ok(ids
        .iter()
        .map(|id| {
            let k = id.public_key();
            (k.algorithm().to_string(), k.comment().to_string(), k.fingerprint(HashAlg::Sha256).to_string())
        })
        .collect())
}

async fn auth_agent(handle: &mut Handle<Client>, user: &str) -> Result<bool, String> {
    let mut agent = AgentClient::connect(agent_stream().await?);
    let ids = agent
        .request_identities()
        .await
        .map_err(|e| tf!("SSH-Agent: {e}", "SSH agent: {e}"))?;
    if ids.is_empty() {
        return Err(t!("Der SSH-Agent enthält keine Schlüssel.", "The SSH agent holds no keys.").into());
    }
    let rsa_hash = handle.best_supported_rsa_hash().await.ok().flatten().flatten();
    for id in ids {
        let key = id.public_key().into_owned();
        let hash = if key.algorithm().is_rsa() { rsa_hash } else { None };
        if let Ok(r) = handle.authenticate_publickey_with(user, key, hash, &mut agent).await {
            if r.success() {
                return Ok(true);
            }
        }
    }
    Ok(false)
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
    shared: Arc<Shared>,
}

impl client::Handler for Client {
    type Error = russh::Error;

    async fn check_server_key(&mut self, key: &russh::keys::PublicKeyOrCertificate) -> Result<bool, Self::Error> {
        let fp = match key {
            russh::keys::PublicKeyOrCertificate::PublicKey { key, .. } => key.fingerprint(HashAlg::Sha256).to_string(),
            russh::keys::PublicKeyOrCertificate::Certificate(c) => c.public_key().fingerprint(HashAlg::Sha256).to_string(),
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
                        tokio::task::spawn_blocking(move || v(h, f)).await.unwrap_or(false)
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

    async fn server_channel_open_forwarded_tcpip(
        &mut self,
        channel: Channel<Msg>,
        _connected_address: &str,
        connected_port: u32,
        _originator_address: &str,
        _originator_port: u32,
        reply: ChannelOpenHandle,
        _session: &mut Session,
    ) -> Result<(), Self::Error> {
        let target = self.shared.remote.lock().unwrap().get(&connected_port).cloned();
        let Some((target, conns)) = target else {
            reply.reject(ChannelOpenFailure::AdministrativelyProhibited).await;
            return Ok(());
        };
        let Ok((h, p)) = parse_target(&target) else {
            reply.reject(ChannelOpenFailure::ConnectFailed).await;
            return Ok(());
        };
        match tokio::time::timeout(CONNECT_TIMEOUT, TcpStream::connect((h.as_str(), p))).await {
            Ok(Ok(sock)) => {
                reply.accept().await;
                conns.fetch_add(1, Ordering::Relaxed);
                let _ = sock.set_nodelay(true);
                tokio::spawn(pump(channel, sock));
            }
            _ => reply.reject(ChannelOpenFailure::ConnectFailed).await,
        }
        Ok(())
    }

    async fn server_channel_open_agent_forward(
        &mut self,
        channel: Channel<Msg>,
        reply: ChannelOpenHandle,
        _session: &mut Session,
    ) -> Result<(), Self::Error> {
        if !self.shared.agent_forward {
            reply.reject(ChannelOpenFailure::AdministrativelyProhibited).await;
            return Ok(());
        }
        match agent_stream().await {
            Ok(agent) => {
                reply.accept().await;
                tokio::spawn(pump(channel, agent));
            }
            Err(_) => reply.reject(ChannelOpenFailure::ConnectFailed).await,
        }
        Ok(())
    }

    async fn server_channel_open_x11(
        &mut self,
        channel: Channel<Msg>,
        _originator_address: &str,
        _originator_port: u32,
        reply: ChannelOpenHandle,
        _session: &mut Session,
    ) -> Result<(), Self::Error> {
        let Some(display) = self.shared.x11_display else {
            reply.reject(ChannelOpenFailure::AdministrativelyProhibited).await;
            return Ok(());
        };
        match TcpStream::connect(("127.0.0.1", 6000 + display as u16)).await {
            Ok(x) => {
                reply.accept().await;
                let _ = x.set_nodelay(true);
                tokio::spawn(x11_pump(channel, x));
            }
            Err(_) => reply.reject(ChannelOpenFailure::ConnectFailed).await,
        }
        Ok(())
    }
}

/// X11: Im ersten Paket steht das (falsche) Cookie, das wir dem Server gegeben haben.
/// Es wird entfernt, damit der lokale X-Server (ohne Zugriffskontrolle) die Verbindung annimmt.
async fn x11_pump(ch: Channel<Msg>, mut x: TcpStream) {
    let mut cs = ch.into_stream();
    let mut head = [0u8; 12];
    if cs.read_exact(&mut head).await.is_err() {
        return;
    }
    let le = head[0] == b'l';
    let rd = |a: u8, b: u8| if le { u16::from_le_bytes([a, b]) } else { u16::from_be_bytes([a, b]) } as usize;
    let pad = |v: usize| (v + 3) & !3;
    let (n, d) = (rd(head[6], head[7]), rd(head[8], head[9]));
    let mut skip = vec![0u8; pad(n) + pad(d)];
    if cs.read_exact(&mut skip).await.is_err() {
        return;
    }
    head[6..10].fill(0);
    if x.write_all(&head).await.is_err() {
        return;
    }
    let _ = tokio::io::copy_bidirectional(&mut cs, &mut x).await;
}

/// Trennt "host", "host:port", "[v6]:port" und reine IPv6-Adressen.
pub fn split_host_port(input: &str) -> (String, u16) {
    split_host_port_or(input, 22)
}

pub fn split_host_port_or(input: &str, default: u16) -> (String, u16) {
    let s = input.trim();
    if let Some(rest) = s.strip_prefix('[') {
        if let Some((h, p)) = rest.split_once(']') {
            let port = p.trim_start_matches(':').parse().unwrap_or(default);
            return (h.to_string(), port);
        }
    }
    if s.matches(':').count() == 1 {
        let (h, p) = s.split_once(':').unwrap();
        if let Ok(port) = p.parse() {
            return (h.to_string(), port);
        }
    }
    (s.to_string(), default)
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

fn auth_failed(msg: &str) -> String {
    format!("{AUTH_FAILED}{msg}")
}

/// Entfernt interne Kennzeichen für die Anzeige.
pub fn clean_error(e: &str) -> &str {
    e.trim_start_matches(AUTH_FAILED)
}

async fn authenticate(handle: &mut Handle<Client>, auth: &Auth) -> Result<(), String> {
    let user = auth.user.as_str();
    let pass = auth.pass.as_str();
    if auth.agent {
        match auth_agent(handle, user).await {
            Ok(true) => return Ok(()),
            Ok(false) if pass.is_empty() => {
                return Err(t!(
                    "Kein Schlüssel aus dem SSH-Agent wurde vom Server akzeptiert.",
                    "The server accepted none of the SSH agent's keys."
                )
                .into())
            }
            Err(e) if pass.is_empty() => return Err(e),
            _ => {} // mit Passwort weiter
        }
    } else if !auth.key.trim().is_empty() {
        let key_pem = auth.key.as_str();
        let encrypted = crate::keys::is_encrypted(key_pem);
        if encrypted && pass.is_empty() {
            return Err(auth_failed(t!("Der SSH-Key ist mit einer Passphrase geschützt.", "The SSH key is protected by a passphrase.")));
        }
        let passphrase = (!pass.is_empty()).then_some(pass);
        let key = russh::keys::decode_secret_key(key_pem, passphrase).map_err(|e| {
            if encrypted {
                auth_failed(t!("Passphrase für den SSH-Key ist falsch.", "Wrong passphrase for the SSH key."))
            } else {
                tf!("Ungültiger SSH-Key: {e}", "Invalid SSH key: {e}")
            }
        })?;
        let hash = handle.best_supported_rsa_hash().await.ok().flatten().flatten();
        let res = handle
            .authenticate_publickey(user, PrivateKeyWithHashAlg::new(Arc::new(key), hash))
            .await
            .map_err(|e| tf!("Auth-Fehler: {e}", "Auth error: {e}"))?;
        if res.success() {
            return Ok(());
        }
        return Err(t!(
            "SSH-Key wurde vom Server abgelehnt (Public Key hinterlegt?)",
            "The server rejected the SSH key (is the public key installed?)"
        )
        .into());
    }

    if !pass.is_empty() {
        let res = handle
            .authenticate_password(user, pass)
            .await
            .map_err(|e| tf!("Auth-Fehler: {e}", "Auth error: {e}"))?;
        if res.success() {
            return Ok(());
        }
        // Viele Switches/Router erlauben nur "keyboard-interactive".
        let mut resp = handle
            .authenticate_keyboard_interactive_start(user, None)
            .await
            .map_err(|e| tf!("Auth-Fehler: {e}", "Auth error: {e}"))?;
        for _ in 0..5 {
            match resp {
                KeyboardInteractiveAuthResponse::Success => return Ok(()),
                KeyboardInteractiveAuthResponse::Failure { .. } => break,
                KeyboardInteractiveAuthResponse::InfoRequest { prompts, .. } => {
                    let answers = prompts.iter().map(|_| pass.to_string()).collect();
                    resp = handle
                        .authenticate_keyboard_interactive_respond(answers)
                        .await
                        .map_err(|e| tf!("Auth-Fehler: {e}", "Auth error: {e}"))?;
                }
            }
        }
        return Err(auth_failed(t!("Benutzername oder Passwort falsch", "Wrong user name or password")));
    }

    let res = handle
        .authenticate_none(user)
        .await
        .map_err(|e| tf!("Auth-Fehler: {e}", "Auth error: {e}"))?;
    if res.success() {
        Ok(())
    } else {
        Err(t!("Kein Passwort/Key angegeben", "No password or key given").into())
    }
}

async fn tcp_connect(host: &str, port: u16) -> Result<TcpStream, String> {
    let tcp = tokio::time::timeout(CONNECT_TIMEOUT, TcpStream::connect((host, port)))
        .await
        .map_err(|_| tf!("Fehler beim Verbinden: Zeitüberschreitung ({host}:{port})", "Connection failed: timeout ({host}:{port})"))?
        .map_err(|e| tf!("Fehler beim Verbinden: {e}", "Connection failed: {e}"))?;
    let _ = tcp.set_nodelay(true);
    Ok(tcp)
}

/// SSH-Handshake über einen beliebigen Datenstrom (TCP oder Kanal eines Jump-Hosts).
async fn handshake<S>(stream: S, host_id: String, opts: &Options, shared: Arc<Shared>) -> Result<Handle<Client>, String>
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let mismatch = Arc::new(Mutex::new(None));
    let handler = Client {
        host_id: host_id.clone(),
        mismatch: mismatch.clone(),
        verify: opts.verify_new_host.clone(),
        shared,
    };
    client::connect_stream(Arc::new(config(opts)), stream, handler)
        .await
        .map_err(|e| match mismatch.lock().unwrap().take() {
            Some(m) if m == REJECTED => {
                t!("Host-Key wurde nicht bestätigt – Verbindung abgebrochen.", "Host key was not confirmed – connection cancelled.").to_string()
            }
            Some(m) => format!("{HOSTKEY_CHANGED}{host_id}|{m}"),
            None => tf!("Handshake Fehler: {e}", "Handshake error: {e}"),
        })
}

struct Established {
    handle: Handle<Client>,
    jump: Option<Handle<Client>>,
    read: ChannelReadHalf,
    write: ChannelWriteHalf<Msg>,
    shared: Arc<Shared>,
}

fn jump_err(e: String) -> String {
    if e.starts_with(HOSTKEY_CHANGED) || e.starts_with(AUTH_FAILED) {
        e
    } else {
        tf!("Jump-Host: {e}", "Jump host: {e}")
    }
}

async fn establish(target: &Target, cols: u32, rows: u32, opts: Options) -> Result<Established, String> {
    let (host, port) = split_host_port(&target.addr);
    let host_id = format!("{host}:{port}");
    let shared = Arc::new(Shared {
        remote: Mutex::new(HashMap::new()),
        x11_display: opts.x11.then_some(opts.x11_display),
        agent_forward: opts.agent_forward,
    });

    let fut = async {
        let jump = match &opts.jump {
            Some(j) => {
                let (jh, jp) = split_host_port(&j.addr);
                let tcp = tcp_connect(&jh, jp).await.map_err(jump_err)?;
                let mut h = handshake(tcp, format!("{jh}:{jp}"), &opts, Arc::new(Shared::default()))
                    .await
                    .map_err(jump_err)?;
                authenticate(&mut h, &j.auth).await.map_err(jump_err)?;
                Some(h)
            }
            None => None,
        };
        let mut handle = match &jump {
            Some(j) => {
                let ch = j
                    .channel_open_direct_tcpip(host.clone(), port as u32, "127.0.0.1", 0)
                    .await
                    .map_err(|e| tf!("Jump-Host erreicht {host_id} nicht: {e}", "Jump host cannot reach {host_id}: {e}"))?;
                handshake(ch.into_stream(), host_id.clone(), &opts, shared.clone()).await?
            }
            None => handshake(tcp_connect(&host, port).await?, host_id.clone(), &opts, shared.clone()).await?,
        };
        authenticate(&mut handle, &target.auth).await?;

        let channel = handle
            .channel_open_session()
            .await
            .map_err(|e| tf!("Session Fehler: {e}", "Session error: {e}"))?;
        let (read, write) = channel.split();
        let modes = [(russh::Pty::ECHO, 1), (russh::Pty::TTY_OP_ISPEED, 14400), (russh::Pty::TTY_OP_OSPEED, 14400)];
        write
            .request_pty(false, "xterm-256color", cols.max(20), rows.max(5), 0, 0, &modes)
            .await
            .map_err(|e| tf!("PTY Fehler: {e}", "PTY error: {e}"))?;
        if opts.agent_forward {
            let _ = write.agent_forward(false).await;
        }
        if opts.x11 {
            let cookie: String = (0..16).map(|_| format!("{:02x}", rand::random::<u8>())).collect();
            let _ = write.request_x11(false, false, "MIT-MAGIC-COOKIE-1", cookie, 0).await;
        }
        write
            .request_shell(false)
            .await
            .map_err(|e| tf!("Shell Fehler: {e}", "Shell error: {e}"))?;
        Ok::<_, String>(Established { handle, jump, read, write, shared: shared.clone() })
    };
    tokio::time::timeout(HANDSHAKE_TIMEOUT, fut)
        .await
        .map_err(|_| t!("Zeitüberschreitung beim Anmelden", "Timeout while logging in").to_string())?
}

/// Baut die Verbindung auf. Die Ausgabe landet im `sink`.
/// Abbrechen: den Future (bzw. Task) einfach abbrechen.
pub async fn connect(target: Target, cols: u32, rows: u32, opts: Options, sink: Sink) -> Result<Arc<SshSession>, String> {
    let forwards = opts.forwards.clone();
    let est = establish(&target, cols, rows, opts).await?;
    let (tx, rx) = mpsc::unbounded_channel();
    let sess = Arc::new(SshSession {
        handle: est.handle,
        jump: est.jump,
        tx,
        sftp: tokio::sync::Mutex::new(None),
        shared: est.shared,
        tunnels: Mutex::new(Vec::new()),
        closed: AtomicBool::new(false),
    });
    for f in forwards.into_iter().filter(|f| f.enabled) {
        let label = f.label();
        let line = match sess.open_tunnel(f).await {
            Ok(()) => tf!("\x1b[90m[Tunnel] {label}\x1b[0m\r\n", "\x1b[90m[Tunnel] {label}\x1b[0m\r\n"),
            Err(e) => tf!("\x1b[33m[Tunnel] {label}: {e}\x1b[0m\r\n", "\x1b[33m[Tunnel] {label}: {e}\x1b[0m\r\n"),
        };
        sink(Event::Data(line.into_bytes()));
    }
    tokio::spawn(writer_task(Arc::downgrade(&sess), est.write, rx));
    tokio::spawn(reader_task(sess.clone(), est.read, sink));
    Ok(sess)
}

async fn writer_task(sess: Weak<SshSession>, write: ChannelWriteHalf<Msg>, mut rx: mpsc::UnboundedReceiver<Cmd>) {
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
    if let Some(sess) = sess.upgrade() {
        sess.closed.store(true, Ordering::Relaxed);
        sess.abort_tunnels();
        let _ = sess.handle.disconnect(russh::Disconnect::ByApplication, "", "de").await;
        if let Some(j) = &sess.jump {
            let _ = j.disconnect(russh::Disconnect::ByApplication, "", "de").await;
        }
    }
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
            sink(Event::Data(std::mem::replace(&mut buf, Vec::with_capacity(64 * 1024))));
        }
    }
    sess.closed.store(true, Ordering::Relaxed);
    sess.abort_tunnels();
    sess.close();
    sink(Event::Closed(t!("[Verbindung getrennt]", "[Disconnected]").into()));
}

/// Führt einen Befehl in einem eigenen Kanal aus (ohne Terminal) und liefert stdout.
pub async fn exec(sess: &SshSession, cmd: &str, timeout: Duration) -> Result<String, String> {
    let fut = async {
        let mut ch = sess.handle.channel_open_session().await.map_err(|e| e.to_string())?;
        ch.exec(true, cmd).await.map_err(|e| e.to_string())?;
        let mut out = Vec::new();
        let mut status = None;
        while let Some(m) = ch.wait().await {
            match m {
                ChannelMsg::Data { data } => out.extend_from_slice(&data),
                ChannelMsg::ExitStatus { exit_status } => status = Some(exit_status),
                ChannelMsg::Failure => return Err("exec abgelehnt".to_string()),
                ChannelMsg::Close => break,
                _ => {}
            }
            if out.len() > 1024 * 1024 {
                break;
            }
        }
        let _ = ch.close().await;
        match status {
            Some(0) | None => Ok(String::from_utf8_lossy(&out).to_string()),
            Some(c) => Err(format!("exit {c}: {}", String::from_utf8_lossy(&out).trim())),
        }
    };
    tokio::time::timeout(timeout, fut).await.map_err(|_| "Timeout".to_string())?
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
        out.push(FileInfo { name: "..".into(), is_dir: true, size: 0, modified: None, perms: String::new(), is_link: false });
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

/// Eintrag beim rekursiven Durchlaufen (Pfad relativ zum Startordner, mit "/").
#[derive(Debug, Clone, PartialEq)]
pub struct TreeEntry {
    pub rel: String,
    pub is_dir: bool,
    pub size: u64,
    pub mtime: u64,
}

/// Durchläuft einen Server-Ordner rekursiv (Symlinks werden nicht verfolgt).
pub async fn walk_remote(sess: &SshSession, root: &str, limit: usize) -> Result<Vec<TreeEntry>, String> {
    let sftp = sess.sftp().await?;
    let mut out = Vec::new();
    let mut stack = vec![String::new()];
    while let Some(rel) = stack.pop() {
        let dir = if rel.is_empty() { root.to_string() } else { join(root, &rel) };
        for e in sftp.read_dir(&dir).await.map_err(|e| format!("{dir}: {e}"))? {
            let name = e.file_name();
            if name == "." || name == ".." {
                continue;
            }
            let m = e.metadata();
            if m.is_symlink() {
                continue;
            }
            let r = if rel.is_empty() { name } else { format!("{rel}/{name}") };
            let is_dir = m.is_dir();
            if is_dir {
                stack.push(r.clone());
            }
            out.push(TreeEntry { rel: r, is_dir, size: m.size.unwrap_or(0), mtime: m.mtime.unwrap_or(0) as u64 });
            if out.len() >= limit {
                return Err(tf!("Mehr als {limit} Einträge – bitte einen kleineren Ordner wählen.", "More than {limit} entries – please pick a smaller folder."));
            }
        }
    }
    out.sort_by(|a, b| a.rel.cmp(&b.rel));
    Ok(out)
}

/// Durchläuft einen lokalen Ordner rekursiv.
pub fn walk_local(root: &std::path::Path, limit: usize) -> Result<Vec<TreeEntry>, String> {
    let mut out = Vec::new();
    let mut stack = vec![(root.to_path_buf(), String::new())];
    while let Some((dir, rel)) = stack.pop() {
        for e in std::fs::read_dir(&dir).map_err(|e| format!("{}: {e}", dir.display()))?.flatten() {
            let name = e.file_name().to_string_lossy().to_string();
            let Ok(m) = std::fs::symlink_metadata(e.path()) else { continue };
            if m.file_type().is_symlink() {
                continue;
            }
            let r = if rel.is_empty() { name } else { format!("{rel}/{name}") };
            let mtime = m
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_secs())
                .unwrap_or(0);
            if m.is_dir() {
                stack.push((e.path(), r.clone()));
            }
            out.push(TreeEntry { rel: r, is_dir: m.is_dir(), size: if m.is_dir() { 0 } else { m.len() }, mtime });
            if out.len() >= limit {
                return Err(tf!("Mehr als {limit} Einträge – bitte einen kleineren Ordner wählen.", "More than {limit} entries – please pick a smaller folder."));
            }
        }
    }
    out.sort_by(|a, b| a.rel.cmp(&b.rel));
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
    String::from_utf8(data).map_err(|_| t!("Binärdatei – bitte den Download nutzen", "Binary file – please use download").to_string())
}

pub async fn write_file(sess: &SshSession, path: &str, content: &str) -> Result<(), String> {
    let sftp = sess.sftp().await?;
    let mut f = sftp.create(path).await.map_err(|e| e.to_string())?;
    f.write_all(content.as_bytes()).await.map_err(|e| e.to_string())?;
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
    let meta = sftp.symlink_metadata(path).await.map_err(|e| e.to_string())?;
    if !meta.is_dir() || meta.is_symlink() {
        return sftp.remove_file(path).await.map_err(|e| e.to_string());
    }
    let mut stack = vec![(path.to_string(), false)];
    while let Some((dir, visited)) = stack.pop() {
        if visited {
            sftp.remove_dir(&dir).await.map_err(|e| format!("{dir}: {e}"))?;
            continue;
        }
        stack.push((dir.clone(), true));
        for e in sftp.read_dir(&dir).await.map_err(|e| format!("{dir}: {e}"))? {
            let name = e.file_name();
            if name == "." || name == ".." {
                continue;
            }
            let p = join(&dir, &name);
            let m = e.metadata();
            if m.is_dir() && !m.is_symlink() {
                stack.push((p, false));
            } else {
                sftp.remove_file(&p).await.map_err(|e| format!("{p}: {e}"))?;
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

/// Legt einen Ordner samt fehlender Elternordner an.
pub async fn mkdir_all(sess: &SshSession, path: &str) -> Result<(), String> {
    let sftp = sess.sftp().await?;
    let mut acc = String::new();
    for part in path.split('/').filter(|p| !p.is_empty()) {
        acc.push('/');
        acc.push_str(part);
        if !sftp.try_exists(acc.clone()).await.unwrap_or(false) {
            sftp.create_dir(acc.clone()).await.map_err(|e| format!("{acc}: {e}"))?;
        }
    }
    Ok(())
}

/// Ändert die Rechte (z.B. 0o644).
pub async fn chmod(sess: &SshSession, path: &str, mode: u32) -> Result<(), String> {
    let sftp = sess.sftp().await?;
    let mut attrs = russh_sftp::protocol::FileAttributes::empty();
    attrs.permissions = Some(mode);
    sftp.set_metadata(path, attrs).await.map_err(|e| e.to_string())
}

/// Hinterlegt einen Public Key in ~/.ssh/authorized_keys (mit korrekten Rechten).
/// Gibt `false` zurück, wenn der Key schon vorhanden war.
pub async fn install_public_key(sess: &SshSession, public_line: &str) -> Result<bool, String> {
    let sftp = sess.sftp().await?;
    let home = sftp.canonicalize(".").await.map_err(|e| e.to_string())?;
    let dir = join(&home, ".ssh");
    if !sftp.try_exists(dir.clone()).await.unwrap_or(false) {
        sftp.create_dir(dir.clone()).await.map_err(|e| tf!(".ssh anlegen: {e}", "create .ssh: {e}"))?;
    }
    let mut attrs = russh_sftp::protocol::FileAttributes::empty();
    attrs.permissions = Some(0o700);
    let _ = sftp.set_metadata(dir.clone(), attrs).await;

    let file = join(&dir, "authorized_keys");
    let existing = match sftp.read(file.clone()).await {
        Ok(b) => String::from_utf8_lossy(&b).to_string(),
        Err(_) => String::new(),
    };
    let key_part: String = public_line.split_whitespace().take(2).collect::<Vec<_>>().join(" ");
    if existing.lines().any(|l| l.contains(&key_part)) {
        return Ok(false);
    }
    let mut content = existing;
    if !content.is_empty() && !content.ends_with('\n') {
        content.push('\n');
    }
    content.push_str(public_line.trim());
    content.push('\n');
    sftp.write(file.clone(), content.as_bytes()).await.map_err(|e| e.to_string())?;
    let mut attrs = russh_sftp::protocol::FileAttributes::empty();
    attrs.permissions = Some(0o600);
    let _ = sftp.set_metadata(file, attrs).await;
    Ok(true)
}

/// Steuerung einer Übertragung: Fortschritt, Pause, Abbruch.
#[derive(Default)]
pub struct Ctl {
    pub done: AtomicU64,
    pub paused: AtomicBool,
    pub cancel: AtomicBool,
}

pub type Progress = Option<Arc<Ctl>>;

/// Kennzeichen für abgebrochene Übertragungen.
pub const CANCELLED: &str = "\u{2}";

fn bump(p: &Progress, n: usize) {
    if let Some(p) = p {
        p.done.fetch_add(n as u64, Ordering::Relaxed);
    }
}

/// Wartet während einer Pause; Fehler bei Abbruch.
async fn gate(p: &Progress) -> Result<(), String> {
    if let Some(c) = p {
        loop {
            if c.cancel.load(Ordering::Relaxed) {
                return Err(CANCELLED.into());
            }
            if !c.paused.load(Ordering::Relaxed) {
                return Ok(());
            }
            tokio::time::sleep(Duration::from_millis(150)).await;
        }
    }
    Ok(())
}

async fn copy<R, W>(src: &mut R, dst: &mut W, progress: &Progress) -> Result<(), String>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let mut buf = vec![0u8; COPY_BUF];
    loop {
        gate(progress).await?;
        let n = src.read(&mut buf).await.map_err(|e| e.to_string())?;
        if n == 0 {
            return Ok(());
        }
        dst.write_all(&buf[..n]).await.map_err(|e| e.to_string())?;
        bump(progress, n);
    }
}

fn unix_mtime(m: &std::fs::Metadata) -> Option<u32> {
    m.modified()
        .ok()?
        .duration_since(std::time::UNIX_EPOCH)
        .ok()
        .map(|d| d.as_secs() as u32)
}

pub async fn upload(sess: &SshSession, remote_dir: &str, local: &str) -> Result<(), String> {
    upload_with(sess, remote_dir, local, None).await
}

pub async fn download(sess: &SshSession, remote: &str, local: &str) -> Result<(), String> {
    download_with(sess, remote, local, None, false).await
}

/// Lädt eine Datei oder einen ganzen Ordner (rekursiv) in `remote_dir` hoch.
pub async fn upload_with(sess: &SshSession, remote_dir: &str, local: &str, progress: Progress) -> Result<(), String> {
    let lp = std::path::Path::new(local);
    let name = lp
        .file_name()
        .and_then(|n| n.to_str())
        .ok_or(t!("Ungültiger Dateiname", "Invalid file name"))?;
    let target = join(remote_dir, name);
    if lp.is_dir() {
        let sftp = sess.sftp().await?;
        if !sftp.try_exists(target.clone()).await.unwrap_or(false) {
            sftp.create_dir(target.clone()).await.map_err(|e| format!("{target}: {e}"))?;
        }
        let entries = std::fs::read_dir(lp).map_err(|e| e.to_string())?;
        for e in entries.flatten() {
            let p = e.path();
            Box::pin(upload_with(sess, &target, &p.to_string_lossy(), progress.clone())).await?;
        }
        return Ok(());
    }
    upload_to(sess, local, &target, progress, false).await
}

/// Lädt eine einzelne Datei an genau diesen Server-Pfad hoch. Mit `resume` wird eine
/// teilweise vorhandene Zieldatei fortgesetzt. Die Änderungszeit wird übernommen.
pub async fn upload_to(sess: &SshSession, local: &str, remote: &str, progress: Progress, resume: bool) -> Result<(), String> {
    use russh_sftp::protocol::OpenFlags;
    let sftp = sess.sftp().await?;
    let mut src = tokio::fs::File::open(local).await.map_err(|e| e.to_string())?;
    let meta = src.metadata().await.map_err(|e| e.to_string())?;
    let mut offset = 0u64;
    if resume {
        if let Ok(m) = sftp.metadata(remote.to_string()).await {
            offset = m.size.unwrap_or(0).min(meta.len());
        }
    }
    let mut dst = if offset > 0 {
        let mut f = sftp
            .open_with_flags(remote.to_string(), OpenFlags::WRITE)
            .await
            .map_err(|e| e.to_string())?;
        f.seek(std::io::SeekFrom::Start(offset)).await.map_err(|e| e.to_string())?;
        src.seek(std::io::SeekFrom::Start(offset)).await.map_err(|e| e.to_string())?;
        bump(&progress, offset as usize);
        f
    } else {
        sftp.create(remote.to_string()).await.map_err(|e| e.to_string())?
    };
    copy(&mut src, &mut dst, &progress).await?;
    dst.shutdown().await.map_err(|e| e.to_string())?;
    if let Some(t) = unix_mtime(&meta) {
        let mut attrs = russh_sftp::protocol::FileAttributes::empty();
        attrs.mtime = Some(t);
        attrs.atime = Some(t);
        let _ = sftp.set_metadata(remote.to_string(), attrs).await;
    }
    Ok(())
}

/// Lädt eine Datei herunter. Mit `resume` wird eine vorhandene Teil-Datei fortgesetzt.
/// Die Änderungszeit des Servers wird übernommen.
pub async fn download_with(sess: &SshSession, remote: &str, local: &str, progress: Progress, resume: bool) -> Result<(), String> {
    let sftp = sess.sftp().await?;
    let rmeta = sftp.metadata(remote.to_string()).await.map_err(|e| e.to_string())?;
    let size = rmeta.size.unwrap_or(0);
    let mut offset = 0u64;
    if resume {
        if let Ok(m) = std::fs::metadata(local) {
            offset = m.len().min(size);
        }
    }
    let mut src = sftp.open(remote).await.map_err(|e| e.to_string())?;
    let mut dst = if offset > 0 {
        src.seek(std::io::SeekFrom::Start(offset)).await.map_err(|e| e.to_string())?;
        bump(&progress, offset as usize);
        tokio::fs::OpenOptions::new().append(true).open(local).await.map_err(|e| e.to_string())?
    } else {
        tokio::fs::File::create(local).await.map_err(|e| e.to_string())?
    };
    copy(&mut src, &mut dst, &progress).await?;
    dst.flush().await.map_err(|e| e.to_string())?;
    drop(dst);
    if let Some(t) = rmeta.mtime {
        if let Ok(f) = std::fs::File::options().write(true).open(local) {
            let _ = f.set_modified(std::time::UNIX_EPOCH + Duration::from_secs(t as u64));
        }
    }
    Ok(())
}

/// Lädt einen ganzen Server-Ordner nach `local_parent/<name>` herunter.
pub async fn download_tree(sess: &SshSession, remote_dir: &str, local_parent: &std::path::Path, progress: Progress) -> Result<(), String> {
    let name = remote_dir.trim_end_matches('/').rsplit('/').next().unwrap_or("download");
    let base = local_parent.join(name);
    std::fs::create_dir_all(&base).map_err(|e| e.to_string())?;
    for e in walk_remote(sess, remote_dir, 200_000).await? {
        let lp = base.join(e.rel.replace('/', "\\"));
        if e.is_dir {
            std::fs::create_dir_all(&lp).map_err(|e| e.to_string())?;
        } else {
            if let Some(p) = lp.parent() {
                std::fs::create_dir_all(p).map_err(|e| e.to_string())?;
            }
            download_with(sess, &join(remote_dir, &e.rel), &lp.to_string_lossy(), progress.clone(), false).await?;
        }
    }
    Ok(())
}

/// Gesamtgröße eines Server-Ordners (für die Fortschrittsanzeige).
pub async fn remote_size(sess: &SshSession, path: &str) -> u64 {
    walk_remote(sess, path, 200_000)
        .await
        .map(|v| v.iter().map(|e| e.size).sum())
        .unwrap_or(0)
}

/// Gesamtgröße einer lokalen Datei bzw. eines Ordners (für die Fortschrittsanzeige).
pub fn local_size(path: &str) -> u64 {
    let p = std::path::Path::new(path);
    if p.is_dir() {
        std::fs::read_dir(p)
            .map(|rd| rd.flatten().map(|e| local_size(&e.path().to_string_lossy())).sum())
            .unwrap_or(0)
    } else {
        std::fs::metadata(p).map(|m| m.len()).unwrap_or(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn host_port_parsing() {
        assert_eq!(split_host_port("10.0.0.1"), ("10.0.0.1".into(), 22));
        assert_eq!(split_host_port("10.0.0.1:2222"), ("10.0.0.1".into(), 2222));
        assert_eq!(split_host_port("example.com"), ("example.com".into(), 22));
        assert_eq!(split_host_port("[::1]:2200"), ("::1".into(), 2200));
        assert_eq!(split_host_port("fe80::1"), ("fe80::1".into(), 22));
        assert_eq!(split_host_port(" host:abc "), ("host:abc".into(), 22));
        assert_eq!(split_host_port_or("switch", 23), ("switch".into(), 23));
    }

    #[test]
    fn bind_and_target_parsing() {
        assert_eq!(parse_bind("8080", "127.0.0.1").unwrap(), ("127.0.0.1".into(), 8080));
        assert_eq!(parse_bind("0.0.0.0:9000", "x").unwrap(), ("0.0.0.0".into(), 9000));
        assert_eq!(parse_bind("*:9000", "x").unwrap(), ("0.0.0.0".into(), 9000));
        assert_eq!(parse_bind("[::1]:80", "x").unwrap(), ("::1".into(), 80));
        assert!(parse_bind("abc", "x").is_err());
        assert_eq!(parse_target("db.local:5432").unwrap(), ("db.local".into(), 5432));
        assert!(parse_target("db.local").is_err());
        assert!(parse_target(":80").is_err());
    }
}

#[cfg(test)]
mod integration {
    //! Läuft gegen einen lokalen Test-Server:
    //! NEBULA_TEST_SSH="127.0.0.1:2222|user|pass" cargo test -p nebula-core integration -- --ignored
    use super::*;

    fn target() -> Target {
        let v = std::env::var("NEBULA_TEST_SSH").expect("NEBULA_TEST_SSH fehlt");
        let p: Vec<&str> = v.split('|').collect();
        Target::password(p[0], p[1], p[2])
    }

    fn collect() -> (Sink, mpsc::UnboundedReceiver<Vec<u8>>) {
        let (dtx, read) = mpsc::unbounded_channel::<Vec<u8>>();
        let sink: Sink = Arc::new(move |e| {
            if let Event::Data(d) = e {
                let _ = dtx.send(d);
            }
        });
        (sink, read)
    }

    async fn wait_for(read: &mut mpsc::UnboundedReceiver<Vec<u8>>, needle: &str) -> String {
        let mut out = Vec::new();
        let deadline = tokio::time::Instant::now() + Duration::from_secs(10);
        while !String::from_utf8_lossy(&out).contains(needle) {
            let m = tokio::time::timeout_at(deadline, read.recv()).await.expect("Timeout");
            if let Some(data) = m {
                out.extend_from_slice(&data);
            }
        }
        String::from_utf8_lossy(&out).to_string()
    }

    #[tokio::test]
    #[ignore]
    async fn shell_and_sftp() {
        let (sink, mut read) = collect();
        let sess = connect(target(), 100, 30, Options::default(), sink).await.unwrap();

        sess.send(Cmd::Resize(120, 40));
        sess.send(Cmd::Data(b"stty size; echo NEBULA_$((6*7))\n".to_vec()));
        let text = wait_for(&mut read, "NEBULA_42").await;
        assert!(text.contains("40 120"), "PTY-Größe falsch: {text}");

        // exec
        let out = exec(&sess, "echo hallo-$((2+3))", Duration::from_secs(10)).await.unwrap();
        assert_eq!(out.trim(), "hallo-5");

        // SFTP
        let home = home(&sess).await.unwrap();
        assert!(home.starts_with('/'));
        write_file(&sess, "/tmp/nebula.txt", "hallo äöü").await.unwrap();
        assert_eq!(read_file(&sess, "/tmp/nebula.txt").await.unwrap(), "hallo äöü");
        let list = list_directory(&sess, "/tmp").await.unwrap();
        let f = list.iter().find(|f| f.name == "nebula.txt").expect("Datei fehlt");
        assert!(f.perms.starts_with('-') && f.modified.is_some());
        chmod(&sess, "/tmp/nebula.txt", 0o600).await.unwrap();
        mkdir_all(&sess, "/tmp/nebula_dir/sub").await.unwrap();
        rename(&sess, "/tmp/nebula.txt", "/tmp/nebula_dir/sub/x.txt").await.unwrap();
        let local = std::env::temp_dir().join("nebula_dl.txt");
        let prog = Arc::new(Ctl::default());
        download_with(&sess, "/tmp/nebula_dir/sub/x.txt", local.to_str().unwrap(), Some(prog.clone()), false)
            .await
            .unwrap();
        assert_eq!(prog.done.load(Ordering::Relaxed), "hallo äöü".len() as u64);
        assert_eq!(std::fs::read_to_string(&local).unwrap(), "hallo äöü");

        // Fortsetzen: halbe Datei lokal, Rest nachladen
        let big: Vec<u8> = (0..300_000u32).map(|i| (i % 251) as u8).collect();
        let lbig = std::env::temp_dir().join("nebula_big.bin");
        std::fs::write(&lbig, &big).unwrap();
        upload_to(&sess, lbig.to_str().unwrap(), "/tmp/nebula_dir/big.bin", None, false).await.unwrap();
        let part = std::env::temp_dir().join("nebula_part.bin");
        std::fs::write(&part, &big[..123_456]).unwrap();
        let prog = Arc::new(Ctl::default());
        download_with(&sess, "/tmp/nebula_dir/big.bin", part.to_str().unwrap(), Some(prog.clone()), true).await.unwrap();
        assert_eq!(std::fs::read(&part).unwrap(), big);
        assert_eq!(prog.done.load(Ordering::Relaxed), big.len() as u64);
        // Upload fortsetzen
        write_file(&sess, "/tmp/nebula_dir/half.bin", "").await.unwrap();
        let sftp = sess.sftp().await.unwrap();
        sftp.write("/tmp/nebula_dir/half.bin", &big[..50_000]).await.unwrap();
        upload_to(&sess, lbig.to_str().unwrap(), "/tmp/nebula_dir/half.bin", None, true).await.unwrap();
        assert_eq!(sftp.read("/tmp/nebula_dir/half.bin").await.unwrap(), big);
        // Abbrechen
        let ctl = Arc::new(Ctl::default());
        ctl.cancel.store(true, Ordering::Relaxed);
        let err = upload_to(&sess, lbig.to_str().unwrap(), "/tmp/nebula_dir/c.bin", Some(ctl), false).await.unwrap_err();
        assert_eq!(err, CANCELLED);
        // Änderungszeit übernommen
        let m = sftp.metadata("/tmp/nebula_dir/big.bin").await.unwrap();
        let lm = std::fs::metadata(&lbig).unwrap();
        assert_eq!(m.mtime, unix_mtime(&lm));

        // Ordner-Upload rekursiv + Walk + Download-Baum
        let ldir = std::env::temp_dir().join("nebula_up");
        let _ = std::fs::remove_dir_all(&ldir);
        std::fs::create_dir_all(ldir.join("a")).unwrap();
        std::fs::write(ldir.join("a/b.txt"), "B").unwrap();
        std::fs::write(ldir.join("c.txt"), "C").unwrap();
        upload_with(&sess, "/tmp/nebula_dir", ldir.to_str().unwrap(), None).await.unwrap();
        assert_eq!(read_file(&sess, "/tmp/nebula_dir/nebula_up/a/b.txt").await.unwrap(), "B");
        let tree = walk_remote(&sess, "/tmp/nebula_dir/nebula_up", 1000).await.unwrap();
        let rels: Vec<&str> = tree.iter().map(|e| e.rel.as_str()).collect();
        assert_eq!(rels, ["a", "a/b.txt", "c.txt"]);
        let dl = std::env::temp_dir().join("nebula_dltree");
        let _ = std::fs::remove_dir_all(&dl);
        download_tree(&sess, "/tmp/nebula_dir/nebula_up", &dl, None).await.unwrap();
        assert_eq!(std::fs::read_to_string(dl.join("nebula_up/a/b.txt")).unwrap(), "B");
        let ltree = walk_local(&dl.join("nebula_up"), 1000).unwrap();
        assert_eq!(ltree.iter().map(|e| e.rel.as_str()).collect::<Vec<_>>(), ["a", "a/b.txt", "c.txt"]);

        // rekursiv löschen
        delete_recursive(&sess, "/tmp/nebula_dir").await.unwrap();
        assert!(list_directory(&sess, "/tmp").await.unwrap().iter().all(|f| f.name != "nebula_dir"));

        // Public Key hinterlegen (zweimal: zweites Mal "schon vorhanden")
        let k = crate::keys::generate(crate::keys::KeyType::Ed25519, "nebula-test", "").unwrap();
        assert!(install_public_key(&sess, &k.public_line).await.unwrap());
        assert!(!install_public_key(&sess, &k.public_line).await.unwrap());

        // Durchsatz: 20 MB Ausgabe
        let start = std::time::Instant::now();
        sess.send(Cmd::Data(b"head -c 20000000 /dev/zero | tr '\\0' 'x'; echo; echo DONE_$((1+1))\n".to_vec()));
        let mut total = 0usize;
        let mut tail = String::new();
        while !tail.contains("DONE_2") {
            match tokio::time::timeout(Duration::from_secs(30), read.recv()).await.expect("Timeout") {
                Some(data) => {
                    total += data.len();
                    tail = String::from_utf8_lossy(&data[data.len().saturating_sub(64)..]).into();
                }
                None => panic!("Kanal zu"),
            }
        }
        let secs = start.elapsed().as_secs_f64();
        println!("Durchsatz: {:.1} MB in {:.2}s = {:.1} MB/s", total as f64 / 1e6, secs, total as f64 / 1e6 / secs);
        sess.close();

        // Mit dem eben hinterlegten Key anmelden
        let mut t = target();
        t.auth.pass.clear();
        t.auth.key = k.private_openssh;
        establish(&t, 80, 24, Options::default()).await.expect("Login mit hinterlegtem Key");
    }

    #[tokio::test]
    #[ignore]
    async fn wrong_password_is_rejected() {
        let mut t = target();
        t.auth.pass = "falsch".into();
        let err = establish(&t, 80, 24, Options::default()).await.err().unwrap();
        assert!(err.starts_with(AUTH_FAILED), "{err}");
    }

    #[tokio::test]
    #[ignore]
    async fn new_host_can_be_rejected() {
        let t = target();
        let (h, p) = split_host_port(&t.addr);
        forget_host_key(&format!("{h}:{p}"));
        let opts = Options { verify_new_host: Some(Arc::new(|_, _| false)), ..Default::default() };
        let err = establish(&t, 80, 24, opts).await.err().unwrap();
        assert!(err.contains("nicht bestätigt") || err.contains("not confirmed"), "{err}");
        // Danach bestätigen -> klappt und wird gespeichert
        let opts = Options { verify_new_host: Some(Arc::new(|_, _| true)), ..Default::default() };
        establish(&t, 80, 24, opts).await.unwrap();
        assert!(known_hosts().iter().any(|(k, _)| *k == format!("{h}:{p}")));
    }

    /// Jump-Host: der Testserver dient als Zwischenstation zu sich selbst (sshd im Container auf 2222).
    #[tokio::test]
    #[ignore]
    async fn jump_host() {
        let t = target();
        let inner = Target { addr: "127.0.0.1:2222".into(), auth: t.auth.clone() };
        let opts = Options { jump: Some(t.clone()), ..Default::default() };
        let (sink, mut read) = collect();
        let sess = connect(inner, 80, 24, opts, sink).await.expect("über Jump-Host verbinden");
        sess.send(Cmd::Data(b"echo JUMP_$((40+2))\n".to_vec()));
        wait_for(&mut read, "JUMP_42").await;
        sess.close();
    }

    /// Lokaler Tunnel und SOCKS-Proxy auf den SSH-Port des Servers selbst.
    #[tokio::test]
    #[ignore]
    async fn tunnels() {
        let (sink, _read) = collect();
        let fwds = vec![
            Forward { kind: "local".into(), bind: "127.0.0.1:47011".into(), target: "127.0.0.1:2222".into(), enabled: true },
            Forward { kind: "dynamic".into(), bind: "47012".into(), target: String::new(), enabled: true },
            Forward { kind: "remote".into(), bind: "47013".into(), target: "127.0.0.1:47011".into(), enabled: true },
        ];
        let opts = Options { forwards: fwds, ..Default::default() };
        let sess = connect(target(), 80, 24, opts, sink).await.unwrap();
        let infos = sess.tunnels();
        assert_eq!(infos.len(), 3);
        for i in &infos {
            assert!(i.error.is_none(), "{:?}", i);
        }
        // -L: Banner des sshd durch den Tunnel lesen
        let mut s = TcpStream::connect("127.0.0.1:47011").await.unwrap();
        let mut buf = [0u8; 7];
        s.read_exact(&mut buf).await.unwrap();
        assert_eq!(&buf, b"SSH-2.0");
        // -D: SOCKS5 CONNECT auf 127.0.0.1:2222
        let mut s = TcpStream::connect("127.0.0.1:47012").await.unwrap();
        s.write_all(&[5, 1, 0]).await.unwrap();
        let mut r = [0u8; 2];
        s.read_exact(&mut r).await.unwrap();
        assert_eq!(r, [5, 0]);
        s.write_all(&[5, 1, 0, 1, 127, 0, 0, 1, 0x08, 0xae]).await.unwrap();
        let mut r = [0u8; 10];
        s.read_exact(&mut r).await.unwrap();
        assert_eq!(r[1], 0, "SOCKS-Antwort {r:?}");
        let mut buf = [0u8; 7];
        s.read_exact(&mut buf).await.unwrap();
        assert_eq!(&buf, b"SSH-2.0");
        // -R: auf dem Server Port 47013 -> hier 47011 -> Server:2222
        let out = exec(&sess, "exec 3<>/dev/tcp/127.0.0.1/47013 && head -c 7 <&3 || nc -w 3 127.0.0.1 47013 </dev/null | head -c 7", Duration::from_secs(15)).await;
        println!("Remote-Tunnel: {out:?}");
        // Tunnel schließen -> Port frei
        sess.close_tunnel(0).await;
        tokio::time::sleep(Duration::from_millis(200)).await;
        assert!(tokio::net::TcpListener::bind("127.0.0.1:47011").await.is_ok());
        sess.close();
    }

    /// X11: ein simulierter X-Server auf :57 (Port 6057) empfängt das Setup-Paket ohne Cookie.
    #[tokio::test]
    #[ignore]
    async fn x11_forwarding() {
        let xserver = tokio::net::TcpListener::bind("127.0.0.1:6057").await.unwrap();
        let (sink, mut read) = collect();
        let opts = Options { x11: true, x11_display: 57, ..Default::default() };
        let sess = connect(target(), 80, 24, opts, sink).await.unwrap();
        sess.send(Cmd::Data(b"echo D=$DISPLAY\n".to_vec()));
        let out = wait_for(&mut read, "D=").await;
        println!("{out}");
        // Setup-Paket: 'l', Version 11.0, Name 18 Bytes, Cookie 16 Bytes, danach "HELLO"
        let cmd = r"printf 'l\000\013\000\000\000\022\000\020\000\000\000MIT-MAGIC-COOKIE-1\000\0000123456789abcdefHELLO' > /dev/tcp/127.0.0.1/6010";
        let s2 = sess.clone();
        tokio::spawn(async move {
            let _ = exec(&s2, cmd, Duration::from_secs(10)).await;
        });
        let (mut x, _) = tokio::time::timeout(Duration::from_secs(10), xserver.accept()).await.expect("keine X11-Verbindung").unwrap();
        let mut got = [0u8; 17];
        x.read_exact(&mut got).await.unwrap();
        assert_eq!(&got[..12], &[b'l', 0, 11, 0, 0, 0, 0, 0, 0, 0, 0, 0]);
        assert_eq!(&got[12..], b"HELLO");
        sess.close();
    }

    #[tokio::test]
    #[ignore]
    async fn login_with_keys() {
        let t = target();
        for f in std::env::var("NEBULA_TEST_KEYS").unwrap().split(';') {
            let pem = std::fs::read_to_string(f).unwrap();
            let mut t = t.clone();
            t.auth.pass.clear();
            t.auth.key = pem;
            establish(&t, 80, 24, Options::default()).await.unwrap_or_else(|e| panic!("{f}: {e}"));
            println!("Key-Login OK: {f}");
        }
    }
}
