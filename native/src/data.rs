//! Gespeicherte Geräte, SSH-Keys und Einstellungen.
//! Dateiformat ist kompatibel zu NebulaSSH 1.x und 2.x (neue Felder sind optional).

use nebula_core::ssh::Forward;
use nebula_core::{storage, t, tf};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct Host {
    /// "ssh", "telnet" oder "serial"
    #[serde(rename = "type")]
    pub kind: String,
    pub name: String,
    pub ip: String,
    pub user: String,
    pub pass: String,
    /// "password", "key" oder "agent"
    pub auth_method: String,
    pub key_name: String,
    /// Serial: COM-Port und Baudrate
    pub port: String,
    pub baud: u32,
    // --- neu in 2.1 ---
    pub group: String,
    pub favorite: bool,
    pub notes: String,
    /// Unix-Zeit der letzten Verbindung
    pub last_used: u64,
    pub use_count: u32,
    /// Passwort nicht speichern, sondern beim Verbinden abfragen
    pub ask_password: bool,
    // --- neu in 2.3 ---
    /// Name eines anderen gespeicherten Geräts, über das verbunden wird (Jump-Host)
    pub jump: String,
    pub forwards: Vec<Forward>,
    pub agent_forward: bool,
    pub x11: bool,
    /// MAC-Adresse für Wake-on-LAN
    pub mac: String,
}

impl Default for Host {
    fn default() -> Self {
        Self {
            kind: "ssh".into(),
            name: String::new(),
            ip: String::new(),
            user: "root".into(),
            pass: String::new(),
            auth_method: "password".into(),
            key_name: String::new(),
            port: String::new(),
            baud: 115200,
            group: String::new(),
            favorite: false,
            notes: String::new(),
            last_used: 0,
            use_count: 0,
            ask_password: false,
            jump: String::new(),
            forwards: Vec::new(),
            agent_forward: false,
            x11: false,
            mac: String::new(),
        }
    }
}

impl Host {
    pub fn is_serial(&self) -> bool {
        self.kind == "serial"
    }
    pub fn is_telnet(&self) -> bool {
        self.kind == "telnet"
    }
    pub fn is_ssh(&self) -> bool {
        !self.is_serial() && !self.is_telnet()
    }
    /// Gerät mit Netzwerkadresse (SSH oder Telnet)
    pub fn is_network(&self) -> bool {
        !self.is_serial()
    }
    pub fn display_name(&self) -> String {
        if !self.name.is_empty() {
            self.name.clone()
        } else if self.is_serial() {
            format!("{} ({})", self.port, self.baud)
        } else if self.is_telnet() || self.user.is_empty() {
            self.ip.clone()
        } else {
            format!("{}@{}", self.user, self.ip)
        }
    }
    pub fn subtitle(&self) -> String {
        if self.is_serial() {
            tf!("{} · {} Baud", "{} · {} baud", self.port, self.baud)
        } else if self.is_telnet() {
            format!("telnet://{}", self.ip)
        } else {
            format!("{}@{}", self.user, self.ip)
        }
    }
    /// Host-Teil ohne Port (für Ping / WoL-Anzeige)
    pub fn host_only(&self) -> String {
        nebula_core::ssh::split_host_port_or(&self.ip, self.default_port()).0
    }
    pub fn default_port(&self) -> u16 {
        if self.is_telnet() {
            23
        } else {
            22
        }
    }
    pub fn net_port(&self) -> u16 {
        nebula_core::ssh::split_host_port_or(&self.ip, self.default_port()).1
    }
    /// Identität für das Wiederfinden in der Liste
    pub fn same_device(&self, other: &Host) -> bool {
        if self.is_serial() {
            other.is_serial() && self.name == other.name && self.port == other.port
        } else {
            self.kind == other.kind && self.name == other.name && self.ip == other.ip && self.user == other.user
        }
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct SshKey {
    pub name: String,
    pub content: String,
}

/// Gespeicherter Befehl. Platzhalter: `{{name}}` wird vor dem Ausführen abgefragt,
/// `{{host}}`, `{{user}}` und `{{ip}}` werden automatisch gefüllt.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Snippet {
    pub name: String,
    pub cmd: String,
    #[serde(default, rename = "isDefault", skip_serializing)]
    pub is_default: bool,
    #[serde(default)]
    pub group: String,
    /// In der Statusleiste als Schnellknopf anzeigen
    #[serde(default = "yes")]
    pub pinned: bool,
}

fn yes() -> bool {
    true
}

impl Snippet {
    /// Platzhalter in der Reihenfolge ihres Auftretens (ohne Doppelte).
    pub fn placeholders(&self) -> Vec<String> {
        let mut out: Vec<String> = Vec::new();
        let mut rest = self.cmd.as_str();
        while let Some(start) = rest.find("{{") {
            let after = &rest[start + 2..];
            let Some(end) = after.find("}}") else { break };
            let name = after[..end].trim().to_string();
            // Nur einfache Namen – z.B. Go-Templates wie {{.Names}} bleiben unberührt
            let simple = name.chars().next().is_some_and(|c| c.is_alphabetic())
                && name.chars().all(|c| c.is_alphanumeric() || "_- ".contains(c));
            if simple && name.len() < 40 && !out.contains(&name) {
                out.push(name);
            }
            rest = &after[end + 2..];
        }
        out
    }

    pub fn fill(&self, values: &[(String, String)]) -> String {
        let mut s = self.cmd.clone();
        for (k, v) in values {
            s = s.replace(&format!("{{{{{k}}}}}"), v);
            s = s.replace(&format!("{{{{ {k} }}}}"), v);
        }
        s
    }
}

/// Automatisch gefüllte Platzhalter
pub const AUTO_PLACEHOLDERS: [&str; 3] = ["host", "user", "ip"];

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct BaudProfile {
    pub rate: u32,
    pub label: String,
}

/// Programmeinstellungen
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct Prefs {
    // Terminal
    pub font_size: f32,
    pub scrollback: usize,
    pub cursor: String,
    pub theme: String,
    pub copy_on_select: bool,
    pub right_click_paste: bool,
    // Verbindung
    pub keepalive: u64,
    pub legacy_algorithms: bool,
    pub default_user: String,
    pub x11_display: u32,
    // Sicherheit
    pub verify_new_hosts: bool,
    pub clipboard_clear: u64,
    pub lock_hash: Option<String>,
    pub autolock_minutes: u64,
    pub confirm_close: bool,
    // Sitzungen
    pub restore_tabs: bool,
    pub log_sessions: bool,
    pub log_dir: String,
    // Oberfläche
    pub splash: bool,
    pub sidebar_open: bool,
    /// "auto", "de" oder "en"
    pub language: String,
    /// CPU/RAM/Festplatte des Servers in der Statusleiste
    pub server_stats: bool,
    /// Erreichbarkeit der Geräte in der Liste prüfen
    pub check_reachability: bool,
    /// Gleichzeitige SFTP-Übertragungen
    pub parallel_transfers: usize,
    // Updates
    pub update_check: bool,
    pub update_url: String,
    pub skip_version: String,
}

impl Default for Prefs {
    fn default() -> Self {
        Self {
            font_size: 14.0,
            scrollback: 100_000,
            cursor: "block".into(),
            theme: "nebula".into(),
            copy_on_select: true,
            right_click_paste: true,
            keepalive: 30,
            legacy_algorithms: true,
            default_user: "root".into(),
            x11_display: 0,
            verify_new_hosts: true,
            clipboard_clear: 30,
            lock_hash: None,
            autolock_minutes: 0,
            confirm_close: true,
            restore_tabs: false,
            log_sessions: false,
            log_dir: String::new(),
            splash: true,
            sidebar_open: true,
            language: "auto".into(),
            server_stats: true,
            check_reachability: false,
            parallel_transfers: 2,
            update_check: true,
            update_url: String::new(),
            skip_version: String::new(),
        }
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
struct SettingsFile {
    snippets: Vec<Snippet>,
    baud_profiles: Vec<BaudProfile>,
    prefs: Prefs,
}

pub fn default_snippets() -> Vec<Snippet> {
    let s = |name: &str, cmd: &str, group: &str, pinned: bool| Snippet {
        name: name.into(),
        cmd: cmd.into(),
        is_default: true,
        group: group.into(),
        pinned,
    };
    vec![
        s("Update", "sudo apt update && sudo apt upgrade -y\n", "System", true),
        s("Reboot", "sudo reboot\n", "System", true),
        s("Disk", "df -h\n", "System", false),
        s("Top 10 RAM", "ps aux --sort=-%mem | head -n 11\n", "System", false),
        s("Service-Status", "systemctl status {{service}} --no-pager\n", "System", false),
        s("Logs (journal)", "journalctl -u {{service}} -n 100 --no-pager\n", "System", false),
        s("Ports", "ss -tulpn\n", t!("Netzwerk", "Network"), false),
        s("Docker ps", "docker ps --format 'table {{.Names}}\\t{{.Status}}\\t{{.Ports}}'\n", "Docker", false),
    ]
}

pub fn default_bauds() -> Vec<BaudProfile> {
    [9600, 19200, 38400, 57600, 115200]
        .into_iter()
        .map(|rate| BaudProfile { rate, label: rate.to_string() })
        .collect()
}

pub fn load_hosts() -> Vec<Host> {
    serde_json::from_str(&storage::load_json("hosts.json")).unwrap_or_default()
}

pub fn save_hosts(hosts: &[Host]) -> Result<(), String> {
    let json = serde_json::to_string_pretty(hosts).map_err(|e| e.to_string())?;
    storage::save_json("hosts.json", &json).map_err(|e| e.to_string())
}

pub fn load_keys() -> Vec<SshKey> {
    serde_json::from_str(&storage::load_json("keys.json")).unwrap_or_default()
}

pub fn save_keys(keys: &[SshKey]) -> Result<(), String> {
    let json = serde_json::to_string(keys).map_err(|e| e.to_string())?;
    storage::save_json("keys.json", &json).map_err(|e| e.to_string())
}

/// Offene Sitzungen beim Beenden (verschlüsselt, da Zugangsdaten enthalten sein können).
pub fn load_sessions() -> Vec<Host> {
    serde_json::from_str(&storage::load_json("sessions.json")).unwrap_or_default()
}

pub fn save_sessions(tabs: &[Host]) {
    if let Ok(json) = serde_json::to_string(tabs) {
        let _ = storage::save_json("sessions.json", &json);
    }
}

pub struct Settings {
    pub snippets: Vec<Snippet>,
    pub bauds: Vec<BaudProfile>,
    pub prefs: Prefs,
}

/// Lädt Makros, Baudraten und Einstellungen; Standardwerte stehen immer vorne.
pub fn load_settings() -> Settings {
    let s: SettingsFile = std::fs::read_to_string(storage::data_dir().join("settings.json"))
        .ok()
        .and_then(|t| serde_json::from_str(&t).ok())
        .unwrap_or_default();
    let mut snippets = default_snippets();
    let custom: Vec<Snippet> = s
        .snippets
        .into_iter()
        .filter(|c| !snippets.iter().any(|d| d.name == c.name))
        .map(|mut c| {
            c.is_default = false;
            c
        })
        .collect();
    snippets.extend(custom);
    let mut bauds = default_bauds();
    bauds.extend(s.baud_profiles.into_iter().filter(|c| !default_bauds().iter().any(|d| d.label == c.label)));
    bauds.sort_by_key(|b| b.rate);
    Settings { snippets, bauds, prefs: s.prefs }
}

pub fn save_settings(snippets: &[Snippet], bauds: &[BaudProfile], prefs: &Prefs) {
    let defaults = default_bauds();
    let s = SettingsFile {
        snippets: snippets.iter().filter(|s| !s.is_default).cloned().collect(),
        baud_profiles: bauds.iter().filter(|b| !defaults.iter().any(|d| d.label == b.label)).cloned().collect(),
        prefs: prefs.clone(),
    };
    if let Ok(json) = serde_json::to_string_pretty(&s) {
        let path = storage::data_dir().join("settings.json");
        let tmp = path.with_extension("tmp");
        if std::fs::write(&tmp, json).is_ok() {
            let _ = std::fs::rename(tmp, path);
        }
    }
}

pub fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// "vor 5 Min." usw.
pub fn ago(ts: u64) -> String {
    if ts == 0 {
        return t!("noch nie", "never").into();
    }
    let d = now().saturating_sub(ts);
    match d {
        0..=59 => t!("gerade eben", "just now").into(),
        60..=3599 => tf!("vor {} Min.", "{} min ago", d / 60),
        3600..=86_399 => tf!("vor {} Std.", "{} h ago", d / 3600),
        _ => tf!("vor {} Tagen", "{} days ago", d / 86_400),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn old_host_format_still_loads() {
        // Format aus Version 1.x (Wails)
        let v1 = r#"[{"type":"ssh","name":"sw","ip":"10.0.0.2","user":"admin","pass":"x","authMethod":"password","keyName":""}]"#;
        let h: Vec<Host> = serde_json::from_str(v1).unwrap();
        assert_eq!(h[0].ip, "10.0.0.2");
        assert_eq!(h[0].baud, 115200);
        assert!(!h[0].favorite);
        assert!(h[0].forwards.is_empty() && h[0].is_ssh());
    }

    #[test]
    fn old_settings_format_still_loads() {
        let v2 = r#"{"snippets":[{"name":"x","cmd":"ls\n"}],"baudProfiles":[{"rate":1200,"label":"1200 (Alt)"}]}"#;
        let s: SettingsFile = serde_json::from_str(v2).unwrap();
        assert_eq!(s.snippets.len(), 1);
        assert!(s.snippets[0].pinned);
        assert_eq!(s.prefs, Prefs::default());
    }

    #[test]
    fn placeholders() {
        let s = Snippet {
            name: "x".into(),
            cmd: "ssh {{user}}@{{host}} -p {{ port }} && echo {{user}} {print $1}".into(),
            is_default: false,
            group: String::new(),
            pinned: true,
        };
        assert_eq!(s.placeholders(), vec!["user", "host", "port"]);
        let filled = s.fill(&[("user".into(), "root".into()), ("host".into(), "h".into()), ("port".into(), "22".into())]);
        assert_eq!(filled, "ssh root@h -p 22 && echo root {print $1}");
        let d = default_snippets().into_iter().find(|s| s.name == "Docker ps").unwrap();
        assert!(d.placeholders().is_empty());
    }

    #[test]
    fn host_kinds() {
        let h = Host { kind: "telnet".into(), ip: "sw1:2323".into(), ..Default::default() };
        assert!(h.is_telnet() && !h.is_ssh() && h.is_network());
        assert_eq!(h.net_port(), 2323);
        assert_eq!(h.host_only(), "sw1");
        let h = Host { ip: "10.0.0.1".into(), ..Default::default() };
        assert_eq!(h.net_port(), 22);
    }
}
