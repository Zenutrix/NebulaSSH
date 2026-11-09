//! Gespeicherte Geräte, SSH-Keys und Einstellungen.
//! Dateiformat ist kompatibel zu NebulaSSH 1.x und 2.0 (neue Felder sind optional).

use nebula_core::storage;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct Host {
    /// "ssh" oder "serial"
    #[serde(rename = "type")]
    pub kind: String,
    pub name: String,
    pub ip: String,
    pub user: String,
    pub pass: String,
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
        }
    }
}

impl Host {
    pub fn is_serial(&self) -> bool {
        self.kind == "serial"
    }
    pub fn display_name(&self) -> String {
        if !self.name.is_empty() {
            self.name.clone()
        } else if self.is_serial() {
            format!("{} ({})", self.port, self.baud)
        } else {
            format!("{}@{}", self.user, self.ip)
        }
    }
    pub fn subtitle(&self) -> String {
        if self.is_serial() {
            format!("{} · {} Baud", self.port, self.baud)
        } else {
            format!("{}@{}", self.user, self.ip)
        }
    }
    /// Identität für das Wiederfinden in der Liste
    pub fn same_device(&self, other: &Host) -> bool {
        if self.is_serial() {
            other.is_serial() && self.name == other.name && self.port == other.port
        } else {
            !other.is_serial() && self.name == other.name && self.ip == other.ip && self.user == other.user
        }
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct SshKey {
    pub name: String,
    pub content: String,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct Snippet {
    pub name: String,
    pub cmd: String,
    #[serde(default, rename = "isDefault", skip_serializing)]
    pub is_default: bool,
}

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
    vec![
        Snippet {
            name: "Update".into(),
            cmd: "sudo apt update && sudo apt upgrade -y\n".into(),
            is_default: true,
        },
        Snippet {
            name: "Reboot".into(),
            cmd: "sudo reboot\n".into(),
            is_default: true,
        },
    ]
}

pub fn default_bauds() -> Vec<BaudProfile> {
    [9600, 19200, 38400, 57600, 115200]
        .into_iter()
        .map(|rate| BaudProfile {
            rate,
            label: rate.to_string(),
        })
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
    bauds.extend(
        s.baud_profiles
            .into_iter()
            .filter(|c| !default_bauds().iter().any(|d| d.label == c.label)),
    );
    bauds.sort_by_key(|b| b.rate);
    Settings {
        snippets,
        bauds,
        prefs: s.prefs,
    }
}

pub fn save_settings(snippets: &[Snippet], bauds: &[BaudProfile], prefs: &Prefs) {
    let defaults = default_bauds();
    let s = SettingsFile {
        snippets: snippets.iter().filter(|s| !s.is_default).cloned().collect(),
        baud_profiles: bauds
            .iter()
            .filter(|b| !defaults.iter().any(|d| d.label == b.label))
            .cloned()
            .collect(),
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
        return "noch nie".into();
    }
    let d = now().saturating_sub(ts);
    match d {
        0..=59 => "gerade eben".into(),
        60..=3599 => format!("vor {} Min.", d / 60),
        3600..=86_399 => format!("vor {} Std.", d / 3600),
        _ => format!("vor {} Tagen", d / 86_400),
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
    }

    #[test]
    fn old_settings_format_still_loads() {
        let v2 = r#"{"snippets":[{"name":"x","cmd":"ls\n"}],"baudProfiles":[{"rate":1200,"label":"1200 (Alt)"}]}"#;
        let s: SettingsFile = serde_json::from_str(v2).unwrap();
        assert_eq!(s.snippets.len(), 1);
        assert_eq!(s.prefs, Prefs::default());
    }
}
