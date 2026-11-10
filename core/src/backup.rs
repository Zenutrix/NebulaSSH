//! Komplettes Backup des Clients (Geräte, Keys, Einstellungen, Host-Keys) für den Umzug
//! auf einen anderen PC. Die Datei wird mit einem Backup-Passwort verschlüsselt
//! (Argon2id + AES-256-GCM), weil der lokale Schlüssel an diesen PC gebunden ist.

use crate::t;
use crate::storage;
use aes_gcm::aead::{Aead, KeyInit};
use aes_gcm::{Aes256Gcm, Nonce};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use zeroize::Zeroize;

const MAGIC: &[u8] = b"NEBULABAK1";

#[derive(Serialize, Deserialize)]
struct Bundle {
    version: u32,
    created: String,
    hosts: String,
    keys: String,
    settings: String,
    known_hosts: String,
}

pub struct Summary {
    pub hosts: usize,
    pub keys: usize,
}

fn derive(password: &str, salt: &[u8]) -> Result<[u8; 32], String> {
    let mut key = [0u8; 32];
    argon2::Argon2::default()
        .hash_password_into(password.as_bytes(), salt, &mut key)
        .map_err(|e| e.to_string())?;
    Ok(key)
}

fn read_plain(file: &str, default: &str) -> String {
    std::fs::read_to_string(storage::data_dir().join(file)).unwrap_or_else(|_| default.into())
}

fn count(json: &str) -> usize {
    serde_json::from_str::<Vec<serde_json::Value>>(json)
        .map(|v| v.len())
        .unwrap_or(0)
}

pub fn export(password: &str) -> Result<(Vec<u8>, Summary), String> {
    if password.chars().count() < 8 {
        return Err(t!("Das Backup-Passwort muss mindestens 8 Zeichen haben.", "The backup password must have at least 8 characters.").into());
    }
    let mut bundle = Bundle {
        version: 1,
        created: chrono::Local::now().format("%Y-%m-%d %H:%M").to_string(),
        hosts: storage::load_json("hosts.json"),
        keys: storage::load_json("keys.json"),
        settings: read_plain("settings.json", "{}"),
        known_hosts: read_plain("known_hosts.json", "{}"),
    };
    let summary = Summary {
        hosts: count(&bundle.hosts),
        keys: count(&bundle.keys),
    };
    let mut plain = serde_json::to_vec(&bundle).map_err(|e| e.to_string())?;
    bundle.keys.zeroize();
    bundle.hosts.zeroize();

    let mut salt = [0u8; 16];
    let mut nonce = [0u8; 12];
    rand::thread_rng().fill_bytes(&mut salt);
    rand::thread_rng().fill_bytes(&mut nonce);
    let mut key = derive(password, &salt)?;
    let cipher = Aes256Gcm::new_from_slice(&key).map_err(|_| "Schlüssel".to_string())?;
    key.zeroize();
    let ct = cipher
        .encrypt(Nonce::from_slice(&nonce), plain.as_slice())
        .map_err(|_| "Verschlüsselung fehlgeschlagen".to_string())?;
    plain.zeroize();

    let mut out = Vec::with_capacity(MAGIC.len() + 28 + ct.len());
    out.extend_from_slice(MAGIC);
    out.extend_from_slice(&salt);
    out.extend_from_slice(&nonce);
    out.extend_from_slice(&ct);
    Ok((out, summary))
}

/// Stellt ein Backup wieder her. Vorhandene Daten werden ersetzt.
pub fn import(data: &[u8], password: &str) -> Result<Summary, String> {
    if data.len() < MAGIC.len() + 28 || &data[..MAGIC.len()] != MAGIC {
        return Err(t!("Das ist keine NebulaSSH-Backup-Datei.", "This is not a NebulaSSH backup file.").into());
    }
    let rest = &data[MAGIC.len()..];
    let (salt, rest) = rest.split_at(16);
    let (nonce, ct) = rest.split_at(12);
    let mut key = derive(password, salt)?;
    let cipher = Aes256Gcm::new_from_slice(&key).map_err(|_| "Schlüssel".to_string())?;
    key.zeroize();
    let mut plain = cipher
        .decrypt(Nonce::from_slice(nonce), ct)
        .map_err(|_| t!("Falsches Passwort oder beschädigte Datei.", "Wrong password or damaged file.").to_string())?;
    let bundle: Bundle = serde_json::from_slice(&plain).map_err(|e| e.to_string())?;
    plain.zeroize();

    storage::save_json("hosts.json", &bundle.hosts).map_err(|e| e.to_string())?;
    storage::save_json("keys.json", &bundle.keys).map_err(|e| e.to_string())?;
    let dir = storage::data_dir();
    std::fs::write(dir.join("settings.json"), &bundle.settings).map_err(|e| e.to_string())?;
    std::fs::write(dir.join("known_hosts.json"), &bundle.known_hosts).map_err(|e| e.to_string())?;
    Ok(Summary {
        hosts: count(&bundle.hosts),
        keys: count(&bundle.keys),
    })
}

/// Hash für die optionale Startsperre (Argon2id, PHC-Format).
pub fn hash_password(password: &str) -> Result<String, String> {
    use argon2::password_hash::{rand_core::OsRng, PasswordHasher, SaltString};
    let salt = SaltString::generate(&mut OsRng);
    argon2::Argon2::default()
        .hash_password(password.as_bytes(), &salt)
        .map(|h| h.to_string())
        .map_err(|e| e.to_string())
}

pub fn verify_password(password: &str, hash: &str) -> bool {
    use argon2::password_hash::{PasswordHash, PasswordVerifier};
    PasswordHash::new(hash)
        .map(|h| {
            argon2::Argon2::default()
                .verify_password(password.as_bytes(), &h)
                .is_ok()
        })
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn password_hash() {
        let h = hash_password("geheim123").unwrap();
        assert!(verify_password("geheim123", &h));
        assert!(!verify_password("falsch", &h));
    }
    #[test]
    fn export_import_roundtrip() {
        let hosts = r#"[{"type":"ssh","name":"backup-test","ip":"10.9.9.9","user":"root"}]"#;
        storage::save_json("hosts.json", hosts).unwrap();
        storage::save_json("keys.json", "[]").unwrap();
        let (bytes, sum) = export("ein-langes-passwort").unwrap();
        assert_eq!(sum.hosts, 1);
        // Datei enthält keinen Klartext
        assert!(!String::from_utf8_lossy(&bytes).contains("backup-test"));
        storage::save_json("hosts.json", "[]").unwrap();
        assert!(import(&bytes, "falsches-passwort").is_err());
        let sum = import(&bytes, "ein-langes-passwort").unwrap();
        assert_eq!(sum.hosts, 1);
        assert!(storage::load_json("hosts.json").contains("backup-test"));
    }

    #[test]
    fn rejects_short_password_and_garbage() {
        assert!(export("kurz").is_err());
        assert!(import(b"irgendwas", "passwort123").is_err());
    }
}
