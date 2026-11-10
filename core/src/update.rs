//! Update-System: signiertes Manifest abrufen, Version vergleichen,
//! Download mit Fortschritt und SHA-256-Prüfung.
//!
//! Manifest (update.json):
//! ```json
//! { "payload": "<JSON-Text von Release>", "signature": "<base64 Ed25519-Signatur über payload>" }
//! ```
//! Nur wenn die Signatur zum in der App eingebauten öffentlichen Schlüssel passt,
//! wird das Update überhaupt angeboten. Danach muss die Datei die signierte
//! SHA-256-Prüfsumme haben – so kann weder ein manipulierter Server noch ein
//! Angreifer im Netzwerk ein fremdes Programm unterschieben.

use crate::t;
use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine;
use ed25519_dalek::{Signature, Verifier, VerifyingKey};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Asset {
    pub url: String,
    pub sha256: String,
    pub size: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Release {
    pub version: String,
    pub date: String,
    /// Änderungen, eine pro Zeile
    pub notes: String,
    pub installer: Asset,
    pub portable: Asset,
    /// Ältere Versionen werden zum Update gedrängt (z.B. bei Sicherheitslücken)
    #[serde(default)]
    pub critical: bool,
}

#[derive(Serialize, Deserialize)]
pub struct Manifest {
    pub payload: String,
    pub signature: String,
}

/// "2.10.1" > "2.9.0"; Vorabversionen ("2.2.0-beta") zählen weniger als die finale.
pub fn is_newer(candidate: &str, current: &str) -> bool {
    fn parse(v: &str) -> (Vec<u64>, bool) {
        let v = v.trim().trim_start_matches('v');
        let (core, pre) = match v.split_once('-') {
            Some((c, _)) => (c, true),
            None => (v, false),
        };
        let mut nums: Vec<u64> = core.split('.').map(|p| p.parse().unwrap_or(0)).collect();
        nums.resize(3, 0);
        (nums, pre)
    }
    let (a, a_pre) = parse(candidate);
    let (b, b_pre) = parse(current);
    match a.cmp(&b) {
        std::cmp::Ordering::Greater => true,
        std::cmp::Ordering::Less => false,
        std::cmp::Ordering::Equal => !a_pre && b_pre,
    }
}

fn agent() -> ureq::Agent {
    ureq::Agent::config_builder()
        .timeout_global(Some(Duration::from_secs(30)))
        .user_agent(concat!("NebulaSSH-Updater/", env!("CARGO_PKG_VERSION")))
        .https_only(false)
        .build()
        .into()
}

/// Prüft Signatur und Inhalt eines Manifests.
pub fn verify_manifest(text: &str, public_key_b64: &str) -> Result<Release, String> {
    let m: Manifest = serde_json::from_str(text).map_err(|_| t!("Update-Informationen sind beschädigt.", "Update information is damaged.").to_string())?;
    let key_bytes: [u8; 32] = B64
        .decode(public_key_b64.trim())
        .ok()
        .and_then(|b| b.try_into().ok())
        .ok_or("Interner Fehler: öffentlicher Update-Schlüssel ungültig.")?;
    let key = VerifyingKey::from_bytes(&key_bytes).map_err(|e| e.to_string())?;
    let sig_bytes: [u8; 64] = B64
        .decode(m.signature.trim())
        .ok()
        .and_then(|b| b.try_into().ok())
        .ok_or(t!("Update-Signatur fehlt oder ist ungültig.", "Update signature missing or invalid."))?;
    key.verify(m.payload.as_bytes(), &Signature::from_bytes(&sig_bytes))
        .map_err(|_| t!("Update-Signatur stimmt nicht – Update wird aus Sicherheitsgründen ignoriert.", "Update signature does not match – the update is ignored for security reasons.").to_string())?;
    let r: Release = serde_json::from_str(&m.payload).map_err(|e| e.to_string())?;
    for a in [&r.installer, &r.portable] {
        if a.sha256.len() != 64 || !a.sha256.chars().all(|c| c.is_ascii_hexdigit()) {
            return Err(t!("Update-Prüfsumme ist ungültig.", "Update checksum is invalid.").into());
        }
    }
    Ok(r)
}

/// GET mit Wiederholung bei vorübergehenden Server-Fehlern (z.B. GitHub 502/503/504).
fn get_retry(url: &str) -> Result<ureq::http::Response<ureq::Body>, ureq::Error> {
    let mut last = None;
    for attempt in 0..4u64 {
        if attempt > 0 {
            std::thread::sleep(Duration::from_millis(800 * attempt * attempt));
        }
        match agent().get(url).call() {
            Ok(r) => return Ok(r),
            Err(ureq::Error::StatusCode(code)) if (500..600).contains(&code) || code == 429 => {
                last = Some(ureq::Error::StatusCode(code));
            }
            Err(ureq::Error::Io(e)) => last = Some(ureq::Error::Io(e)),
            Err(ureq::Error::Timeout(t)) => last = Some(ureq::Error::Timeout(t)),
            Err(e) => return Err(e),
        }
    }
    Err(last.expect("mindestens ein Versuch"))
}

/// Lädt und prüft das Manifest. `Ok(None)` = schon aktuell.
pub fn check(url: &str, current: &str, public_key_b64: &str) -> Result<Option<Release>, String> {
    let text = get_retry(url)
        .map_err(|e| format!("Update-Server nicht erreichbar: {e}"))?
        .body_mut()
        .with_config()
        .limit(1024 * 1024)
        .read_to_string()
        .map_err(|e| format!("Update-Informationen nicht lesbar: {e}"))?;
    let r = verify_manifest(&text, public_key_b64)?;
    Ok(is_newer(&r.version, current).then_some(r))
}

/// Fortschritt eines Downloads (für die Anzeige).
#[derive(Default)]
pub struct DownloadProgress {
    pub done: AtomicU64,
    pub total: AtomicU64,
    pub cancel: AtomicBool,
}

/// Lädt eine Datei herunter und prüft die SHA-256-Prüfsumme.
/// Erst nach erfolgreicher Prüfung erhält die Datei ihren endgültigen Namen.
pub fn download(asset: &Asset, target: &Path, progress: Arc<DownloadProgress>) -> Result<PathBuf, String> {
    let mut resp = get_retry(&asset.url).map_err(|e| format!("Download fehlgeschlagen: {e}"))?;
    progress.total.store(asset.size, Ordering::Relaxed);
    let part = target.with_extension("part");
    let mut file = std::fs::File::create(&part).map_err(|e| e.to_string())?;
    let mut reader = resp.body_mut().with_config().limit(500 * 1024 * 1024).reader();
    let mut hasher = Sha256::new();
    let mut buf = vec![0u8; 64 * 1024];
    loop {
        if progress.cancel.load(Ordering::Relaxed) {
            drop(file);
            let _ = std::fs::remove_file(&part);
            return Err("Abgebrochen".into());
        }
        let n = reader.read(&mut buf).map_err(|e| format!("Download unterbrochen: {e}"))?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
        file.write_all(&buf[..n]).map_err(|e| e.to_string())?;
        progress.done.fetch_add(n as u64, Ordering::Relaxed);
    }
    file.flush().map_err(|e| e.to_string())?;
    drop(file);
    let digest: String = hasher.finalize().iter().map(|b| format!("{b:02x}")).collect();
    if !digest.eq_ignore_ascii_case(&asset.sha256) {
        let _ = std::fs::remove_file(&part);
        return Err(t!("Die heruntergeladene Datei ist beschädigt oder manipuliert (Prüfsumme stimmt nicht). Update abgebrochen.", "The downloaded file is damaged or tampered with (checksum mismatch). Update cancelled.").into());
    }
    let _ = std::fs::remove_file(target);
    std::fs::rename(&part, target).map_err(|e| e.to_string())?;
    Ok(target.to_path_buf())
}

/// Läuft die App aus "Programme" (installiert) oder portabel?
pub fn is_installed() -> bool {
    let Ok(exe) = std::env::current_exe() else { return false };
    let exe = exe.to_string_lossy().to_lowercase();
    ["ProgramFiles", "ProgramFiles(x86)", "ProgramW6432"].iter().any(|v| {
        std::env::var_os(v).is_some_and(|pf| exe.starts_with(&pf.to_string_lossy().to_lowercase()))
    })
}

/// Portable Version: neue EXE an die Stelle der laufenden setzen und neu starten.
/// Windows erlaubt das Umbenennen einer laufenden EXE, aber nicht das Überschreiben.
pub fn replace_running_exe(new_exe: &Path) -> Result<(), String> {
    let current = std::env::current_exe().map_err(|e| e.to_string())?;
    let old = current.with_extension("exe.old");
    let _ = std::fs::remove_file(&old);
    std::fs::rename(&current, &old).map_err(|e| format!("Konnte alte Version nicht umbenennen: {e}"))?;
    if let Err(e) = std::fs::copy(new_exe, &current) {
        // Zurückrollen
        let _ = std::fs::rename(&old, &current);
        return Err(format!("Konnte neue Version nicht schreiben: {e}"));
    }
    let _ = std::fs::remove_file(new_exe);
    std::process::Command::new(&current)
        .spawn()
        .map_err(|e| format!("Neustart fehlgeschlagen: {e}"))?;
    Ok(())
}

/// Reste eines vorherigen Selbst-Updates entfernen.
pub fn cleanup_old() {
    if let Ok(exe) = std::env::current_exe() {
        let _ = std::fs::remove_file(exe.with_extension("exe.old"));
    }
}

/// Installierte Version: Installer still mit Neustart-Option starten (fragt per UAC nach Adminrechten).
pub fn run_installer(installer: &Path) -> Result<(), String> {
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        use windows_sys::Win32::UI::Shell::ShellExecuteW;
        use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
        let wide = |s: &std::ffi::OsStr| s.encode_wide().chain(std::iter::once(0)).collect::<Vec<u16>>();
        let verb = wide(std::ffi::OsStr::new("runas"));
        let file = wide(installer.as_os_str());
        let params = wide(std::ffi::OsStr::new("/S /RELAUNCH"));
        // SAFETY: alle Zeiger zeigen auf nullterminierte, lebende Puffer.
        let r = unsafe {
            ShellExecuteW(std::ptr::null_mut(), verb.as_ptr(), file.as_ptr(), params.as_ptr(), std::ptr::null(), SW_SHOWNORMAL)
        };
        if (r as isize) <= 32 {
            return Err("Der Installer konnte nicht gestartet werden (Administratorrechte abgelehnt?).".into());
        }
        Ok(())
    }
    #[cfg(not(windows))]
    {
        let _ = installer;
        Err("Nur unter Windows verfügbar".into())
    }
}

/// Signiert eine Release-Beschreibung (für das Release-Werkzeug).
pub fn sign_release(release: &Release, signing_key: &ed25519_dalek::SigningKey) -> String {
    use ed25519_dalek::Signer;
    let payload = serde_json::to_string_pretty(release).expect("Release serialisierbar");
    let sig = signing_key.sign(payload.as_bytes());
    serde_json::to_string_pretty(&Manifest {
        payload,
        signature: B64.encode(sig.to_bytes()),
    })
    .expect("Manifest serialisierbar")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn release() -> Release {
        let a = Asset { url: "http://x/a".into(), sha256: "a".repeat(64), size: 1 };
        Release { version: "9.9.9".into(), date: "2026-11-08".into(), notes: "Test".into(), installer: a.clone(), portable: a, critical: false }
    }

    /// Prüft das echte veröffentlichte Release: Signatur + Download beider Dateien + Prüfsummen.
    /// NEBULA_UPDATE_URL=... cargo test -p nebula-core live_release -- --ignored --nocapture
    #[test]
    #[ignore]
    fn live_release() {
        let url = std::env::var("NEBULA_UPDATE_URL").unwrap();
        let key = include_str!("../../native/update_public_key.txt");
        let r = check(&url, "0.0.1", key).unwrap().expect("Release gefunden");
        println!("Release {} – Signatur gültig", r.version);
        for a in [&r.installer, &r.portable] {
            let t = std::env::temp_dir().join("nebula_live_check.bin");
            download(a, &t, Arc::new(DownloadProgress::default())).unwrap();
            println!("  {} – Prüfsumme stimmt ({} Bytes)", a.url, a.size);
            let _ = std::fs::remove_file(t);
        }
        // Eine aktuelle Installation soll nichts angeboten bekommen
        assert!(check(&url, &r.version, key).unwrap().is_none());
    }

    /// Prüft, dass GitHub-Weiterleitungen (latest/download -> Datei-Server) verfolgt werden.
    #[test]
    #[ignore]
    fn follows_github_redirects() {
        let asset = Asset {
            url: "https://github.com/cli/cli/releases/download/v2.63.0/gh_2.63.0_checksums.txt".into(),
            sha256: "0".repeat(64),
            size: 0,
        };
        let target = std::env::temp_dir().join("nebula_redirect_test.bin");
        let err = download(&asset, &target, Arc::new(DownloadProgress::default())).unwrap_err();
        // Datei wurde geladen (sonst "Download fehlgeschlagen"), nur die Prüfsumme passt absichtlich nicht
        assert!(err.contains("Prüfsumme"), "{err}");
    }

    #[test]
    fn versions() {
        assert!(is_newer("2.1.1", "2.1.0"));
        assert!(is_newer("2.10.0", "2.9.9"));
        assert!(is_newer("v3.0", "2.99.99"));
        assert!(!is_newer("2.1.0", "2.1.0"));
        assert!(!is_newer("2.0.9", "2.1.0"));
        assert!(is_newer("2.2.0", "2.2.0-beta"));
        assert!(!is_newer("2.2.0-beta", "2.2.0"));
    }

    #[test]
    fn signature_is_enforced() {
        let sk = ed25519_dalek::SigningKey::from_bytes(&[7u8; 32]);
        let pk = B64.encode(sk.verifying_key().to_bytes());
        let text = sign_release(&release(), &sk);
        assert_eq!(verify_manifest(&text, &pk).unwrap(), release());

        // Manipulierte Version / URL -> abgelehnt
        let mut m: Manifest = serde_json::from_str(&text).unwrap();
        m.payload = m.payload.replace("http://x/a", "http://boese/a");
        let bad = serde_json::to_string(&m).unwrap();
        assert!(verify_manifest(&bad, &pk).is_err());

        // Fremder Schlüssel -> abgelehnt
        let other = ed25519_dalek::SigningKey::from_bytes(&[9u8; 32]);
        assert!(verify_manifest(&sign_release(&release(), &other), &pk).is_err());
    }
}
