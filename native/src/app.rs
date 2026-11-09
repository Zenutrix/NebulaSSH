//! Hauptfenster von NebulaSSH: Zustand, Verbindungslogik und Grundlayout.

use crate::data::{self, BaudProfile, Host, Prefs, Snippet, SshKey};
use crate::terminal::{Output, Size, Terminal, View};
use crate::theme;
use eframe::egui::{
    self, vec2, Align, Color32, FontId, Key, Layout, Margin, Modifiers, RichText, Stroke, Ui,
};
use nebula_core::ssh::{self, FileInfo, SshSession};
use nebula_core::{keys, serial, Cmd, Connection, Event, Sink};
use rfd::{MessageButtons, MessageDialog, MessageDialogResult, MessageLevel};
use std::io::Write as _;
use std::sync::atomic::AtomicU64;
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

pub const BG: Color32 = Color32::from_rgb(0x0f, 0x0f, 0x17);
pub const PANEL: Color32 = Color32::from_rgb(0x16, 0x16, 0x21);
pub const PANEL2: Color32 = Color32::from_rgb(0x1e, 0x1e, 0x2d);
pub const FIELD: Color32 = Color32::from_rgb(0x23, 0x23, 0x33);
pub const LINE: Color32 = Color32::from_rgb(0x2e, 0x2e, 0x3e);
pub const MUTED: Color32 = Color32::from_rgb(0x8e, 0x8d, 0xa3);
pub const RED: Color32 = Color32::from_rgb(0xff, 0x4d, 0x73);
pub const GREEN: Color32 = Color32::from_rgb(0x5c, 0xdb, 0x7d);
pub const AMBER: Color32 = Color32::from_rgb(0xff, 0xb5, 0x47);
pub const ACCENT: Color32 = Color32::from_rgb(0x62, 0x00, 0xee);
pub const ACCENT_HI: Color32 = Color32::from_rgb(0x7c, 0x22, 0xff);
pub const LAVENDER: Color32 = Color32::from_rgb(0xb9, 0xa4, 0xff);
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

#[derive(PartialEq, Clone, Copy)]
pub enum Status {
    Connecting,
    Connected,
    Closed,
}

#[derive(Clone, Copy, PartialEq)]
pub enum SortBy {
    Name,
    Size,
    Date,
}

pub struct Sftp {
    pub path: String,
    pub path_edit: String,
    pub files: Vec<FileInfo>,
    pub loading: bool,
    pub error: Option<String>,
    pub history: Vec<String>,
    pub show_hidden: bool,
    pub filter: String,
    pub sort: SortBy,
    pub asc: bool,
    pub initialized: bool,
    pub selected: Option<String>,
    pub editing_path: bool,
}

impl Default for Sftp {
    fn default() -> Self {
        Self {
            path: String::new(),
            path_edit: String::new(),
            files: Vec::new(),
            loading: false,
            error: None,
            history: Vec::new(),
            show_hidden: false,
            filter: String::new(),
            sort: SortBy::Name,
            asc: true,
            initialized: false,
            selected: None,
            editing_path: false,
        }
    }
}

pub struct Tab {
    pub id: u64,
    pub title: String,
    pub host: Host,
    /// Zugangsdaten dieser Sitzung (für "Neu verbinden"), nur im Speicher
    pub secret: String,
    pub term: Terminal,
    pub conn: Option<Arc<dyn Connection>>,
    pub ssh: Option<Arc<SshSession>>,
    pub task: Option<tokio::task::AbortHandle>,
    pub status: Status,
    pub sftp: Sftp,
    pub log: Option<Arc<Mutex<std::fs::File>>>,
    pub connected_at: Option<Instant>,
}

impl Tab {
    pub fn send(&self, cmd: Cmd) {
        if let Some(c) = &self.conn {
            c.send(cmd);
        }
    }
    pub fn shutdown(&mut self) {
        if let Some(t) = self.task.take() {
            t.abort();
        }
        if let Some(c) = self.conn.take() {
            c.close();
        }
        self.ssh = None;
    }
    pub fn is_ssh(&self) -> bool {
        !self.host.is_serial()
    }
}

pub enum TransferState {
    Running,
    Done(Instant),
    Failed(String),
}

pub struct Transfer {
    pub id: u64,
    pub label: String,
    pub upload: bool,
    pub total: u64,
    pub done: Arc<AtomicU64>,
    pub state: TransferState,
}

pub enum Msg {
    SshConnected(u64, Arc<SshSession>),
    SerialConnected(u64, Arc<serial::SerialSession>),
    Failed(u64, String),
    Closed(u64),
    SftpList(u64, String, Result<Vec<FileInfo>, String>),
    SftpHome(u64, Result<String, String>),
    SftpDone(u64, Result<(), String>, &'static str),
    FileLoaded(u64, String, Result<String, String>),
    FileSaved(u64, Result<(), String>),
    TransferDone(u64, u64, Result<(), String>),
    KeyGenerated(Result<keys::GeneratedKey, String>),
    KeyInstalled(Result<bool, String>),
    UpdateChecked(Result<Option<nebula_core::update::Release>, String>, bool),
    UpdateDownloaded(Result<std::path::PathBuf, String>, bool),
}

#[derive(Clone, Copy, PartialEq)]
pub enum SettingsPage {
    Terminal,
    Connection,
    Security,
    Sessions,
    Backup,
    About,
}

pub enum Dialog {
    AddBaud { name: String, rate: String },
    AddSnippet { name: String, cmd: String },
    EditHost { index: Option<usize>, host: Host, new_group: String },
    Editor {
        tab: u64,
        path: String,
        content: String,
        original: String,
        loading: bool,
        saving: bool,
    },
    Rename { tab: u64, old: String, new: String },
    MkDir { tab: u64, name: String },
    NewFile { tab: u64, name: String },
    Chmod { tab: u64, path: String, name: String, mode: String },
    Keys { selected: Option<usize> },
    KeyGen {
        kind: keys::KeyType,
        name: String,
        comment: String,
        pass1: String,
        pass2: String,
        busy: bool,
        result: Option<keys::GeneratedKey>,
        install_status: Option<String>,
    },
    ImportKey { name: String, content: String },
    Settings { page: SettingsPage, prefs: Prefs, lock1: String, lock2: String },
    Backup { export: bool, pass1: String, pass2: String, status: Option<String> },
    /// Passwort / Passphrase beim Verbinden abfragen
    Ask { tab: u64, passphrase: bool, value: String, remember: bool },
    Update,
}

pub enum Screen {
    Splash(Instant),
    Locked { input: String, error: bool },
    Main,
}

pub struct App {
    pub rt: tokio::runtime::Runtime,
    pub tx: mpsc::Sender<Msg>,
    rx: mpsc::Receiver<Msg>,
    pub ctx: egui::Context,

    pub screen: Screen,
    pub tabs: Vec<Tab>,
    pub active: Option<u64>,
    next_id: u64,

    pub form: Host,
    pub ports: Vec<String>,
    pub baud_label: String,
    pub device_search: String,

    pub hosts: Vec<Host>,
    pub keys: Vec<SshKey>,
    pub snippets: Vec<Snippet>,
    pub bauds: Vec<BaudProfile>,
    pub prefs: Prefs,

    pub show_files: bool,
    pub broadcast: bool,
    pub search: Option<String>,
    pub search_focus: bool,
    pub search_miss: bool,
    pub focus_term: bool,
    pub dialog: Option<Dialog>,
    pub transfers: Vec<Transfer>,
    pub pending_backup: Option<bool>,
    pub update: crate::updater::UpdateState,
    update_started: bool,
    pub toast: Option<(String, Instant, bool)>,
    last_activity: Instant,
    restore_pending: bool,
    last_title: String,

    pub logo: egui::TextureHandle,
}

pub fn alert(title: &str, text: &str) {
    MessageDialog::new()
        .set_level(MessageLevel::Error)
        .set_title(title)
        .set_description(text)
        .set_buttons(MessageButtons::Ok)
        .show();
}

pub fn confirm(title: &str, text: &str, level: MessageLevel) -> bool {
    MessageDialog::new()
        .set_level(level)
        .set_title(title)
        .set_description(text)
        .set_buttons(MessageButtons::YesNo)
        .show()
        == MessageDialogResult::Yes
}

pub fn human_size(n: u64) -> String {
    match n {
        0..=1023 => format!("{n} B"),
        1024..=1_048_575 => format!("{:.1} KB", n as f64 / 1024.0),
        1_048_576..=1_073_741_823 => format!("{:.1} MB", n as f64 / 1_048_576.0),
        _ => format!("{:.2} GB", n as f64 / 1_073_741_824.0),
    }
}

/// Entfernt Steuersequenzen für lesbare Log-Dateien.
fn strip_ansi(data: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(data.len());
    let mut i = 0;
    while i < data.len() {
        match data[i] {
            0x1b => {
                i += 1;
                if i < data.len() && data[i] == b'[' {
                    i += 1;
                    while i < data.len() && !(0x40..=0x7e).contains(&data[i]) {
                        i += 1;
                    }
                } else if i < data.len() && data[i] == b']' {
                    while i < data.len() && data[i] != 0x07 && data[i] != 0x1b {
                        i += 1;
                    }
                }
            }
            b'\r' | 0x07 | 0x08 => {}
            b => out.push(b),
        }
        i += 1;
    }
    out
}

impl App {
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .thread_name("nebula-net")
            .build()
            .expect("Tokio-Runtime");
        let (tx, rx) = mpsc::channel();
        nebula_core::update::cleanup_old();
        let settings = data::load_settings();
        let keys = data::load_keys();
        let prefs = settings.prefs;
        let form = Host {
            user: prefs.default_user.clone(),
            key_name: keys.first().map(|k| k.name.clone()).unwrap_or_default(),
            ..Default::default()
        };
        let img = image::load_from_memory(include_bytes!("../assets/256x256.png"))
            .expect("Logo")
            .into_rgba8();
        let logo = cc.egui_ctx.load_texture(
            "logo",
            egui::ColorImage::from_rgba_unmultiplied([256, 256], img.as_raw()),
            egui::TextureOptions::LINEAR,
        );
        let screen = if prefs.splash {
            Screen::Splash(Instant::now())
        } else if prefs.lock_hash.is_some() {
            Screen::Locked { input: String::new(), error: false }
        } else {
            Screen::Main
        };
        Self {
            rt,
            tx,
            rx,
            ctx: cc.egui_ctx.clone(),
            screen,
            tabs: Vec::new(),
            active: None,
            next_id: 1,
            form,
            ports: Vec::new(),
            baud_label: "115200".into(),
            device_search: String::new(),
            hosts: data::load_hosts(),
            keys,
            snippets: settings.snippets,
            bauds: settings.bauds,
            restore_pending: prefs.restore_tabs,
            prefs,
            show_files: false,
            broadcast: false,
            search: None,
            search_focus: false,
            search_miss: false,
            focus_term: false,
            dialog: None,
            transfers: Vec::new(),
            pending_backup: None,
            update: crate::updater::UpdateState::Idle,
            update_started: false,
            toast: None,
            last_activity: Instant::now(),
            last_title: String::new(),
            logo,
        }
    }

    pub fn notify(&mut self, text: impl Into<String>) {
        self.toast = Some((text.into(), Instant::now(), false));
    }

    pub fn notify_err(&mut self, text: impl Into<String>) {
        self.toast = Some((text.into(), Instant::now(), true));
    }

    pub fn save_settings(&self) {
        data::save_settings(&self.snippets, &self.bauds, &self.prefs);
    }

    pub fn tab(&mut self, id: u64) -> Option<&mut Tab> {
        self.tabs.iter_mut().find(|t| t.id == id)
    }

    pub fn active_tab(&mut self) -> Option<&mut Tab> {
        let id = self.active?;
        self.tab(id)
    }

    pub fn active_ssh(&self) -> Option<Arc<SshSession>> {
        let id = self.active?;
        self.tabs.iter().find(|t| t.id == id)?.ssh.clone()
    }

    pub fn mono(&self) -> FontId {
        FontId::new(self.prefs.font_size, egui::FontFamily::Monospace)
    }

    fn sink(&self, tab: u64, term: &Terminal, log: Option<Arc<Mutex<std::fs::File>>>) -> Sink {
        let core = term.core.clone();
        let tx = self.tx.clone();
        let ctx = self.ctx.clone();
        Arc::new(move |ev| {
            match ev {
                Event::Data(d) => {
                    if let Some(l) = &log {
                        let _ = l.lock().unwrap().write_all(&strip_ansi(&d));
                    }
                    core.lock().unwrap().feed(&d);
                }
                Event::Closed(msg) => {
                    core.lock()
                        .unwrap()
                        .feed(format!("\r\n\x1b[90m{msg}\x1b[0m\r\n").as_bytes());
                    let _ = tx.send(Msg::Closed(tab));
                }
            }
            ctx.request_repaint();
        })
    }

    fn open_log(&self, title: &str) -> Option<Arc<Mutex<std::fs::File>>> {
        if !self.prefs.log_sessions {
            return None;
        }
        let dir = if self.prefs.log_dir.is_empty() {
            nebula_core::storage::data_dir().join("logs")
        } else {
            self.prefs.log_dir.clone().into()
        };
        std::fs::create_dir_all(&dir).ok()?;
        let safe: String = title
            .chars()
            .map(|c| if c.is_alphanumeric() || "-_.@".contains(c) { c } else { '_' })
            .collect();
        let name = format!("{}_{}.log", chrono::Local::now().format("%Y-%m-%d_%H-%M-%S"), safe);
        std::fs::File::create(dir.join(name)).ok().map(|f| Arc::new(Mutex::new(f)))
    }

    // ------------------------------------------------------------------
    // Verbinden
    // ------------------------------------------------------------------

    /// Öffnet einen neuen Tab für ein Gerät und verbindet.
    pub fn open(&mut self, host: Host, secret: Option<String>) {
        if host.is_serial() {
            if host.port.is_empty() {
                alert("NebulaSSH", "Kein COM-Port gefunden oder ausgewählt!");
                return;
            }
        } else {
            if host.ip.trim().is_empty() || host.user.trim().is_empty() {
                alert("NebulaSSH", "Bitte IP/Host und Benutzername eingeben!");
                return;
            }
            if host.auth_method == "key" && !self.keys.iter().any(|k| k.name == host.key_name) {
                alert("NebulaSSH", "Bitte wähle einen gültigen SSH-Key aus.");
                return;
            }
        }
        // Statistik für gespeicherte Geräte
        if let Some(h) = self.hosts.iter_mut().find(|h| h.same_device(&host)) {
            h.last_used = data::now();
            h.use_count += 1;
            let _ = data::save_hosts(&self.hosts);
        }

        let id = self.next_id;
        self.next_id += 1;
        let size: Option<Size> = self.active_tab().map(|t| t.term.size);
        let title = host.display_name();
        let term = Terminal::new(self.prefs.scrollback, &self.prefs.cursor, size);
        let log = self.open_log(&title);
        let default_secret = if host.auth_method == "key" { String::new() } else { host.pass.clone() };
        self.tabs.push(Tab {
            id,
            title,
            secret: secret.unwrap_or(default_secret),
            host,
            term,
            conn: None,
            ssh: None,
            task: None,
            status: Status::Connecting,
            sftp: Sftp::default(),
            log,
            connected_at: None,
        });
        self.active = Some(id);
        self.focus_term = true;
        self.start(id);
    }

    /// Startet (oder wiederholt) den Verbindungsaufbau für einen Tab.
    pub fn start(&mut self, id: u64) {
        let Some(idx) = self.tabs.iter().position(|t| t.id == id) else { return };
        let host = self.tabs[idx].host.clone();

        // Fehlende Zugangsdaten zuerst abfragen
        if !host.is_serial() {
            let key = self.keys.iter().find(|k| k.name == host.key_name).map(|k| k.content.clone());
            let secret = self.tabs[idx].secret.clone();
            if host.auth_method == "key" {
                if let Some(k) = &key {
                    if keys::is_encrypted(k) && secret.is_empty() {
                        self.dialog = Some(Dialog::Ask { tab: id, passphrase: true, value: String::new(), remember: false });
                        return;
                    }
                }
            } else if secret.is_empty() {
                self.dialog = Some(Dialog::Ask { tab: id, passphrase: false, value: String::new(), remember: false });
                return;
            }
        }

        let log = self.tabs[idx].log.clone();
        let sink = self.sink(id, &self.tabs[idx].term, log);
        let tx = self.tx.clone();
        let ctx = self.ctx.clone();
        let keys_store = self.keys.clone();
        let opts = ssh::Options {
            keepalive_secs: self.prefs.keepalive,
            legacy_algorithms: self.prefs.legacy_algorithms,
            verify_new_host: self.prefs.verify_new_hosts.then(|| {
                let v: ssh::Verifier = Arc::new(|host: String, fp: String| {
                    MessageDialog::new()
                        .set_level(MessageLevel::Warning)
                        .set_title("Neuer Server – Host-Key prüfen")
                        .set_description(format!(
                            "Mit {host} wurde noch nie verbunden.\n\n\
                             Fingerprint des Servers:\n{fp}\n\n\
                             Stimmt dieser Fingerprint mit dem des Servers überein?\n\
                             (Auf dem Server: ssh-keygen -lf /etc/ssh/ssh_host_ed25519_key.pub)\n\n\
                             Dem Server vertrauen und verbinden?"
                        ))
                        .set_buttons(MessageButtons::YesNo)
                        .show()
                        == MessageDialogResult::Yes
                });
                v
            }),
        };
        let tab = &mut self.tabs[idx];
        tab.status = Status::Connecting;
        if host.is_serial() {
            let (port, baud) = (host.port.clone(), host.baud);
            tab.term.print(&format!("\x1b[1;32mÖffne Serial Port {port} mit {baud} Baud...\x1b[0m\n"));
            self.rt.spawn_blocking(move || {
                let msg = match serial::connect(&port, baud, sink) {
                    Ok(s) => Msg::SerialConnected(id, s),
                    Err(e) => Msg::Failed(id, e),
                };
                let _ = tx.send(msg);
                ctx.request_repaint();
            });
        } else {
            tab.term.print(&format!("\x1b[1;32mBaue SSH Verbindung zu {} auf...\x1b[0m\n", host.ip));
            let key = if host.auth_method == "key" {
                keys_store.iter().find(|k| k.name == host.key_name).map(|k| k.content.clone()).unwrap_or_default()
            } else {
                String::new()
            };
            let secret = tab.secret.clone();
            let (cols, rows) = (tab.term.size.cols as u32, tab.term.size.rows as u32);
            let handle = self.rt.spawn(async move {
                let msg = match ssh::connect(host.ip, host.user, secret, key, cols, rows, opts, sink).await {
                    Ok(s) => Msg::SshConnected(id, s),
                    Err(e) => Msg::Failed(id, e),
                };
                let _ = tx.send(msg);
                ctx.request_repaint();
            });
            tab.task = Some(handle.abort_handle());
        }
    }

    pub fn reconnect(&mut self, id: u64) {
        if let Some(t) = self.tab(id) {
            t.shutdown();
            t.term.print("\n");
        }
        self.start(id);
        self.focus_term = true;
    }

    pub fn duplicate(&mut self, id: u64) {
        if let Some(t) = self.tabs.iter().find(|t| t.id == id) {
            let (h, s) = (t.host.clone(), t.secret.clone());
            self.open(h, Some(s));
        }
    }

    pub fn close_tab(&mut self, id: u64, ask: bool) {
        let Some(idx) = self.tabs.iter().position(|t| t.id == id) else { return };
        if ask
            && self.prefs.confirm_close
            && self.tabs[idx].status == Status::Connected
            && !confirm(
                "Tab schließen",
                &format!("Die Verbindung „{}“ ist noch aktiv. Trennen und schließen?", self.tabs[idx].title),
                MessageLevel::Info,
            )
        {
            return;
        }
        let mut tab = self.tabs.remove(idx);
        tab.shutdown();
        if self.active == Some(id) {
            self.active = if self.tabs.is_empty() {
                self.search = None;
                None
            } else {
                Some(self.tabs[idx.saturating_sub(1).min(self.tabs.len() - 1)].id)
            };
            self.focus_term = true;
        }
    }

    pub fn switch_tab(&mut self, id: u64) {
        self.active = Some(id);
        self.focus_term = true;
        let need_files = self.show_files
            && self.tabs.iter().any(|t| t.id == id && t.ssh.is_some() && !t.sftp.initialized);
        if need_files {
            self.sftp_init(id);
        }
    }

    fn cycle_tab(&mut self, dir: i32) {
        if self.tabs.is_empty() {
            return;
        }
        let idx = self.active.and_then(|a| self.tabs.iter().position(|t| t.id == a)).unwrap_or(0) as i32;
        let n = self.tabs.len() as i32;
        let next = ((idx + dir) % n + n) % n;
        let id = self.tabs[next as usize].id;
        self.switch_tab(id);
    }

    fn handle_msgs(&mut self) {
        while let Ok(msg) = self.rx.try_recv() {
            match msg {
                Msg::SshConnected(id, s) => {
                    let files = self.show_files && self.active == Some(id);
                    match self.tab(id) {
                        Some(t) => {
                            t.task = None;
                            t.status = Status::Connected;
                            t.connected_at = Some(Instant::now());
                            let size = t.term.size;
                            s.send(Cmd::Resize(size.cols as u32, size.rows as u32));
                            t.conn = Some(s.clone());
                            t.ssh = Some(s);
                            if files {
                                self.sftp_init(id);
                            }
                        }
                        None => s.close(),
                    }
                }
                Msg::SerialConnected(id, s) => match self.tab(id) {
                    Some(t) => {
                        t.status = Status::Connected;
                        t.connected_at = Some(Instant::now());
                        t.conn = Some(s);
                    }
                    None => s.close(),
                },
                Msg::Failed(id, err) => self.on_failed(id, err),
                Msg::Closed(id) => {
                    if let Some(t) = self.tab(id) {
                        t.status = Status::Closed;
                        t.conn = None;
                        t.ssh = None;
                    }
                }
                Msg::SftpList(id, path, res) => self.on_list(id, path, res),
                Msg::SftpHome(id, res) => {
                    let path = res.unwrap_or_else(|_| "/".into());
                    if let Some(t) = self.tab(id) {
                        t.sftp.path = path.clone();
                    }
                    self.load_dir(id, path, false);
                }
                Msg::SftpDone(id, res, what) => match res {
                    Ok(()) => {
                        self.notify(format!("{what} erfolgreich"));
                        self.reload_dir(id);
                    }
                    Err(e) => {
                        self.reload_dir(id);
                        alert("Dateien", &format!("{what} fehlgeschlagen:\n{e}"));
                    }
                },
                Msg::FileLoaded(id, path, res) => {
                    let mut err = None;
                    if let Some(Dialog::Editor { tab, path: p, content, original, loading, .. }) = &mut self.dialog {
                        if *tab == id && *p == path {
                            *loading = false;
                            match res {
                                Ok(c) => {
                                    *content = c.clone();
                                    *original = c;
                                }
                                Err(e) => err = Some(e),
                            }
                        }
                    }
                    if let Some(e) = err {
                        self.dialog = None;
                        alert("Editor", &format!("Konnte Datei nicht öffnen:\n{e}"));
                    }
                }
                Msg::FileSaved(id, res) => match res {
                    Ok(()) => {
                        if let Some(Dialog::Editor { saving, content, original, .. }) = &mut self.dialog {
                            *saving = false;
                            *original = content.clone();
                        }
                        self.notify("Datei gespeichert");
                        self.reload_dir(id);
                    }
                    Err(e) => {
                        if let Some(Dialog::Editor { saving, .. }) = &mut self.dialog {
                            *saving = false;
                        }
                        alert("Editor", &format!("Fehler beim Speichern:\n{e}"));
                    }
                },
                Msg::TransferDone(tid, tab, res) => {
                    let ok = res.is_ok();
                    if let Some(t) = self.transfers.iter_mut().find(|t| t.id == tid) {
                        t.state = match res {
                            Ok(()) => TransferState::Done(Instant::now()),
                            Err(e) => TransferState::Failed(e),
                        };
                    }
                    if ok {
                        self.reload_dir(tab);
                    }
                }
                Msg::KeyGenerated(res) => {
                    if let Some(Dialog::KeyGen { busy, result, name, .. }) = &mut self.dialog {
                        *busy = false;
                        match res {
                            Ok(k) => {
                                let n = name.trim().to_string();
                                self.keys.retain(|x| x.name != n);
                                self.keys.push(SshKey { name: n.clone(), content: k.private_openssh.clone() });
                                if let Err(e) = data::save_keys(&self.keys) {
                                    alert("SSH-Key", &e);
                                }
                                self.form.key_name = n;
                                *result = Some(k);
                            }
                            Err(e) => alert("SSH-Key", &e),
                        }
                    }
                }
                Msg::UpdateChecked(res, manual) => self.on_update_checked(res, manual),
                Msg::UpdateDownloaded(res, installed) => self.on_update_downloaded(res, installed),
                Msg::KeyInstalled(res) => {
                    let text = match res {
                        Ok(true) => "✔ Public Key wurde auf dem Server hinterlegt.".to_string(),
                        Ok(false) => "✔ Dieser Key war auf dem Server bereits hinterlegt.".to_string(),
                        Err(e) => format!("✖ Fehler: {e}"),
                    };
                    if let Some(Dialog::KeyGen { install_status, .. }) = &mut self.dialog {
                        *install_status = Some(text.clone());
                    }
                    self.notify(text);
                }
            }
        }
    }

    fn on_failed(&mut self, id: u64, err: String) {
        let Some(t) = self.tab(id) else { return };
        t.task = None;
        t.status = Status::Closed;
        if let Some(rest) = err.strip_prefix(ssh::HOSTKEY_CHANGED) {
            let (host_id, change) = rest.split_once('|').unwrap_or((rest, ""));
            t.term.print(&format!(
                "\x1b[1;31m[WARNUNG] Der Host-Key von {host_id} hat sich geändert!\x1b[0m\n\x1b[33m{change}\x1b[0m\n"
            ));
            let ok = confirm(
                "Host-Key geändert",
                &format!(
                    "ACHTUNG: Der Host-Key von {host_id} hat sich geändert!\n\n\
                     Das kann eine Neuinstallation des Servers sein – oder ein Man-in-the-Middle-Angriff.\n\n\
                     Neuen Key vertrauen und erneut verbinden?"
                ),
                MessageLevel::Warning,
            );
            if ok {
                ssh::forget_host_key(host_id);
                self.start(id);
            }
        } else {
            // Falsche Zugangsdaten nicht weiterverwenden
            if err.contains("Passwort falsch") || err.contains("Passphrase") {
                t.secret.clear();
                if !t.host.pass.is_empty() && err.contains("Passwort falsch") {
                    t.host.pass.clear();
                }
            }
            t.term.print(&format!("\x1b[1;31m[Fehler] {err}\x1b[0m\n"));
        }
    }

    // ------------------------------------------------------------------
    // Tastenkürzel
    // ------------------------------------------------------------------

    fn shortcuts(&mut self, ui: &mut Ui) {
        if self.dialog.is_some() {
            return;
        }
        let cs = Modifiers::CTRL | Modifiers::SHIFT;
        let (f, w, t, d, r, tab, stab, plus, minus, zero, b, e) = ui.input_mut(|i| {
            (
                i.consume_key(Modifiers::CTRL, Key::F),
                i.consume_key(cs, Key::W),
                i.consume_key(cs, Key::T),
                i.consume_key(cs, Key::D),
                i.consume_key(cs, Key::R),
                i.consume_key(Modifiers::CTRL, Key::Tab),
                i.consume_key(cs, Key::Tab),
                i.consume_key(Modifiers::CTRL, Key::Plus) || i.consume_key(Modifiers::CTRL, Key::Equals),
                i.consume_key(Modifiers::CTRL, Key::Minus),
                i.consume_key(Modifiers::CTRL, Key::Num0),
                i.consume_key(cs, Key::B),
                i.consume_key(cs, Key::E),
            )
        });
        if f && self.active.is_some() {
            self.search.get_or_insert_with(String::new);
            self.search_focus = true;
        }
        if w {
            if let Some(id) = self.active {
                self.close_tab(id, true);
            }
        }
        if t {
            self.focus_quick_connect();
        }
        if d {
            if let Some(id) = self.active {
                self.duplicate(id);
            }
        }
        if r {
            if let Some(id) = self.active {
                self.reconnect(id);
            }
        }
        if tab {
            self.cycle_tab(1);
        }
        if stab {
            self.cycle_tab(-1);
        }
        if plus || minus || zero {
            self.prefs.font_size = if zero {
                14.0
            } else {
                (self.prefs.font_size + if plus { 1.0 } else { -1.0 }).clamp(9.0, 28.0)
            };
            self.save_settings();
            self.notify(format!("Schriftgröße {:.0}", self.prefs.font_size));
        }
        if b {
            self.prefs.sidebar_open = !self.prefs.sidebar_open;
            self.save_settings();
        }
        if e && self.active_ssh().is_some() {
            self.toggle_files();
        }
    }

    pub fn toggle_files(&mut self) {
        self.show_files = !self.show_files;
        if self.show_files {
            if let Some(id) = self.active {
                let init = self.tabs.iter().any(|t| t.id == id && !t.sftp.initialized);
                if init {
                    self.sftp_init(id);
                }
            }
        } else {
            self.focus_term = true;
        }
    }

    pub fn run_snippet(&mut self, cmd: String) {
        let broadcast = self.broadcast;
        let targets: Vec<u64> = if broadcast {
            self.tabs.iter().filter(|t| t.conn.is_some()).map(|t| t.id).collect()
        } else {
            self.active.into_iter().collect()
        };
        let mut sent = false;
        for id in targets {
            if let Some(t) = self.tab(id) {
                if t.conn.is_some() {
                    t.send(Cmd::Data(cmd.clone().into_bytes()));
                    sent = true;
                }
            }
        }
        if sent {
            self.focus_term = true;
        } else {
            alert("Makro", "Bitte öffne zuerst einen verbundenen Terminal-Tab, um das Makro auszuführen.");
        }
    }

    // ------------------------------------------------------------------
    // Startbildschirm & Sperre
    // ------------------------------------------------------------------

    fn splash(&mut self, ui: &mut Ui, started: Instant) {
        let t = started.elapsed().as_secs_f32();
        let total = 2.2;
        let rect = ui.max_rect();
        let p = ui.painter();
        // Hintergrund mit sanftem Verlauf
        p.rect_filled(rect, 0.0, Color32::from_rgb(0x0b, 0x0a, 0x14));
        let glow_c = rect.center() + vec2(0.0, -40.0);
        for i in 0..24 {
            let r = 420.0 - i as f32 * 16.0;
            let a = (6.0 + i as f32 * 0.6) as u8;
            p.circle_filled(glow_c, r, Color32::from_rgba_unmultiplied(98, 0, 238, a));
        }
        let fade = (t / 0.5).min(1.0);
        let ease = 1.0 - (1.0 - fade).powi(3);
        let size = 150.0 + 12.0 * ease;
        let logo_rect = egui::Rect::from_center_size(rect.center() + vec2(0.0, -70.0 + (1.0 - ease) * 20.0), vec2(size, size));
        p.image(
            self.logo.id(),
            logo_rect,
            egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)),
            Color32::from_white_alpha((255.0 * ease) as u8),
        );
        let a = |v: f32| Color32::from_white_alpha((255.0 * v) as u8);
        let ta = ((t - 0.3) / 0.5).clamp(0.0, 1.0);
        p.text(
            rect.center() + vec2(-2.0, 40.0),
            egui::Align2::RIGHT_CENTER,
            "Nebula",
            FontId::new(40.0, egui::FontFamily::Name("ui-bold".into())),
            a(ta),
        );
        p.text(
            rect.center() + vec2(-2.0, 40.0),
            egui::Align2::LEFT_CENTER,
            "SSH",
            FontId::new(40.0, egui::FontFamily::Name("ui-bold".into())),
            Color32::from_rgba_unmultiplied(0xb9, 0x9c, 0xff, (255.0 * ta) as u8),
        );
        p.text(
            rect.center() + vec2(0.0, 82.0),
            egui::Align2::CENTER_CENTER,
            format!("SSH · SFTP · Serial   —   Version {VERSION}"),
            FontId::proportional(15.0),
            Color32::from_rgba_unmultiplied(0xa7, 0xa3, 0xc2, (255.0 * ta) as u8),
        );
        // Ladebalken
        let bar = egui::Rect::from_center_size(rect.center() + vec2(0.0, 130.0), vec2(220.0, 4.0));
        p.rect_filled(bar, 2.0, Color32::from_rgba_unmultiplied(255, 255, 255, 20));
        let prog = (t / (total - 0.2)).clamp(0.0, 1.0);
        let mut fill = bar;
        fill.set_width(bar.width() * prog);
        p.rect_filled(fill, 2.0, AMBER);
        p.text(
            egui::pos2(rect.center().x, rect.bottom() - 40.0),
            egui::Align2::CENTER_CENTER,
            "by Thomas Schöpf",
            FontId::proportional(14.0),
            Color32::from_rgba_unmultiplied(0x8e, 0x8a, 0xab, (255.0 * ta) as u8),
        );
        let skip = ui.input(|i| i.pointer.any_click() || i.key_pressed(Key::Escape) || i.key_pressed(Key::Enter));
        if t >= total || skip {
            self.screen = if self.prefs.lock_hash.is_some() {
                Screen::Locked { input: String::new(), error: false }
            } else {
                Screen::Main
            };
        } else {
            ui.ctx().request_repaint();
        }
    }

    fn lock_screen(&mut self, ui: &mut Ui) {
        let rect = ui.max_rect();
        ui.painter().rect_filled(rect, 0.0, Color32::from_rgb(0x0b, 0x0a, 0x14));
        let mut unlocked = false;
        if let Screen::Locked { input, error } = &mut self.screen {
            let hash = self.prefs.lock_hash.clone().unwrap_or_default();
            ui.scope_builder(
                egui::UiBuilder::new().max_rect(egui::Rect::from_center_size(rect.center(), vec2(340.0, 360.0))),
                |ui| {
                    ui.vertical_centered(|ui| {
                        ui.add(egui::Image::new(&self.logo).fit_to_exact_size(vec2(96.0, 96.0)));
                        ui.add_space(14.0);
                        ui.label(RichText::new("NebulaSSH ist gesperrt").size(22.0).strong().color(Color32::WHITE));
                        ui.add_space(4.0);
                        ui.label(RichText::new("Gib dein App-Passwort ein, um fortzufahren.").color(MUTED));
                        ui.add_space(18.0);
                        let r = ui.add(
                            egui::TextEdit::singleline(input)
                                .password(true)
                                .hint_text("App-Passwort")
                                .desired_width(280.0)
                                .min_size(vec2(280.0, 34.0)),
                        );
                        if !r.has_focus() {
                            r.request_focus();
                        }
                        let enter = (r.lost_focus() || r.has_focus()) && ui.input(|i| i.key_pressed(Key::Enter));
                        ui.add_space(10.0);
                        let go = ui
                            .add(egui::Button::new(RichText::new("Entsperren").strong().color(Color32::WHITE)).fill(ACCENT).min_size(vec2(280.0, 36.0)))
                            .clicked();
                        if enter || go {
                            if nebula_core::backup::verify_password(input, &hash) {
                                unlocked = true;
                            } else {
                                *error = true;
                                input.clear();
                            }
                        }
                        if *error {
                            ui.add_space(8.0);
                            ui.label(RichText::new("Falsches Passwort.").color(RED));
                        }
                    });
                },
            );
        }
        if unlocked {
            self.screen = Screen::Main;
            self.last_activity = Instant::now();
            self.focus_term = true;
        }
    }

    pub fn lock_now(&mut self) {
        if self.prefs.lock_hash.is_some() {
            self.dialog = None;
            self.screen = Screen::Locked { input: String::new(), error: false };
        }
    }

    // ------------------------------------------------------------------
    // Hauptlayout
    // ------------------------------------------------------------------

    fn main_ui(&mut self, ui: &mut Ui) {
        self.shortcuts(ui);

        if self.prefs.sidebar_open {
            egui::Panel::left("sidebar")
                .default_size(300.0)
                .size_range(260.0..=420.0)
                .resizable(true)
                .frame(egui::Frame::new().fill(PANEL).inner_margin(Margin::symmetric(16, 0)).stroke(Stroke::new(1.0, LINE)))
                .show(ui, |ui| self.sidebar(ui));
        }

        egui::Panel::bottom("statusbar")
            .exact_size(44.0)
            .frame(egui::Frame::new().fill(PANEL).stroke(Stroke::new(1.0, LINE)).inner_margin(Margin::symmetric(10, 0)))
            .show(ui, |ui| self.status_bar(ui));

        egui::Panel::top("tabs")
            .exact_size(42.0)
            .frame(egui::Frame::new().fill(PANEL).stroke(Stroke::new(1.0, LINE)))
            .show(ui, |ui| self.tab_bar(ui));

        let files_tab = self
            .active
            .filter(|id| self.show_files && self.tabs.iter().any(|t| t.id == *id && t.is_ssh()));
        if let Some(id) = files_tab {
            egui::Panel::right("files")
                .default_size(420.0)
                .size_range(300.0..=900.0)
                .resizable(true)
                .frame(egui::Frame::new().fill(PANEL).inner_margin(Margin::symmetric(12, 6)).stroke(Stroke::new(1.0, LINE)))
                .show(ui, |ui| self.files_panel(ui, id));
        }

        egui::CentralPanel::no_frame().show(ui, |ui| self.terminal_area(ui));

        self.drop_files(ui);
    }

    fn terminal_area(&mut self, ui: &mut Ui) {
        let rect = ui.max_rect();
        let th = theme::by_id(&self.prefs.theme);
        ui.painter().rect_filled(rect, 0.0, th.bg);
        let view = View {
            font: self.mono(),
            theme: th,
            copy_on_select: self.prefs.copy_on_select,
            right_click_paste: self.prefs.right_click_paste,
        };
        let focus = std::mem::take(&mut self.focus_term) && self.dialog.is_none() && self.search.is_none();
        let broadcast = self.broadcast;
        let mut bc: Vec<Vec<u8>> = Vec::new();
        let mut reconnect = None;
        let mut close = None;
        match self.active.and_then(|id| self.tabs.iter_mut().find(|t| t.id == id)) {
            Some(tab) => {
                let tid = egui::Id::new(("term", tab.id));
                if focus {
                    ui.memory_mut(|m| m.request_focus(tid));
                }
                for o in tab.term.show(ui, tid, &view) {
                    match o {
                        Output::Input(b) => {
                            if tab.status == Status::Closed && b == b"\r" {
                                reconnect = Some(tab.id);
                            } else {
                                if broadcast {
                                    bc.push(b.clone());
                                }
                                tab.send(Cmd::Data(b));
                            }
                        }
                        Output::Reply(b) => tab.send(Cmd::Data(b)),
                        Output::Resize(s) => tab.send(Cmd::Resize(s.cols as u32, s.rows as u32)),
                    }
                }
                // Hinweis bei getrennter Verbindung
                if tab.status == Status::Closed {
                    let bar = egui::Rect::from_min_size(
                        egui::pos2(rect.left() + 14.0, rect.bottom() - 58.0),
                        vec2(rect.width() - 28.0, 44.0),
                    );
                    ui.scope_builder(egui::UiBuilder::new().max_rect(bar), |ui| {
                        egui::Frame::new()
                            .fill(Color32::from_rgb(0x2a, 0x16, 0x24))
                            .stroke(Stroke::new(1.0, RED.gamma_multiply(0.6)))
                            .corner_radius(8)
                            .inner_margin(Margin::symmetric(12, 6))
                            .show(ui, |ui| {
                                ui.horizontal_centered(|ui| {
                                    ui.label(RichText::new("⚠ Verbindung getrennt").color(Color32::WHITE).strong());
                                    ui.label(RichText::new("– Enter drückt „Neu verbinden“").color(MUTED));
                                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                        if ui.button("Tab schließen").clicked() {
                                            close = Some(tab.id);
                                        }
                                        if ui.add(egui::Button::new(RichText::new("⟳ Neu verbinden").color(Color32::WHITE)).fill(ACCENT)).clicked() {
                                            reconnect = Some(tab.id);
                                        }
                                    });
                                });
                            });
                    });
                }
            }
            None => self.welcome(ui, rect),
        }
        if !bc.is_empty() {
            let active = self.active;
            for t in self.tabs.iter().filter(|t| Some(t.id) != active && t.conn.is_some()) {
                for b in &bc {
                    t.send(Cmd::Data(b.clone()));
                }
            }
        }
        if let Some(id) = reconnect {
            self.reconnect(id);
        }
        if let Some(id) = close {
            self.close_tab(id, false);
        }
        if self.search.is_some() {
            self.search_bar(ui, rect);
        }
    }

    fn welcome(&mut self, ui: &mut Ui, rect: egui::Rect) {
        let mut action: Option<Host> = None;
        let mut open_keys = false;
        let mut quick = false;
        let mut restore = false;
        ui.scope_builder(
            egui::UiBuilder::new().max_rect(egui::Rect::from_center_size(rect.center(), vec2(600.0_f32.min(rect.width() - 40.0), 560.0))),
            |ui| {
                ui.vertical_centered(|ui| {
                    ui.add(egui::Image::new(&self.logo).fit_to_exact_size(vec2(84.0, 84.0)));
                    ui.add_space(10.0);
                    ui.label(RichText::new("Willkommen bei NebulaSSH").size(24.0).strong().color(Color32::WHITE));
                    ui.add_space(4.0);
                    ui.label(RichText::new("Verbinde dich links über „Neue Verbindung“ oder wähle ein gespeichertes Gerät.").color(MUTED));
                    ui.add_space(22.0);
                });
                // Schnellaktionen als Kacheln
                ui.horizontal(|ui| {
                    let w = (ui.available_width() - 2.0 * 10.0) / 3.0;
                    ui.spacing_mut().item_spacing.x = 10.0;
                    let mut tile = |ui: &mut Ui, icon: &str, title: &str, sub: &str, color: Color32| -> bool {
                        let r = egui::Frame::new()
                            .fill(PANEL2)
                            .corner_radius(12)
                            .inner_margin(Margin::same(14))
                            .show(ui, |ui| {
                                ui.set_width(w - 28.0);
                                ui.vertical(|ui| {
                                    ui.spacing_mut().item_spacing.y = 2.0;
                                    ui.label(RichText::new(icon).size(22.0).color(color));
                                    ui.add_space(6.0);
                                    ui.label(RichText::new(title).strong().color(Color32::WHITE));
                                    ui.label(RichText::new(sub).size(11.5).color(MUTED));
                                });
                            })
                            .response
                            .interact(egui::Sense::click());
                        if r.hovered() {
                            ui.painter().rect_stroke(r.rect, 12.0, Stroke::new(1.0, color), egui::StrokeKind::Inside);
                            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                        }
                        r.clicked()
                    };
                    if tile(ui, "⚡", "Neue Verbindung", "SSH oder COM-Port", ACCENT_HI) {
                        quick = true;
                    }
                    if tile(ui, "🔑", "SSH-Key erzeugen", "mit Anleitung", LAVENDER) {
                        open_keys = true;
                    }
                    if tile(ui, "♻", "Backup laden", "vom alten PC", AMBER) {
                        restore = true;
                    }
                });
                ui.add_space(22.0);
                // Zuletzt verwendet
                let mut recent: Vec<&Host> = self.hosts.iter().filter(|h| h.last_used > 0).collect();
                recent.sort_by(|a, b| b.last_used.cmp(&a.last_used));
                if !recent.is_empty() {
                    ui.label(RichText::new("Zuletzt verwendet").color(MUTED).strong());
                    ui.add_space(6.0);
                    for h in recent.into_iter().take(5) {
                        let r = egui::Frame::new()
                            .fill(PANEL2)
                            .corner_radius(8)
                            .inner_margin(Margin::symmetric(12, 8))
                            .show(ui, |ui| {
                                ui.set_width(ui.available_width());
                                ui.horizontal(|ui| {
                                    crate::sidebar::tile(ui, h.is_serial(), 34.0, false);
                                    ui.add_space(4.0);
                                    ui.vertical(|ui| {
                                        ui.label(RichText::new(h.display_name()).strong().color(Color32::WHITE));
                                        ui.label(RichText::new(h.subtitle()).size(12.0).color(MUTED));
                                    });
                                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                        ui.label(RichText::new(data::ago(h.last_used)).size(12.0).color(MUTED));
                                    });
                                });
                            })
                            .response
                            .interact(egui::Sense::click());
                        if r.hovered() {
                            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                        }
                        if r.clicked() {
                            action = Some(h.clone());
                        }
                        ui.add_space(4.0);
                    }
                    ui.add_space(14.0);
                }
                ui.horizontal_wrapped(|ui| {
                    let tips = [
                        ("Strg+Umschalt+T", "neue Verbindung"),
                        ("Strg+Tab", "Tab wechseln"),
                        ("Strg+F", "im Terminal suchen"),
                        ("Strg+Umschalt+E", "Dateien"),
                        ("Strg + / −", "Schriftgröße"),
                    ];
                    for (k, v) in tips {
                        ui.label(RichText::new(k).monospace().color(LAVENDER).size(12.0));
                        ui.label(RichText::new(v).size(12.0).color(MUTED));
                        ui.add_space(8.0);
                    }
                });

            },
        );
        if let Some(h) = action {
            self.open(h, None);
        }
        if open_keys {
            self.open_keygen();
        }
        if quick {
            self.focus_quick_connect();
        }
        if restore {
            self.dialog = Some(Dialog::Backup { export: false, pass1: String::new(), pass2: String::new(), status: None });
        }
    }

    fn search_bar(&mut self, ui: &mut Ui, rect: egui::Rect) {
        let Some(mut text) = self.search.take() else { return };
        let mut close = false;
        let mut find: Option<bool> = None;
        egui::Area::new(egui::Id::new("search"))
            .fixed_pos(egui::pos2(rect.right() - 360.0, rect.top() + 12.0))
            .order(egui::Order::Foreground)
            .show(ui.ctx(), |ui| {
                egui::Frame::new()
                    .fill(Color32::from_rgb(0x1a, 0x1a, 0x26))
                    .stroke(Stroke::new(1.0, if self.search_miss { RED } else { Color32::from_rgb(0x3a, 0x3a, 0x4e) }))
                    .corner_radius(8)
                    .inner_margin(6)
                    .shadow(egui::Shadow { offset: [0, 4], blur: 16, spread: 0, color: Color32::from_black_alpha(120) })
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.label("🔍");
                            let r = ui.add(egui::TextEdit::singleline(&mut text).hint_text("Im Terminal suchen…").desired_width(200.0));
                            if self.search_focus {
                                r.request_focus();
                                self.search_focus = false;
                            }
                            if r.changed() {
                                find = Some(false);
                            }
                            if (r.lost_focus() || r.has_focus()) && ui.input(|i| i.key_pressed(Key::Enter)) {
                                find = Some(ui.input(|i| i.modifiers.shift));
                                r.request_focus();
                            }
                            if ui.input(|i| i.key_pressed(Key::Escape)) {
                                close = true;
                            }
                            if ui.small_button("⬆").on_hover_text("Vorheriger (Umschalt+Enter)").clicked() {
                                find = Some(true);
                            }
                            if ui.small_button("⬇").on_hover_text("Nächster (Enter)").clicked() {
                                find = Some(false);
                            }
                            if ui.small_button(RichText::new("✖").color(RED)).on_hover_text("Schließen (Esc)").clicked() {
                                close = true;
                            }
                        });
                    });
            });
        if let Some(back) = find {
            let found = self.active_tab().map(|t| t.term.find(&text, back)).unwrap_or(false);
            self.search_miss = !found && !text.is_empty();
        }
        if close {
            if let Some(t) = self.active_tab() {
                t.term.core.lock().unwrap().term.selection = None;
            }
            self.focus_term = true;
        } else {
            self.search = Some(text);
        }
    }

    fn toast_ui(&mut self, ctx: &egui::Context) {
        let Some((text, at, err)) = self.toast.clone() else { return };
        let age = at.elapsed().as_secs_f32();
        if age > 3.5 {
            self.toast = None;
            return;
        }
        let alpha = if age > 3.0 { 1.0 - (age - 3.0) / 0.5 } else { 1.0 };
        let screen = ctx.content_rect();
        egui::Area::new(egui::Id::new("toast"))
            .order(egui::Order::Tooltip)
            .anchor(egui::Align2::RIGHT_BOTTOM, vec2(-18.0, -58.0))
            .interactable(false)
            .show(ctx, |ui| {
                ui.set_max_width(screen.width() * 0.6);
                egui::Frame::new()
                    .fill(if err { Color32::from_rgb(0x3a, 0x14, 0x22) } else { Color32::from_rgb(0x22, 0x1a, 0x3d) })
                    .stroke(Stroke::new(1.0, if err { RED } else { ACCENT_HI }))
                    .corner_radius(10)
                    .inner_margin(Margin::symmetric(14, 10))
                    .shadow(egui::Shadow { offset: [0, 6], blur: 24, spread: 0, color: Color32::from_black_alpha(140) })
                    .multiply_with_opacity(alpha)
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.label(RichText::new(if err { "⚠" } else { "✔" }).color(if err { RED } else { GREEN }).strong());
                            ui.label(RichText::new(text).color(Color32::WHITE));
                        });
                    });
            });
        ctx.request_repaint_after(Duration::from_millis(100));
    }

    fn restore_sessions(&mut self) {
        self.restore_pending = false;
        for h in data::load_sessions() {
            let secret = if h.auth_method == "key" { String::new() } else { h.pass.clone() };
            self.open(h, Some(secret));
        }
    }

    fn update_title(&mut self, ctx: &egui::Context) {
        let title = match self.active.and_then(|id| self.tabs.iter().find(|t| t.id == id)) {
            Some(t) => format!("{} – NebulaSSH", t.term.title.clone().unwrap_or_else(|| t.title.clone())),
            None => "NebulaSSH".into(),
        };
        if title != self.last_title {
            ctx.send_viewport_cmd(egui::ViewportCommand::Title(title.clone()));
            self.last_title = title;
        }
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut Ui, _frame: &mut eframe::Frame) {
        self.handle_msgs();
        let ctx = ui.ctx().clone();

        // Aktivität für die automatische Sperre
        if ui.input(|i| !i.events.is_empty() || i.pointer.is_moving()) {
            self.last_activity = Instant::now();
        }
        if self.prefs.lock_hash.is_some() && self.prefs.autolock_minutes > 0 && matches!(self.screen, Screen::Main) {
            if self.last_activity.elapsed() > Duration::from_secs(self.prefs.autolock_minutes * 60) {
                self.lock_now();
            } else {
                ctx.request_repaint_after(Duration::from_secs(15));
            }
        }

        match &self.screen {
            Screen::Splash(t) => {
                let t = *t;
                self.splash(ui, t);
                return;
            }
            Screen::Locked { .. } => {
                self.lock_screen(ui);
                return;
            }
            Screen::Main => {}
        }
        if self.restore_pending {
            self.restore_sessions();
        }
        if !self.update_started {
            self.update_started = true;
            if self.prefs.update_check {
                self.check_updates(false);
            }
        }

        self.main_ui(ui);
        self.dialogs(&ctx);
        self.toast_ui(&ctx);
        self.update_title(&ctx);

        // Laufende Übertragungen regelmäßig neu zeichnen
        if self.transfers.iter().any(|t| matches!(t.state, TransferState::Running)) {
            ctx.request_repaint_after(Duration::from_millis(150));
        }
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        if self.prefs.restore_tabs {
            let hosts: Vec<Host> = self
                .tabs
                .iter()
                .map(|t| {
                    let mut h = t.host.clone();
                    if h.auth_method != "key" {
                        h.pass = t.secret.clone();
                    }
                    h
                })
                .collect();
            data::save_sessions(&hosts);
        } else {
            data::save_sessions(&[]);
        }
        for t in &mut self.tabs {
            t.shutdown();
        }
    }
}
