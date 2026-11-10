//! Import vorhandener Verbindungen aus `~/.ssh/config`, PuTTY und MobaXterm.

use crate::ssh::Forward;
use crate::{t, tf};
use std::path::{Path, PathBuf};

/// Eine gefundene Verbindung (unabhängig vom Format der App).
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Imported {
    pub name: String,
    /// "ssh", "telnet" oder "serial"
    pub kind: String,
    /// host oder host:port (Port nur, wenn nicht Standard)
    pub host: String,
    pub user: String,
    /// Pfad zu einem privaten Schlüssel (OpenSSH-Format)
    pub key_file: Option<PathBuf>,
    /// Schlüssel liegt im Agent (z.B. PuTTY-.ppk über Pageant)
    pub use_agent: bool,
    /// Jump-Host: Name einer anderen importierten Verbindung oder user@host:port
    pub jump: String,
    pub forwards: Vec<Forward>,
    pub group: String,
    pub serial_port: String,
    pub baud: u32,
    pub agent_forward: bool,
    pub x11: bool,
}

fn host_port(host: &str, port: u16, default: u16) -> String {
    if port == 0 || port == default {
        host.to_string()
    } else if host.contains(':') {
        format!("[{host}]:{port}")
    } else {
        format!("{host}:{port}")
    }
}

fn home() -> PathBuf {
    std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME")).map(PathBuf::from).unwrap_or_default()
}

fn expand(p: &str, base: &Path) -> PathBuf {
    let p = p.trim().trim_matches('"');
    if let Some(rest) = p.strip_prefix("~/").or_else(|| p.strip_prefix("~\\")) {
        return home().join(rest);
    }
    let pb = PathBuf::from(p);
    if pb.is_absolute() {
        pb
    } else {
        base.join(pb)
    }
}

// ---------------------------------------------------------------------------
// OpenSSH: ~/.ssh/config
// ---------------------------------------------------------------------------

pub fn default_ssh_config() -> PathBuf {
    home().join(".ssh").join("config")
}

fn yes(v: &str) -> bool {
    v.eq_ignore_ascii_case("yes")
}

/// "8080 localhost:80", "127.0.0.1:8080 db:5432", "1080"
fn ssh_forward(kind: &str, value: &str) -> Option<Forward> {
    let mut it = value.split_whitespace();
    let bind = it.next()?.to_string();
    let target = it.next().unwrap_or("").to_string();
    if kind != "dynamic" && target.is_empty() {
        return None;
    }
    Some(Forward { kind: kind.into(), bind, target, enabled: true })
}

/// Wertet eine ssh_config aus. `base` ist der Ordner für relative IdentityFile-Pfade.
pub fn parse_ssh_config(text: &str, base: &Path) -> Vec<Imported> {
    #[derive(Default, Clone)]
    struct Block {
        names: Vec<String>,
        host: String,
        port: u16,
        user: String,
        key: Option<PathBuf>,
        jump: String,
        forwards: Vec<Forward>,
        agent: bool,
        x11: bool,
    }
    let mut blocks: Vec<Block> = Vec::new();
    let mut global = Block::default();
    let mut cur: Option<Block> = None;
    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let (key, value) = match line.split_once(|c: char| c == '=' || c.is_whitespace()) {
            Some((k, v)) => (k.trim().to_lowercase(), v.trim().trim_start_matches('=').trim().to_string()),
            None => continue,
        };
        if key == "host" {
            if let Some(b) = cur.take() {
                blocks.push(b);
            }
            let names: Vec<String> = value.split_whitespace().map(|s| s.trim_matches('"').to_string()).collect();
            if names.iter().all(|n| n == "*") {
                cur = None; // Einstellungen für alle -> global
                continue;
            }
            cur = Some(Block { names, ..Default::default() });
            continue;
        }
        if key == "match" {
            if let Some(b) = cur.take() {
                blocks.push(b);
            }
            cur = Some(Block::default()); // wird ignoriert (keine Namen)
            continue;
        }
        let b = cur.as_mut().unwrap_or(&mut global);
        match key.as_str() {
            "hostname" => b.host = value,
            "port" => b.port = value.parse().unwrap_or(0),
            "user" => b.user = value,
            "identityfile" if b.key.is_none() => b.key = Some(expand(&value, base)),
            "proxyjump" if !value.eq_ignore_ascii_case("none") => b.jump = value.split(',').next().unwrap_or("").trim().to_string(),
            "localforward" => b.forwards.extend(ssh_forward("local", &value)),
            "remoteforward" => b.forwards.extend(ssh_forward("remote", &value)),
            "dynamicforward" => b.forwards.extend(ssh_forward("dynamic", &value)),
            "forwardagent" => b.agent = yes(&value),
            "forwardx11" | "forwardx11trusted" => b.x11 = b.x11 || yes(&value),
            _ => {}
        }
    }
    if let Some(b) = cur.take() {
        blocks.push(b);
    }
    let mut out = Vec::new();
    for b in blocks {
        for name in b.names.iter().filter(|n| !n.contains(['*', '?', '!'])) {
            let host = if b.host.is_empty() { name.clone() } else { b.host.replace("%h", name) };
            let port = if b.port > 0 { b.port } else { global.port };
            out.push(Imported {
                name: name.clone(),
                kind: "ssh".into(),
                host: host_port(&host, port, 22),
                user: if b.user.is_empty() { global.user.clone() } else { b.user.clone() },
                key_file: b.key.clone().or_else(|| global.key.clone()),
                jump: if b.jump.is_empty() { global.jump.clone() } else { b.jump.clone() },
                forwards: b.forwards.clone(),
                agent_forward: b.agent || global.agent,
                x11: b.x11 || global.x11,
                group: "SSH-Config".into(),
                ..Default::default()
            });
        }
    }
    out
}

// ---------------------------------------------------------------------------
// PuTTY (Registry: HKCU\Software\SimonTatham\PuTTY\Sessions)
// ---------------------------------------------------------------------------

fn url_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::new();
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' && i + 2 < b.len() {
            if let Some(v) = std::str::from_utf8(&b[i + 1..i + 3]).ok().and_then(|h| u8::from_str_radix(h, 16).ok()) {
                out.push(v);
                i += 3;
                continue;
            }
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).to_string()
}

/// "L8080=localhost:80,4R9000=127.0.0.1:22,D1080"
fn putty_forwards(s: &str) -> Vec<Forward> {
    s.split(',')
        .filter_map(|e| {
            let e = e.trim().trim_start_matches(['4', '6']);
            let kind = match e.chars().next()? {
                'L' => "local",
                'R' => "remote",
                'D' => "dynamic",
                _ => return None,
            };
            let rest = &e[1..];
            let (bind, target) = rest.split_once('=').unwrap_or((rest, ""));
            if bind.is_empty() {
                return None;
            }
            Some(Forward { kind: kind.into(), bind: bind.into(), target: target.into(), enabled: true })
        })
        .collect()
}

/// Liest den Text einer .reg-Datei (Export von `HKCU\Software\SimonTatham\PuTTY\Sessions`).
pub fn parse_putty_reg(text: &str) -> Vec<Imported> {
    let mut out: Vec<Imported> = Vec::new();
    let mut vals: Vec<(String, String)> = Vec::new();
    let mut name: Option<String> = None;
    let flush = |name: &Option<String>, vals: &mut Vec<(String, String)>, out: &mut Vec<Imported>| {
        let Some(n) = name else { return };
        let get = |k: &str| vals.iter().find(|(a, _)| a.eq_ignore_ascii_case(k)).map(|(_, v)| v.clone()).unwrap_or_default();
        let num = |k: &str| {
            let v = get(k);
            v.strip_prefix("dword:").and_then(|h| u32::from_str_radix(h, 16).ok()).unwrap_or(0)
        };
        let proto = get("Protocol").to_lowercase();
        let host = get("HostName");
        let mut it = Imported { name: n.clone(), group: "PuTTY".into(), ..Default::default() };
        match proto.as_str() {
            "serial" => {
                it.kind = "serial".into();
                it.serial_port = get("SerialLine");
                it.baud = num("SerialSpeed");
                if it.serial_port.is_empty() {
                    vals.clear();
                    return;
                }
            }
            "telnet" | "ssh" | "" => {
                if host.is_empty() {
                    vals.clear();
                    return;
                }
                it.kind = if proto == "telnet" { "telnet".into() } else { "ssh".into() };
                let default = if it.kind == "telnet" { 23 } else { 22 };
                let (h, user) = match host.rsplit_once('@') {
                    Some((u, h)) => (h.to_string(), u.to_string()),
                    None => (host.clone(), get("UserName")),
                };
                it.host = host_port(&h, num("PortNumber") as u16, default);
                it.user = user;
                it.forwards = putty_forwards(&get("PortForwardings"));
                it.agent_forward = num("AgentFwd") == 1;
                it.x11 = num("X11Forward") == 1;
                let key = get("PublicKeyFile");
                if !key.is_empty() {
                    // .ppk wird über Pageant genutzt
                    it.use_agent = true;
                    if !key.to_lowercase().ends_with(".ppk") {
                        it.key_file = Some(PathBuf::from(key));
                        it.use_agent = false;
                    }
                }
            }
            _ => {
                vals.clear();
                return;
            }
        }
        out.push(it);
        vals.clear();
    };
    for raw in text.lines() {
        let line = raw.trim().trim_start_matches('\u{feff}');
        if let Some(sec) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            flush(&name, &mut vals, &mut out);
            name = sec
                .split_once("\\Sessions\\")
                .map(|(_, n)| url_decode(n))
                .filter(|n| !n.contains('\\') && n != "Default Settings");
            continue;
        }
        if let Some(rest) = line.strip_prefix('"') {
            if let Some((k, v)) = rest.split_once("\"=") {
                let v = v.trim();
                let v = if let Some(s) = v.strip_prefix('"').and_then(|s| s.strip_suffix('"')) {
                    s.replace("\\\\", "\\").replace("\\\"", "\"")
                } else {
                    v.to_string()
                };
                vals.push((k.to_string(), v));
            }
        }
    }
    flush(&name, &mut vals, &mut out);
    out
}

/// Liest die PuTTY-Sitzungen dieses Benutzers aus der Registry (über `reg export`).
pub fn putty_sessions() -> Result<Vec<Imported>, String> {
    let file = std::env::temp_dir().join(format!("nebula-putty-{}.reg", std::process::id()));
    let mut cmd = std::process::Command::new("reg");
    cmd.args(["export", r"HKCU\Software\SimonTatham\PuTTY\Sessions"]).arg(&file).arg("/y");
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    let st = cmd.output().map_err(|e| e.to_string())?;
    if !st.status.success() {
        return Err(t!("Keine PuTTY-Sitzungen gefunden.", "No PuTTY sessions found.").into());
    }
    let bytes = std::fs::read(&file).map_err(|e| e.to_string());
    let _ = std::fs::remove_file(&file);
    Ok(parse_putty_reg(&decode_text(&bytes?)))
}

/// UTF-16 (mit BOM), UTF-8 oder Windows-1252.
pub fn decode_text(bytes: &[u8]) -> String {
    if bytes.len() >= 2 && bytes[0] == 0xff && bytes[1] == 0xfe {
        let u: Vec<u16> = bytes[2..].chunks_exact(2).map(|c| u16::from_le_bytes([c[0], c[1]])).collect();
        return String::from_utf16_lossy(&u);
    }
    match std::str::from_utf8(bytes) {
        Ok(s) => s.trim_start_matches('\u{feff}').to_string(),
        Err(_) => bytes.iter().map(|&b| b as char).collect(),
    }
}

// ---------------------------------------------------------------------------
// MobaXterm (MobaXterm.ini, Abschnitte [Bookmarks], [Bookmarks_1] …)
// ---------------------------------------------------------------------------

/// Typische Speicherorte der MobaXterm.ini
pub fn find_mobaxterm_ini() -> Option<PathBuf> {
    let mut c = Vec::new();
    if let Some(a) = std::env::var_os("APPDATA") {
        c.push(PathBuf::from(&a).join("MobaXterm").join("MobaXterm.ini"));
    }
    c.push(home().join("Documents").join("MobaXterm").join("MobaXterm.ini"));
    c.into_iter().find(|p| p.exists())
}

pub fn parse_mobaxterm(text: &str) -> Vec<Imported> {
    let mut out = Vec::new();
    let mut in_bm = false;
    let mut group = String::new();
    for raw in text.lines() {
        let line = raw.trim();
        if line.starts_with('[') {
            in_bm = line.to_lowercase().starts_with("[bookmarks");
            group.clear();
            continue;
        }
        if !in_bm {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else { continue };
        match key {
            "SubRep" => {
                group = value.replace('\\', " / ");
                continue;
            }
            "ImgNum" => continue,
            _ => {}
        }
        // Name= #109#0%host%port%user%…#MobaFont…
        let v = value.trim();
        let mut parts = v.trim_start_matches(' ').splitn(4, '#');
        parts.next(); // leer vor dem ersten '#'
        let (Some(kind), Some(data)) = (parts.next(), parts.next()) else { continue };
        // data = "<Symbol>%host%port%user%…"
        let fields: Vec<&str> = data.split('%').skip(1).collect();
        let mut it = Imported {
            name: key.trim().to_string(),
            group: if group.is_empty() { "MobaXterm".into() } else { format!("MobaXterm / {group}") },
            ..Default::default()
        };
        match kind {
            "109" | "98" => {
                let host = fields.first().copied().unwrap_or("").trim();
                if host.is_empty() {
                    continue;
                }
                it.kind = if kind == "98" { "telnet".into() } else { "ssh".into() };
                let port: u16 = fields.get(1).and_then(|p| p.trim().parse().ok()).unwrap_or(0);
                it.host = host_port(host, port, if kind == "98" { 23 } else { 22 });
                it.user = fields.get(2).copied().unwrap_or("").trim().to_string();
                if kind == "109" {
                    // Feld 7 = Jump-Host ("host" oder leer), Feld 14 = privater Schlüssel
                    let looks_like_host = |s: &&str| !s.is_empty() && !s.chars().all(|c| c.is_ascii_digit() || c == '-');
                    if let Some(j) = fields.get(7).map(|s| s.trim()).filter(looks_like_host) {
                        let jport: u16 = fields.get(8).and_then(|p| p.trim().parse().ok()).unwrap_or(22);
                        let juser = fields.get(9).copied().unwrap_or("").trim();
                        let hp = host_port(j, jport, 22);
                        it.jump = if juser.is_empty() { hp } else { format!("{juser}@{hp}") };
                    }
                    if let Some(k) = fields.get(14).map(|s| s.trim()).filter(|s| !s.is_empty()) {
                        let k = k.replace("_CurrentDrive_:", "C:").replace("_ProfileDir_", &home().to_string_lossy());
                        if k.to_lowercase().ends_with(".ppk") {
                            it.use_agent = true;
                        } else {
                            it.key_file = Some(PathBuf::from(k));
                        }
                    }
                }
            }
            "131" => {
                // Serial: erstes Feld enthält "COM3  (Beschreibung)", danach die Baudrate
                let port = fields.iter().find_map(|f| f.split_whitespace().find(|w| w.to_uppercase().starts_with("COM")));
                let Some(port) = port else { continue };
                it.kind = "serial".into();
                it.serial_port = port.to_uppercase();
                it.baud = fields.iter().filter_map(|f| f.trim().parse::<u32>().ok()).find(|b| *b >= 300).unwrap_or(115200);
            }
            _ => continue,
        }
        out.push(it);
    }
    out
}

/// Liest eine Datei und erkennt das Format (ssh_config, .reg, MobaXterm.ini).
pub fn from_file(path: &Path) -> Result<Vec<Imported>, String> {
    let bytes = std::fs::read(path).map_err(|e| e.to_string())?;
    if bytes.len() > 20 * 1024 * 1024 {
        return Err(t!("Datei ist zu groß.", "File is too large.").into());
    }
    let text = decode_text(&bytes);
    let lower = text.to_lowercase();
    let list = if lower.contains("windows registry editor") || lower.contains("simontatham") {
        parse_putty_reg(&text)
    } else if lower.contains("[bookmarks") {
        parse_mobaxterm(&text)
    } else {
        parse_ssh_config(&text, path.parent().unwrap_or(Path::new(".")))
    };
    if list.is_empty() {
        Err(tf!("In {} wurden keine Verbindungen gefunden.", "No connections found in {}.", path.display()))
    } else {
        Ok(list)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ssh_config() {
        let text = "\
User global
Host *
    ForwardAgent yes

Host web prod-web
    HostName 10.0.0.5
    Port 2200
    User admin
    IdentityFile ~/.ssh/id_ed25519
    LocalForward 8080 localhost:80
    DynamicForward 1080

Host db
    HostName=db.internal
    ProxyJump web
    RemoteForward 9000 127.0.0.1:3000

Host *.example.com
    User x
";
        let v = parse_ssh_config(text, Path::new("C:\\cfg"));
        assert_eq!(v.len(), 3);
        assert_eq!(v[0].name, "web");
        assert_eq!(v[0].host, "10.0.0.5:2200");
        assert_eq!(v[0].user, "admin");
        assert!(v[0].key_file.as_ref().unwrap().ends_with(".ssh\\id_ed25519") || v[0].key_file.as_ref().unwrap().ends_with(".ssh/id_ed25519"));
        assert_eq!(v[0].forwards.len(), 2);
        assert_eq!(v[0].forwards[1].kind, "dynamic");
        assert!(v[0].agent_forward);
        assert_eq!(v[1].name, "prod-web");
        assert_eq!(v[2].host, "db.internal");
        assert_eq!(v[2].user, "global");
        assert_eq!(v[2].jump, "web");
        assert_eq!(v[2].forwards[0].target, "127.0.0.1:3000");
    }

    #[test]
    fn putty_reg() {
        let text = "Windows Registry Editor Version 5.00\r\n\r\n\
[HKEY_CURRENT_USER\\Software\\SimonTatham\\PuTTY\\Sessions]\r\n\r\n\
[HKEY_CURRENT_USER\\Software\\SimonTatham\\PuTTY\\Sessions\\Mein%20Server]\r\n\
\"HostName\"=\"root@10.1.1.1\"\r\n\"PortNumber\"=dword:00000016\r\n\"Protocol\"=\"ssh\"\r\n\
\"PortForwardings\"=\"L8080=localhost:80,4D1080,R9000=127.0.0.1:22\"\r\n\"AgentFwd\"=dword:00000001\r\n\
\"PublicKeyFile\"=\"C:\\\\keys\\\\a.ppk\"\r\n\r\n\
[HKEY_CURRENT_USER\\Software\\SimonTatham\\PuTTY\\Sessions\\Switch]\r\n\
\"HostName\"=\"sw1\"\r\n\"PortNumber\"=dword:00000017\r\n\"Protocol\"=\"telnet\"\r\n\r\n\
[HKEY_CURRENT_USER\\Software\\SimonTatham\\PuTTY\\Sessions\\Konsole]\r\n\
\"Protocol\"=\"serial\"\r\n\"SerialLine\"=\"COM4\"\r\n\"SerialSpeed\"=dword:00002580\r\n";
        let v = parse_putty_reg(text);
        assert_eq!(v.len(), 3);
        assert_eq!(v[0].name, "Mein Server");
        assert_eq!(v[0].host, "10.1.1.1");
        assert_eq!(v[0].user, "root");
        assert!(v[0].use_agent && v[0].agent_forward);
        assert_eq!(v[0].forwards.len(), 3);
        assert_eq!(v[0].forwards[1], Forward { kind: "dynamic".into(), bind: "1080".into(), target: String::new(), enabled: true });
        assert_eq!(v[1].kind, "telnet");
        assert_eq!(v[1].host, "sw1");
        assert_eq!(v[2].serial_port, "COM4");
        assert_eq!(v[2].baud, 9600);
        assert_eq!(url_decode("a%20b%"), "a b%");
    }

    #[test]
    fn mobaxterm_ini() {
        let text = "[Bookmarks]\r\nSubRep=\r\nImgNum=42\r\n\
Web= #109#0%192.168.1.10%2222%admin%%-1%-1%%%%%0%0%0%%%-1%0%0%0%%1080%%0%0%1#MobaFont%10%0%0%-1%15%236,236,236#0# #-1\r\n\
[Bookmarks_1]\r\nSubRep=Kunden\\Nord\r\nImgNum=41\r\n\
Sw= #98#1%10.0.0.2%23%%%2%%%%%0%0%%1080%#MobaFont%10#0# #-1\r\n\
Kons= #131#8%COM3  (USB Serial Port)%9600%3%0%0%1%2%#MobaFont%10#0# #-1\r\n";
        let v = parse_mobaxterm(text);
        assert_eq!(v.len(), 3, "{v:?}");
        assert_eq!(v[0].host, "192.168.1.10:2222");
        assert_eq!(v[0].user, "admin");
        assert_eq!(v[0].group, "MobaXterm");
        assert_eq!(v[1].kind, "telnet");
        assert_eq!(v[1].host, "10.0.0.2");
        assert_eq!(v[1].group, "MobaXterm / Kunden / Nord");
        assert_eq!(v[2].serial_port, "COM3");
        assert_eq!(v[2].baud, 9600);
    }
}
