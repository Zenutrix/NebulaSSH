//! Update-Prüfung und Selbst-Update in der Oberfläche.

use crate::app::*;
use eframe::egui::{self, vec2, Align, Color32, Layout, RichText, Ui};
use nebula_core::update::{self, DownloadProgress, Release};
use nebula_core::{t, tf};
use rfd::MessageLevel;
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Instant;

/// Öffentlicher Schlüssel, mit dem Updates signiert sein müssen (siehe tools/release).
pub const PUBLIC_KEY: &str = include_str!("../update_public_key.txt");
/// Standard-Adresse der update.json (leer = nur über die Einstellungen festgelegt).
const DEFAULT_URL: &str = include_str!("../update_url.txt");

pub enum UpdateState {
    Idle,
    Checking { manual: bool },
    UpToDate(Instant),
    Available(Release),
    Downloading { release: Release, progress: Arc<DownloadProgress> },
    Error(String),
}

pub fn update_url(prefs: &crate::data::Prefs) -> String {
    let custom = prefs.update_url.trim();
    if custom.is_empty() {
        DEFAULT_URL.trim().to_string()
    } else {
        custom.to_string()
    }
}

impl App {
    pub fn check_updates(&mut self, manual: bool) {
        let url = update_url(&self.prefs);
        if url.is_empty() {
            if manual {
                self.update = UpdateState::Error(t!("Keine Update-Adresse eingestellt.", "No update address configured.").into());
            }
            return;
        }
        if matches!(self.update, UpdateState::Checking { .. } | UpdateState::Downloading { .. }) {
            return;
        }
        self.update = UpdateState::Checking { manual };
        let tx = self.tx.clone();
        let ctx = self.ctx.clone();
        std::thread::spawn(move || {
            let res = update::check(&url, crate::app::VERSION, PUBLIC_KEY);
            let _ = tx.send(Msg::UpdateChecked(res, manual));
            ctx.request_repaint();
        });
    }

    pub fn on_update_checked(&mut self, res: Result<Option<Release>, String>, manual: bool) {
        match res {
            Ok(Some(r)) => {
                let skipped = !manual && !r.critical && self.prefs.skip_version == r.version;
                let show = manual || r.critical;
                self.update = UpdateState::Available(r);
                if skipped {
                    self.update = UpdateState::Idle;
                } else if show && self.dialog.is_none() {
                    self.dialog = Some(Dialog::Update);
                }
            }
            Ok(None) => {
                self.update = UpdateState::UpToDate(Instant::now());
                if manual {
                    self.notify(tf!("NebulaSSH {VERSION} ist aktuell.", "NebulaSSH {VERSION} is up to date."));
                }
            }
            Err(e) => {
                // Automatische Prüfung: Fehler (z.B. offline) still ignorieren
                self.update = if manual { UpdateState::Error(e) } else { UpdateState::Idle };
            }
        }
    }

    fn start_update_download(&mut self, release: Release) {
        let installed = update::is_installed();
        let asset = if installed { release.installer.clone() } else { release.portable.clone() };
        let progress = Arc::new(DownloadProgress::default());
        let name = if installed {
            format!("NebulaSSH-Setup-{}.exe", release.version)
        } else {
            format!("NebulaSSH-{}-update.exe", release.version)
        };
        // Installer in den Temp-Ordner; portable Version neben die EXE (gleiches Laufwerk)
        let target: PathBuf = if installed {
            std::env::temp_dir().join(name)
        } else {
            std::env::current_exe()
                .ok()
                .and_then(|p| p.parent().map(|d| d.join(&name)))
                .unwrap_or_else(|| std::env::temp_dir().join(&name))
        };
        let tx = self.tx.clone();
        let ctx = self.ctx.clone();
        let p2 = progress.clone();
        std::thread::spawn(move || {
            let res = update::download(&asset, &target, p2);
            let _ = tx.send(Msg::UpdateDownloaded(res, installed));
            ctx.request_repaint();
        });
        self.update = UpdateState::Downloading { release, progress };
    }

    pub fn on_update_downloaded(&mut self, res: Result<PathBuf, String>, installed: bool) {
        let release = match std::mem::replace(&mut self.update, UpdateState::Idle) {
            UpdateState::Downloading { release, .. } => release,
            other => {
                self.update = other;
                return;
            }
        };
        match res {
            Ok(file) => {
                let result = if installed { update::run_installer(&file) } else { update::replace_running_exe(&file) };
                match result {
                    Ok(()) => {
                        // Sitzungen sichern und beenden – die neue Version startet automatisch.
                        self.ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                    }
                    Err(e) => {
                        self.update = UpdateState::Available(release);
                        alert("Update", &e);
                    }
                }
            }
            Err(e) if e == "Abgebrochen" => self.update = UpdateState::Available(release),
            Err(e) => {
                self.update = UpdateState::Available(release);
                alert("Update", &e);
            }
        }
    }

    /// Kleiner Hinweis in der Statusleiste.
    pub fn update_badge(&mut self, ui: &mut Ui) {
        let label = match &self.update {
            UpdateState::Available(r) => tf!("Update {} verfügbar", "Update {} available", r.version),
            UpdateState::Downloading { progress, .. } => {
                let total = progress.total.load(Ordering::Relaxed).max(1);
                let pct = progress.done.load(Ordering::Relaxed) * 100 / total;
                ui.ctx().request_repaint_after(std::time::Duration::from_millis(200));
                format!("Update {pct} %")
            }
            _ => return,
        };
        let b = egui::Button::image_and_text(icons::image(ui.ctx(), Icon::ArrowUpCircle, 15.0, Color32::BLACK), RichText::new(label).strong().color(Color32::BLACK)).fill(AMBER);
        if ui.add(b).on_hover_text(t!("Details und Installation", "Details and installation")).clicked() {
            self.dialog = Some(Dialog::Update);
        }
    }

    /// Inhalt des Update-Dialogs. Gibt `keep` zurück.
    pub fn update_dialog(&mut self, ui: &mut Ui) -> bool {
        let mut keep = true;
        ui.set_width(520.0);
        let mut action: Option<u8> = None;
        match &self.update {
            UpdateState::Available(r) | UpdateState::Downloading { release: r, .. } => {
                ui.horizontal(|ui| {
                    ui.add(egui::Image::new(&self.logo).fit_to_exact_size(vec2(48.0, 48.0)));
                    ui.vertical(|ui| {
                        ui.label(RichText::new(tf!("NebulaSSH {} ist verfügbar", "NebulaSSH {} is available", r.version)).size(19.0).strong().color(Color32::WHITE));
                        ui.label(RichText::new(tf!("Installiert: {VERSION}  ·  veröffentlicht am {}", "Installed: {VERSION}  ·  released on {}", r.date)).color(MUTED));
                    });
                });
                if r.critical {
                    ui.add_space(8.0);
                    ui.label(RichText::new(t!("⚠ Wichtiges Sicherheitsupdate – bitte zeitnah installieren.", "⚠ Important security update – please install soon.")).color(AMBER).strong());
                }
                ui.add_space(12.0);
                ui.label(RichText::new(t!("Neu in dieser Version", "New in this version")).color(MUTED).strong().size(12.5));
                egui::Frame::new().fill(BG).corner_radius(8).inner_margin(12).show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    egui::ScrollArea::vertical().max_height(200.0).show(ui, |ui| {
                        for line in r.notes.lines().filter(|l| !l.trim().is_empty()) {
                            let l = line.trim().trim_start_matches(['-', '*', '•']).trim();
                            ui.horizontal_wrapped(|ui| {
                                ui.label(RichText::new("•").color(LAVENDER));
                                ui.label(l);
                            });
                        }
                    });
                });
                ui.add_space(10.0);
                let installed = update::is_installed();
                let size = if installed { r.installer.size } else { r.portable.size };
                ui.label(
                    RichText::new(if installed {
                        tf!("Der Installer ({}) wird geladen, geprüft und mit Administratorrechten gestartet. NebulaSSH startet danach automatisch neu.", "The installer ({}) is downloaded, verified and started with administrator rights. NebulaSSH restarts automatically afterwards.", human_size(size))
                    } else {
                        tf!("Die neue Version ({}) wird geladen, geprüft und ersetzt diese portable EXE. NebulaSSH startet danach neu.", "The new version ({}) is downloaded, verified and replaces this portable EXE. NebulaSSH restarts afterwards.", human_size(size))
                    })
                    .size(12.5)
                    .color(MUTED),
                );
                ui.add_space(12.0);
                if let UpdateState::Downloading { progress, .. } = &self.update {
                    let done = progress.done.load(Ordering::Relaxed);
                    let total = progress.total.load(Ordering::Relaxed).max(1);
                    ui.add(egui::ProgressBar::new(done as f32 / total as f32).fill(ACCENT_HI).text(format!("{} / {}", human_size(done), human_size(total))));
                    ui.ctx().request_repaint_after(std::time::Duration::from_millis(100));
                    ui.add_space(10.0);
                    ui.horizontal(|ui| {
                        if ui.button(t!("Abbrechen", "Cancel")).clicked() {
                            progress.cancel.store(true, Ordering::Relaxed);
                        }
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            if ui.button(t!("Im Hintergrund", "In background")).clicked() {
                                action = Some(3);
                            }
                        });
                    });
                } else {
                    ui.horizontal(|ui| {
                        if ui.button(t!("Diese Version überspringen", "Skip this version")).clicked() {
                            action = Some(2);
                        }
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            if ui
                                .add(egui::Button::new(RichText::new(t!("Jetzt aktualisieren", "Update now")).strong().color(Color32::WHITE)).fill(ACCENT).min_size(vec2(170.0, 34.0)))
                                .clicked()
                            {
                                action = Some(1);
                            }
                            if ui.add(egui::Button::new(t!("Später", "Later")).min_size(vec2(90.0, 34.0))).clicked() {
                                action = Some(3);
                            }
                        });
                    });
                }
            }
            _ => {
                ui.label(t!("Kein Update verfügbar.", "No update available."));
                if ui.button(t!("Schließen", "Close")).clicked() {
                    action = Some(3);
                }
            }
        }
        match action {
            Some(1) => {
                if let UpdateState::Available(r) = &self.update {
                    let open = self.tabs.iter().filter(|t| t.status == Status::Connected).count();
                    let ok = open == 0
                        || confirm(
                            t!("Update installieren", "Install update"),
                            &tf!(
                                "{open} Verbindung(en) sind noch offen und werden beim Neustart getrennt.\n\nJetzt aktualisieren?",
                                "{open} connection(s) are still open and will be closed on restart.\n\nUpdate now?"
                            ),
                            MessageLevel::Info,
                        );
                    if ok {
                        let r = r.clone();
                        self.start_update_download(r);
                    }
                }
            }
            Some(2) => {
                if let UpdateState::Available(r) = &self.update {
                    self.prefs.skip_version = r.version.clone();
                    self.save_settings();
                }
                self.update = UpdateState::Idle;
                keep = false;
            }
            Some(3) => keep = false,
            _ => {}
        }
        if ui.input(|i| i.key_pressed(egui::Key::Escape)) && !matches!(self.update, UpdateState::Downloading { .. }) {
            keep = false;
        }
        keep
    }

    /// Bereich auf der Einstellungsseite "Über".
    pub fn update_settings(&mut self, ui: &mut Ui, prefs: &mut crate::data::Prefs) {
        use crate::dialogs::{divider, group, row, switch};
        group(ui, "Updates", |ui| {
            row(ui, t!("Automatisch suchen", "Check automatically"), t!("Beim Start nach einer neuen Version schauen", "Look for a new version on start"), |ui| {
                switch(ui, &mut prefs.update_check);
            });
            divider(ui);
            let status: (Option<Icon>, String, Color32) = match &self.update {
                UpdateState::Checking { .. } => (None, t!("Suche…", "Checking…").to_string(), MUTED),
                UpdateState::UpToDate(_) => (Some(Icon::CheckCircle), tf!("Version {VERSION} ist aktuell", "Version {VERSION} is up to date"), GREEN),
                UpdateState::Available(r) => (Some(Icon::ArrowUpCircle), tf!("Version {} verfügbar", "Version {} available", r.version), AMBER),
                UpdateState::Error(e) => (Some(Icon::Warning), e.clone(), RED),
                _ => (None, String::new(), MUTED),
            };
            let checking = matches!(self.update, UpdateState::Checking { .. });
            let mut check = false;
            row(ui, t!("Jetzt prüfen", "Check now"), &status.1, |ui| {
                let busy = matches!(self.update, UpdateState::Checking { .. } | UpdateState::Downloading { .. });
                if ui.add_enabled(!busy, egui::Button::image_and_text(icons::image(ui.ctx(), Icon::Refresh, 14.0, Color32::WHITE), t!("Nach Updates suchen", "Check for updates"))).clicked() {
                    check = true;
                }
                if checking {
                    ui.spinner();
                } else if let Some(i) = status.0 {
                    icons::show(ui, i, 16.0, status.2);
                }
            });
            if check {
                // Geänderte Adresse sofort verwenden
                self.prefs.update_url = prefs.update_url.clone();
                self.check_updates(true);
            }
            divider(ui);
            egui::CollapsingHeader::new(RichText::new(t!("Erweitert", "Advanced")).size(12.5).color(Color32::from_gray(200))).id_salt("upd_adv").show(ui, |ui| {
                ui.label(RichText::new(t!("Update-Adresse (leer = Standard)", "Update address (empty = default)")).size(12.0).color(MUTED));
                let hint = DEFAULT_URL.trim();
                ui.add(egui::TextEdit::singleline(&mut prefs.update_url).hint_text(if hint.is_empty() { "https://…/update.json" } else { hint }).desired_width(f32::INFINITY));
                ui.add_space(4.0);
                ui.label(RichText::new(t!("Updates werden nur installiert, wenn sie mit dem Signierschlüssel von Thomas Schöpf signiert sind.", "Updates are only installed if they are signed with Thomas Schöpf's signing key.")).size(11.5).color(MUTED));
                ui.add_space(6.0);
            });
        });
    }
}
