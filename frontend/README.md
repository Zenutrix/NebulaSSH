# NebulaSSH 🌌

> **The Professional Open Source Terminal Experience.**

NebulaSSH ist ein portabler, hochperformanter Terminal-Emulator, der SSH, SFTP und Serial Console in einem einzigen, schlanken Tool vereint. Entwickelt mit Fokus auf Sicherheit, Geschwindigkeit und reibungslose Workflows für Systemadministratoren und Hardware-Entwickler.

<!-- Ersetze dies ggf. durch einen echten GitHub-Link zum Bild -->

## ✨ Features

* **SSH Terminal (powered by xterm.js):** Ultraschnelles Rendering, Multi-Tab-Support und Live-Suche (`Strg + F`).

* **Integrierter SFTP-Browser:** Dateien direkt auf dem Server verwalten, hochladen, herunterladen und mit dem internen Editor bearbeiten.

* **Serial Console:** Direkter Zugriff auf COM-Ports inkl. anpassbarer Baudraten-Profile (perfekt für Cisco, Arduino, Raspberry Pi).

* **SSH Key Manager:** Sichere Verwaltung von `.pem` und `id_rsa` Dateien.

* **Smart Macros:** Komplexe Befehlsketten als Snippets speichern und mit einem Klick ausführen.

* **Zero-Knowledge Architektur:** Vollständig lokale, AES-256-verschlüsselte Speicherung aller Credentials.

## 🛡️ Sicherheit & Architektur

NebulaSSH speichert Zugangsdaten **niemals im Klartext**.
Die Dateien `hosts.json` und `keys.json` werden mithilfe des **Go AES-256-GCM** Algorithmus verschlüsselt. Der dafür notwendige Master-Key wird sicher im nativen **System-Keyring** (Windows Credential Manager / macOS Keychain / Linux Secret Service) abgelegt.

Es gibt keinen Cloud-Sync und keine Telemetrie. Deine Daten verlassen niemals deinen Rechner.

## 🛠️ Tech Stack

NebulaSSH ist eine Desktop-Anwendung, die auf modernen Web-Technologien und Go basiert:

* **Backend:** [Go](https://go.dev/) + [Wails](https://wails.io/)

* **Frontend:** [Svelte](https://svelte.dev/) + [Vite](https://vitejs.dev/)

* **Styling:** [Tailwind CSS](https://tailwindcss.com/)

* **Terminal Engine:** [Xterm.js](https://xtermjs.org/)

## 🚀 Entwicklung & Setup

Da NebulaSSH auf dem Wails-Framework aufbaut, weicht der Workflow leicht von einem Standard-Vite-Projekt ab.

### Voraussetzungen

1. [Go](https://go.dev/doc/install) (1.18+)

2. [Node.js](https://nodejs.org/en/download/) (16+)

3. [Wails CLI](https://wails.io/docs/gettingstarted/installation)

Installiere die Wails CLI:

```bash
go install [github.com/wailsapp/wails/v2/cmd/wails@latest](https://github.com/wailsapp/wails/v2/cmd/wails@latest)