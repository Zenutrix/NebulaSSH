//! Verschlüsselte Ablage für hosts.json / keys.json.
//!
//! Kompatibel zur alten Go-Version: AES-256-GCM, Format base64(nonce || ciphertext+tag),
//! Schlüssel liegt in der Windows-Anmeldeinformationsverwaltung unter
//! "NebulaSSH:encryption_key" (so wie go-keyring ihn abgelegt hat).

use aes_gcm::aead::{Aead, KeyInit};
use aes_gcm::{Aes256Gcm, Nonce};
use anyhow::{anyhow, Result};
use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine;
use rand::RngCore;
use std::path::PathBuf;
use std::sync::OnceLock;

const KEYRING_SERVICE: &str = "NebulaSSH";
const KEYRING_USER: &str = "encryption_key";

/// Verzeichnis für Konfigurationsdateien.
/// Wie bisher neben der EXE (portabel). Ist das nicht beschreibbar
/// (z.B. unter "Programme"), wird %APPDATA%\NebulaSSH verwendet.
pub fn data_dir() -> PathBuf {
    static DIR: OnceLock<PathBuf> = OnceLock::new();
    DIR.get_or_init(|| {
        if let Some(exe_dir) = std::env::current_exe()
            .ok()
            .and_then(|p| p.parent().map(|d| d.to_path_buf()))
        {
            let probe = exe_dir.join(".nebula_write_test");
            if std::fs::write(&probe, b"x").is_ok() {
                let _ = std::fs::remove_file(&probe);
                return exe_dir;
            }
        }
        let appdata = std::env::var_os("APPDATA")
            .map(PathBuf::from)
            .unwrap_or_else(std::env::temp_dir)
            .join("NebulaSSH");
        let _ = std::fs::create_dir_all(&appdata);
        appdata
    })
    .clone()
}

fn encryption_key() -> Result<[u8; 32]> {
    static KEY: OnceLock<[u8; 32]> = OnceLock::new();
    if let Some(k) = KEY.get() {
        return Ok(*k);
    }
    let target = format!("{KEYRING_SERVICE}:{KEYRING_USER}");
    let key = match cred::read(&target) {
        Some(secret) => {
            let raw = B64
                .decode(secret.trim())
                .map_err(|e| anyhow!("Schlüssel im Tresor ungültig: {e}"))?;
            raw.try_into()
                .map_err(|_| anyhow!("Schlüssel im Tresor hat falsche Länge"))?
        }
        None => {
            let mut k = [0u8; 32];
            rand::thread_rng().fill_bytes(&mut k);
            cred::write(&target, KEYRING_USER, B64.encode(k).as_bytes())?;
            k
        }
    };
    let _ = KEY.set(key);
    Ok(key)
}

pub fn encrypt(plaintext: &[u8]) -> Result<String> {
    let key = encryption_key()?;
    let cipher = Aes256Gcm::new_from_slice(&key)?;
    let mut nonce = [0u8; 12];
    rand::thread_rng().fill_bytes(&mut nonce);
    let ct = cipher
        .encrypt(Nonce::from_slice(&nonce), plaintext)
        .map_err(|_| anyhow!("Verschlüsselung fehlgeschlagen"))?;
    let mut out = Vec::with_capacity(12 + ct.len());
    out.extend_from_slice(&nonce);
    out.extend_from_slice(&ct);
    Ok(B64.encode(out))
}

pub fn decrypt(text: &str) -> Result<Vec<u8>> {
    let data = B64.decode(text.trim())?;
    if data.len() < 12 {
        return Err(anyhow!("ciphertext too short"));
    }
    let key = encryption_key()?;
    let cipher = Aes256Gcm::new_from_slice(&key)?;
    let (nonce, ct) = data.split_at(12);
    cipher
        .decrypt(Nonce::from_slice(nonce), ct)
        .map_err(|_| anyhow!("Entschlüsselung fehlgeschlagen"))
}

/// Lädt eine (verschlüsselte oder alte unverschlüsselte) JSON-Datei. Fehler => "[]".
pub fn load_json(file: &str) -> String {
    let path = data_dir().join(file);
    let Ok(data) = std::fs::read_to_string(&path) else {
        return "[]".into();
    };
    let trimmed = data.trim_start();
    if trimmed.starts_with('[') || trimmed.starts_with('{') {
        return data;
    }
    match decrypt(&data).ok().and_then(|d| String::from_utf8(d).ok()) {
        Some(s) => s,
        None => "[]".into(),
    }
}

/// Speichert verschlüsselt und atomar (erst .tmp, dann umbenennen).
pub fn save_json(file: &str, json: &str) -> Result<()> {
    let path = data_dir().join(file);
    let enc = encrypt(json.as_bytes())?;
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, enc)?;
    std::fs::rename(&tmp, &path)?;
    Ok(())
}

#[cfg(windows)]
mod cred {
    use anyhow::{anyhow, Result};
    use windows_sys::Win32::Security::Credentials::{
        CredFree, CredReadW, CredWriteW, CREDENTIALW, CRED_PERSIST_ENTERPRISE, CRED_TYPE_GENERIC,
    };

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().chain(std::iter::once(0)).collect()
    }

    pub fn read(target: &str) -> Option<String> {
        let t = wide(target);
        let mut pcred: *mut CREDENTIALW = std::ptr::null_mut();
        // SAFETY: gültiger, nullterminierter String; pcred wird von Windows befüllt
        // und anschließend mit CredFree freigegeben.
        unsafe {
            if CredReadW(t.as_ptr(), CRED_TYPE_GENERIC, 0, &mut pcred) == 0 || pcred.is_null() {
                return None;
            }
            let c = &*pcred;
            let blob =
                std::slice::from_raw_parts(c.CredentialBlob, c.CredentialBlobSize as usize).to_vec();
            CredFree(pcred as *const _);
            String::from_utf8(blob).ok()
        }
    }

    pub fn write(target: &str, user: &str, secret: &[u8]) -> Result<()> {
        let mut t = wide(target);
        let mut u = wide(user);
        let mut blob = secret.to_vec();
        // SAFETY: alle Zeiger zeigen auf lokal gehaltene Puffer, die den Aufruf überleben.
        unsafe {
            let mut c: CREDENTIALW = std::mem::zeroed();
            c.Type = CRED_TYPE_GENERIC;
            c.TargetName = t.as_mut_ptr();
            c.UserName = u.as_mut_ptr();
            c.CredentialBlobSize = blob.len() as u32;
            c.CredentialBlob = blob.as_mut_ptr();
            c.Persist = CRED_PERSIST_ENTERPRISE;
            if CredWriteW(&c, 0) == 0 {
                return Err(anyhow!("Konnte Schlüssel nicht im Windows-Tresor speichern"));
            }
        }
        Ok(())
    }
}

#[cfg(not(windows))]
mod cred {
    // Fallback für Nicht-Windows-Builds: Schlüssel als Datei im Datenverzeichnis.
    use anyhow::Result;
    fn path(target: &str) -> std::path::PathBuf {
        super::data_dir().join(format!(".{}", target.replace(':', "_")))
    }
    pub fn read(target: &str) -> Option<String> {
        std::fs::read_to_string(path(target)).ok()
    }
    pub fn write(target: &str, _user: &str, secret: &[u8]) -> Result<()> {
        std::fs::write(path(target), secret)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip() {
        let enc = encrypt(b"[{\"name\":\"test\"}]").unwrap();
        assert_eq!(decrypt(&enc).unwrap(), b"[{\"name\":\"test\"}]");
    }
}

#[cfg(test)]
mod compat {
    /// Prüft, dass eine von der Go-Version verschlüsselte Datei gelesen werden kann.
    #[test]
    #[ignore]
    fn decrypt_legacy_hosts() {
        let data = std::fs::read_to_string(std::env::var("NEBULA_COMPAT_FILE").unwrap()).unwrap();
        let plain = super::decrypt(&data).expect("Go-Datei nicht entschlüsselbar");
        let v: serde_json::Value = serde_json::from_slice(&plain).unwrap();
        println!("OK: {} Einträge", v.as_array().map(|a| a.len()).unwrap_or(0));
    }
}
