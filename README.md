# NebulaSSH 2.3

Nativer SSH-, SFTP-, Telnet- und Serial-Client für Windows – komplett in **Rust**, ohne WebView.
Oberfläche auf Deutsch und Englisch.

## Funktionen

- SSH mit Host-Key-Prüfung, Passwort, Key oder SSH-Agent (Pageant, Windows-OpenSSH-Agent, 1Password, KeePassXC)
- Jump-Host (ProxyJump), Port-Weiterleitung (-L, -R, SOCKS -D), Agent- und X11-Weiterleitung
- Telnet und serielle COM-Ports
- SFTP-Explorer mit Editor, Drag & Drop, Ordnervergleich/Synchronisation und Übertragungs-Warteschlange
- Geteilte Ansicht (bis zu 4 Terminals), Tabs als eigene Fenster
- Snippets mit Platzhaltern, Broadcast an alle Tabs, Server-Auslastung in der Statusleiste
- Import aus PuTTY, MobaXterm und `~/.ssh/config`; Wake-on-LAN, Ping und Port-Prüfung
- Verschlüsselte Ablage, App-Sperre, verschlüsseltes Backup, signierte Auto-Updates
by Thomas Schöpf

## Projektstruktur

| Ordner | Inhalt |
|---|---|
| `core/` | Kern-Bibliothek: SSH inkl. Tunnel/Jump/Agent/X11 (`ssh.rs`), Telnet (`telnet.rs`), Serial (`serial.rs`), Import (`import.rs`), Server-Kennzahlen (`stats.rs`), Wake-on-LAN/Ping (`net.rs`), Sprache (`i18n.rs`), verschlüsselte Ablage (`storage.rs`), SSH-Key-Generator (`keys.rs`), Backup & App-Sperre (`backup.rs`) |
| `native/` | Die Windows-App (egui + alacritty_terminal): `app.rs` (Logik, geteilte Ansicht, eigene Fenster), `terminal.rs`, `sidebar.rs`, `files.rs` (SFTP-Explorer, Warteschlange), `sync.rs`, `importer.rs`, `snippets.rs` (Snippets, Tunnel-Dialog), `dialogs.rs`, `theme.rs` |
| `branding/` | Logo (`logo.svg`) und Installer-Grafiken; `cargo run -p brand` erzeugt daraus `branding/out/` |
| `installer/` | NSIS-Skript (`nebulassh.nsi`) und Build-Skript (`build.ps1`) |
| `website/` | Produkt-Website mit echten Screenshots und Downloads |
| `legacy-go/`, `legacy-tauri/` | Ältere Versionen (Go/Wails 1.x, Tauri 2.0) zum Nachschlagen |

## Bauen

Voraussetzungen: Rust (MSVC-Toolchain), NSIS (wird unter `%LOCALAPPDATA%\tauri\NSIS` oder `Programme\NSIS` gesucht).

```powershell
powershell -ExecutionPolicy Bypass -File installer\build.ps1
```

Ergebnis in `dist/`:
- `NebulaSSH-Setup-<version>.exe` – Installer (installiert nach `C:\Program Files\NebulaSSH`, Startmenü, optional Desktop, Eintrag in „Apps & Features“)
- `NebulaSSH-<version>-portable.exe` – einzelne EXE

Nur die App: `cargo build -p nebulassh --release` → `target/release/nebulassh.exe`

## Updates über GitHub-Releases

Die App sucht beim Start unter
`https://github.com/Zenutrix/NebulaSSH/releases/latest/download/update.json` nach einer neuen Version
(Repository steht in `installer/github-repo.txt`). Ein Update wird nur installiert, wenn diese Datei mit dem
geheimen Signierschlüssel (Ed25519) signiert ist und die heruntergeladene Datei die signierte SHA-256-Prüfsumme hat.
Installierte Versionen starten den Installer still (`/S /RELAUNCH`, mit UAC-Abfrage), die portable Version ersetzt sich selbst.

- **Geheimer Signierschlüssel:** `%USERPROFILE%\.nebulassh-release\update-signing.key` – liegt bewusst außerhalb des Projekts und wird **nie** hochgeladen. **Unbedingt sichern!** Ohne ihn kann keine installierte Version mehr Updates erhalten.
- **Öffentlicher Schlüssel:** `native/update_public_key.txt` (wird in die App eingebaut)

### Neue Version veröffentlichen

1. Version in `native/Cargo.toml` erhöhen, Änderungen in `installer/release-notes.txt` eintragen.
2. Bauen:
   ```powershell
   powershell -ExecutionPolicy Bypass -File installer\build.ps1
   ```
3. Auf GitHub ein neues Release mit dem Tag `v<version>` (z.B. `v2.2.0`) anlegen und **alle Dateien aus `dist\github-release\`** hochladen:
   `NebulaSSH-Setup-<version>.exe`, `NebulaSSH-<version>-portable.exe`, `NebulaSSH-Setup.exe`, `NebulaSSH-portable.exe`, `update.json`, `SHA256SUMS.txt`.
   Das Release muss als **Latest** markiert sein (kein Entwurf, keine Vorabversion).

Alternativ automatisch per Token (GitHub → Settings → Developer settings → Fine-grained token, nur dieses Repository, „Contents: Read and write“):
```powershell
$env:GITHUB_TOKEN = "<Token>"
powershell -ExecutionPolicy Bypass -File installer\build.ps1 -Publish
```

Die Website verlinkt auf `…/releases/latest/download/NebulaSSH-Setup.exe` und zeigt damit immer die neueste Version.
Sicherheitsupdates: `cargo run -p release -- manifest <version> <url> <notizen> --critical` öffnet den Update-Dialog sofort beim Start.

## Tests

```bash
cargo test                       # Unit-Tests (Kern + App)
# Integrationstests gegen einen echten SSH-Server, z.B.:
# docker run -d -p 127.0.0.1:2222:2222 -e PASSWORD_ACCESS=true -e USER_NAME=nebula -e USER_PASSWORD=nebulatest lscr.io/linuxserver/openssh-server
# (für Tunnel-Tests: -e DOCKER_MODS=linuxserver/mods:openssh-server-ssh-tunnel)
NEBULA_TEST_SSH="127.0.0.1:2222|nebula|nebulatest" cargo test -p nebula-core -- --ignored --test-threads=1
```

## Daten

- **Installiert:** `%APPDATA%\NebulaSSH` – **Portable:** neben der EXE (falls beschreibbar)
- `hosts.json`, `keys.json`, `sessions.json`: AES-256-GCM-verschlüsselt, Schlüssel in der Windows-Anmeldeinformationsverwaltung (`NebulaSSH:encryption_key`, kompatibel zu 1.x)
- `settings.json` (Einstellungen, Snippets), `known_hosts.json` (Host-Fingerprints), `logs/` (optional)
- Umzug auf einen neuen PC: Einstellungen → Daten & Backup → Backup erstellen (`.nebula`, Argon2id + AES-256-GCM)

## Tastenkürzel

| Kürzel | Aktion |
|---|---|
| Strg+Umschalt+T | Neue Verbindung |
| Strg+Tab / Strg+Umschalt+Tab | Tab wechseln |
| Strg+Umschalt+W | Tab schließen |
| Strg+Umschalt+D | Tab duplizieren |
| Strg+Umschalt+R | Neu verbinden |
| Strg+Umschalt+E | Datei-Explorer |
| Strg+Umschalt+B | Seitenleiste ein/aus |
| Strg+Umschalt+P | Snippets (Befehlsbibliothek) |
| Strg+Umschalt+S | Geteilte Ansicht ein/aus |
| Strg+Umschalt+O | Tab in eigenem Fenster |
| Strg+F | Im Terminal suchen |
| Strg + / − / 0 | Schriftgröße |
| Umschalt+Bild↑/↓, Umschalt+Pos1/Ende | Im Verlauf scrollen |
