# NebulaSSH 🚀

[![Website](https://img.shields.io/badge/Website-nebulassh.schoepf--tirol.at-blue?style=for-the-badge&logo=google-chrome)](https://nebulassh.schoepf-tirol.at/)
[![Made with Wails](https://img.shields.io/badge/Wails-Go_%2B_Svelte-red?style=for-the-badge&logo=go)](https://wails.io/)

**NebulaSSH** ist ein moderner, pfeilschneller und plattformübergreifender Terminal-Client für SSH- und serielle (COM) Verbindungen. Entwickelt für Administratoren, Netzwerker und Maker, die ein aufgeräumtes und effizientes Werkzeug für ihren Alltag brauchen.

![NebulaSSH Screenshot](https://nebulassh.schoepf-tirol.at/screenshot.png)

🌐 **[Offizielle Website besuchen](https://nebulassh.schoepf-tirol.at/)**

---

## ✨ Features

* 💻 **Multi-Protokoll Unterstützung:** Nahtloser Wechsel zwischen SSH-Verbindungen (Netzwerk) und seriellen Verbindungen (COM-Ports/USB).
* 📑 **Tab-System:** Mehrere parallele Sitzungen gleichzeitig offen halten und blitzschnell zwischen ihnen wechseln.
* ⚡ **Makros & Snippets:** Häufig genutzte Befehle (z.B. Updates, Reboots) als Buttons speichern und mit einem Klick ausführen.
* 🔍 **Live-Suche (Strg + F):** Durchsuche Terminal-Outputs (bis zu 50.000 Zeilen Scrollback) in Echtzeit mit farbigem Highlighting.
* 📋 **Smart Copy & Paste:** Markierter Text wird sofort automatisch kopiert (ohne Fokusverlust) und kann per Rechtsklick eingefügt werden. Auch Passwörter lassen sich mit einem Klick in die Zwischenablage befördern.
* 💾 **Geräte-Manager:** Speichere Server, Router und Switches mit IP und Benutzername für schnellen Zugriff.
* 🔌 **Baudraten-Profile:** Eigene Baudraten für spezielle serielle Hardware anlegen und verwalten.

## 🛠️ Tech-Stack

NebulaSSH wurde gebaut mit:
* **[Wails](https://wails.io/)** - Das Framework für Desktop-Apps mit Go & Web-Technologien.
* **[Go (Golang)](https://go.dev/)** - Für ein rasend schnelles, ressourcenschonendes Backend (SSH & Serial Handling).
* **[Svelte](https://svelte.dev/)** - Für eine reaktive, flüssige und moderne Benutzeroberfläche.
* **[xterm.js](https://xtermjs.org/)** - Der Industrie-Standard für Terminal-Emulation im Web.

## 🚀 Installation & Entwicklung

Voraussetzungen: [Go](https://go.dev/), [Node.js](https://nodejs.org/) und [Wails CLI](https://wails.io/docs/gettingstarted/installation).

**1. Repository klonen:**
```bash
git clone [https://github.com/Zenutrix/NebulaSSH.git](https://github.com/Zenutrix/NebulaSSH.git)
cd NebulaSSH