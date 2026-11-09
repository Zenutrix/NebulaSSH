//! Release-Werkzeug für NebulaSSH-Updates.
//!
//!   cargo run -p release -- keygen
//!       Erzeugt den geheimen Signierschlüssel (einmalig!) außerhalb des Projekts
//!       und schreibt den öffentlichen Schlüssel nach native/update_public_key.txt.
//!
//!   cargo run -p release -- manifest <version> <basis-url> [notizen.txt] [--critical]
//!       Liest dist/NebulaSSH-Setup-<version>.exe und dist/NebulaSSH-<version>-portable.exe,
//!       berechnet Prüfsummen und schreibt die signierte dist/update.json.

use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine;
use ed25519_dalek::SigningKey;
use nebula_core::update::{sign_release, Asset, Release};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// Der geheime Schlüssel liegt bewusst NICHT im Projektordner.
fn key_path() -> PathBuf {
    if let Ok(p) = std::env::var("NEBULA_SIGNING_KEY") {
        return p.into();
    }
    let home = std::env::var_os("USERPROFILE").or_else(|| std::env::var_os("HOME")).expect("Benutzerordner");
    PathBuf::from(home).join(".nebulassh-release").join("update-signing.key")
}

fn fail(msg: &str) -> ! {
    eprintln!("Fehler: {msg}");
    std::process::exit(1)
}

fn load_key() -> SigningKey {
    let p = key_path();
    let text = std::fs::read_to_string(&p).unwrap_or_else(|_| {
        fail(&format!("Kein Signierschlüssel unter {} – zuerst `cargo run -p release -- keygen` ausführen.", p.display()))
    });
    let bytes: [u8; 32] = B64
        .decode(text.trim())
        .ok()
        .and_then(|b| b.try_into().ok())
        .unwrap_or_else(|| fail("Signierschlüssel ist beschädigt."));
    SigningKey::from_bytes(&bytes)
}

fn keygen() {
    let p = key_path();
    if p.exists() {
        fail(&format!(
            "Es gibt bereits einen Signierschlüssel ({}).\nEin neuer Schlüssel würde alle bereits installierten Versionen von Updates ausschließen.",
            p.display()
        ));
    }
    let key = SigningKey::generate(&mut rand::rngs::OsRng);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(&p, B64.encode(key.to_bytes())).unwrap();
    let public = B64.encode(key.verifying_key().to_bytes());
    std::fs::write(root().join("native/update_public_key.txt"), format!("{public}\n")).unwrap();
    println!("Geheimer Signierschlüssel: {}", p.display());
    println!("  -> SICHER AUFBEWAHREN (z.B. im Passwort-Manager). Ohne ihn können keine Updates mehr veröffentlicht werden.");
    println!("Öffentlicher Schlüssel (in die App eingebaut): {public}");
}

fn asset(file: &Path, base_url: &str) -> Asset {
    let data = std::fs::read(file).unwrap_or_else(|_| fail(&format!("Datei fehlt: {}", file.display())));
    let sha256 = Sha256::digest(&data).iter().map(|b| format!("{b:02x}")).collect();
    let name = file.file_name().unwrap().to_string_lossy();
    Asset {
        url: format!("{}/{}", base_url.trim_end_matches('/'), name),
        sha256,
        size: data.len() as u64,
    }
}

fn manifest(args: &[String]) {
    let critical = args.iter().any(|a| a == "--critical");
    let args: Vec<&String> = args.iter().filter(|a| *a != "--critical").collect();
    let (Some(version), Some(base_url)) = (args.first(), args.get(1)) else {
        fail("Aufruf: manifest <version> <basis-url> [notizen.txt] [--critical]")
    };
    let notes = match args.get(2) {
        Some(f) => std::fs::read_to_string(f).unwrap_or_else(|_| fail("Notizen-Datei nicht lesbar")),
        None => format!("NebulaSSH {version}"),
    };
    let dist = root().join("dist");
    let release = Release {
        version: version.to_string(),
        date: chrono::Local::now().format("%Y-%m-%d").to_string(),
        notes: notes.trim().to_string(),
        installer: asset(&dist.join(format!("NebulaSSH-Setup-{version}.exe")), base_url),
        portable: asset(&dist.join(format!("NebulaSSH-{version}-portable.exe")), base_url),
        critical,
    };
    let key = load_key();
    // Sicherheitsnetz: passt der Schlüssel zur App?
    let embedded = std::fs::read_to_string(root().join("native/update_public_key.txt")).unwrap_or_default();
    if embedded.trim() != B64.encode(key.verifying_key().to_bytes()) {
        fail("Der Signierschlüssel passt nicht zum öffentlichen Schlüssel in native/update_public_key.txt.");
    }
    let out = dist.join("update.json");
    std::fs::write(&out, sign_release(&release, &key)).unwrap();
    println!("Signiertes Update-Manifest: {}", out.display());
    println!("  Version {} · Installer {} · Portable {}", release.version, release.installer.url, release.portable.url);
}

/// Legt auf GitHub das Release `v<version>` an und lädt alle Dateien hoch.
/// Benötigt ein Token in der Umgebungsvariablen GITHUB_TOKEN
/// (Fine-grained Token mit "Contents: Read and write" für das Repository).
fn github(args: &[String]) {
    let (Some(version), Some(repo)) = (args.first(), args.get(1)) else {
        fail("Aufruf: github <version> <konto/repository> [notizen.txt]")
    };
    let token = std::env::var("GITHUB_TOKEN")
        .unwrap_or_else(|_| fail("Bitte zuerst die Umgebungsvariable GITHUB_TOKEN setzen."));
    let notes = match args.get(2) {
        Some(f) => std::fs::read_to_string(f).unwrap_or_else(|_| fail("Notizen-Datei nicht lesbar")),
        None => format!("NebulaSSH {version}"),
    };
    let dist = root().join("dist");
    let manifest = dist.join("update.json");
    let text = std::fs::read_to_string(&manifest)
        .unwrap_or_else(|_| fail("dist/update.json fehlt – zuerst `manifest` ausführen."));
    // Sicherheitsnetz: passt das Manifest zu dieser Version und diesem Repository?
    let embedded = std::fs::read_to_string(root().join("native/update_public_key.txt")).unwrap_or_default();
    let release = nebula_core::update::verify_manifest(&text, embedded.trim())
        .unwrap_or_else(|e| fail(&format!("update.json ungültig: {e}")));
    if release.version != *version {
        fail(&format!("update.json gehört zu Version {}, nicht {version}.", release.version));
    }
    let expected = format!("https://github.com/{repo}/releases/download/v{version}/");
    if !release.installer.url.starts_with(&expected) {
        fail(&format!("Die Download-Adressen in update.json zeigen nicht auf {expected}"));
    }

    let agent: ureq::Agent = ureq::Agent::config_builder()
        .http_status_as_error(false)
        .user_agent("NebulaSSH-Release")
        .build()
        .into();
    let auth = format!("Bearer {token}");
    let body = serde_json::json!({
        "tag_name": format!("v{version}"),
        "name": format!("NebulaSSH {version}"),
        "body": notes.lines().map(|l| format!("- {}", l.trim_start_matches(['-', '*', ' ']))).collect::<Vec<_>>().join("\n"),
        "draft": false,
        "prerelease": version.contains('-'),
        "make_latest": "true",
    });
    let mut resp = agent
        .post(&format!("https://api.github.com/repos/{repo}/releases"))
        .header("Authorization", &auth)
        .header("Accept", "application/vnd.github+json")
        .header("X-GitHub-Api-Version", "2022-11-28")
        .send(body.to_string())
        .unwrap_or_else(|e| fail(&format!("GitHub nicht erreichbar: {e}")));
    let status = resp.status().as_u16();
    let text = resp.body_mut().read_to_string().unwrap_or_default();
    if status != 201 {
        fail(&format!("Release konnte nicht angelegt werden (HTTP {status}): {text}"));
    }
    let created: serde_json::Value = serde_json::from_str(&text).unwrap();
    let id = created["id"].as_u64().unwrap();
    println!("Release v{version} angelegt: {}", created["html_url"].as_str().unwrap_or(""));

    let setup = format!("NebulaSSH-Setup-{version}.exe");
    let portable = format!("NebulaSSH-{version}-portable.exe");
    // Versionierte Dateien (für das Update) + feste Namen (für Download-Links der Website)
    let uploads: Vec<(PathBuf, String)> = vec![
        (dist.join(&setup), setup.clone()),
        (dist.join(&portable), portable.clone()),
        (dist.join(&setup), "NebulaSSH-Setup.exe".into()),
        (dist.join(&portable), "NebulaSSH-portable.exe".into()),
        (manifest.clone(), "update.json".into()),
    ];
    let mut sums = String::new();
    for (path, name) in &uploads {
        let data = std::fs::read(path).unwrap_or_else(|_| fail(&format!("Datei fehlt: {}", path.display())));
        if name.ends_with(".exe") {
            let h: String = Sha256::digest(&data).iter().map(|b| format!("{b:02x}")).collect();
            sums.push_str(&format!("{h}  {name}\n"));
        }
        upload(&agent, &auth, repo, id, name, data);
    }
    upload(&agent, &auth, repo, id, "SHA256SUMS.txt", sums.into_bytes());
    println!("Fertig. Installierte NebulaSSH-Versionen finden das Update ab sofort.");
}

fn upload(agent: &ureq::Agent, auth: &str, repo: &str, id: u64, name: &str, data: Vec<u8>) {
    print!("  lade {name} hoch ({:.1} MB)… ", data.len() as f64 / 1_048_576.0);
    let mut resp = agent
        .post(&format!("https://uploads.github.com/repos/{repo}/releases/{id}/assets?name={name}"))
        .header("Authorization", auth)
        .header("Accept", "application/vnd.github+json")
        .header("Content-Type", "application/octet-stream")
        .send(data)
        .unwrap_or_else(|e| fail(&format!("Upload fehlgeschlagen: {e}")));
    let status = resp.status().as_u16();
    if status != 201 {
        let t = resp.body_mut().read_to_string().unwrap_or_default();
        fail(&format!("Upload von {name} fehlgeschlagen (HTTP {status}): {t}"));
    }
    println!("ok");
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("keygen") => keygen(),
        Some("manifest") => manifest(&args[1..]),
        Some("github") => github(&args[1..]),
        Some("pubkey") => println!("{}", B64.encode(load_key().verifying_key().to_bytes())),
        _ => {
            eprintln!("NebulaSSH Release-Werkzeug\n  keygen                                   Signierschlüssel einmalig erzeugen\n  manifest <version> <basis-url> [notizen] [--critical]\n  github <version> <konto/repo> [notizen]   Release auf GitHub veröffentlichen (GITHUB_TOKEN)
  pubkey                                   öffentlichen Schlüssel anzeigen");
            std::process::exit(2);
        }
    }
}
