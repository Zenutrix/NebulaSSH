//! Dialog: Verbindungen aus ~/.ssh/config, PuTTY und MobaXterm übernehmen.

use crate::app::*;
use crate::data::{self, Host, SshKey};
use eframe::egui::{self, vec2, Align, Color32, Layout, RichText, ScrollArea, Ui};
use nebula_core::import::{self, Imported};
use nebula_core::{keys, t, tf};

pub struct Row {
    pub item: Imported,
    pub selected: bool,
    /// Gerät mit gleichem Namen existiert bereits
    pub exists: bool,
}

impl App {
    pub fn open_import(&mut self) {
        self.dialog = Some(Dialog::Import { rows: Vec::new(), source: String::new(), error: None });
    }

    fn import_rows(&self, list: Vec<Imported>) -> Vec<Row> {
        list.into_iter()
            .map(|item| {
                let exists = self.hosts.iter().any(|h| h.name == item.name && h.kind == item.kind);
                Row { selected: !exists, exists, item }
            })
            .collect()
    }

    /// Übernimmt die ausgewählten Einträge; liefert (neu, aktualisiert, Keys).
    fn apply_import(&mut self, rows: &[Row]) -> (usize, usize, usize) {
        let (mut added, mut updated, mut keys_added) = (0, 0, 0);
        for r in rows.iter().filter(|r| r.selected) {
            let it = &r.item;
            let mut h = Host {
                kind: it.kind.clone(),
                name: it.name.clone(),
                ip: it.host.clone(),
                user: if it.kind == "ssh" && it.user.is_empty() { self.prefs.default_user.clone() } else { it.user.clone() },
                group: it.group.clone(),
                jump: it.jump.clone(),
                forwards: it.forwards.clone(),
                agent_forward: it.agent_forward,
                x11: it.x11,
                ..Default::default()
            };
            if it.kind == "serial" {
                h.port = it.serial_port.clone();
                h.baud = if it.baud > 0 { it.baud } else { 115200 };
            }
            if it.use_agent {
                h.auth_method = "agent".into();
            }
            if let Some(kf) = &it.key_file {
                if let Ok(content) = std::fs::read_to_string(kf) {
                    let valid = keys::public_from_private(&content).is_ok() || keys::is_encrypted(&content);
                    if valid {
                        let name = kf.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_else(|| "key".into());
                        if !self.keys.iter().any(|k| k.name == name) {
                            self.keys.push(SshKey { name: name.clone(), content });
                            keys_added += 1;
                        }
                        h.auth_method = "key".into();
                        h.key_name = name;
                    }
                } else if h.auth_method == "password" {
                    // Key-Datei nicht lesbar -> auf den Agent ausweichen
                    h.auth_method = "agent".into();
                }
            }
            if it.kind == "ssh" && h.auth_method == "password" {
                h.ask_password = true;
            }
            match self.hosts.iter().position(|x| x.name == h.name && x.kind == h.kind) {
                Some(i) => {
                    let old = &self.hosts[i];
                    h.favorite = old.favorite;
                    h.notes = old.notes.clone();
                    h.last_used = old.last_used;
                    h.use_count = old.use_count;
                    h.mac = old.mac.clone();
                    if !old.pass.is_empty() && h.auth_method == "password" {
                        h.pass = old.pass.clone();
                        h.ask_password = false;
                    }
                    self.hosts[i] = h;
                    updated += 1;
                }
                None => {
                    self.hosts.push(h);
                    added += 1;
                }
            }
        }
        let _ = data::save_hosts(&self.hosts);
        if keys_added > 0 {
            let _ = data::save_keys(&self.keys);
        }
        (added, updated, keys_added)
    }

    pub fn import_dialog(&mut self, ui: &mut Ui, rows: &mut Vec<Row>, source: &mut String, error: &mut Option<String>) -> bool {
        let mut keep = true;
        ui.set_width(720.0);
        ui.horizontal(|ui| {
            ui.label(RichText::new("📥").size(20.0));
            ui.label(RichText::new(t!("Verbindungen importieren", "Import connections")).size(19.0).family(egui::FontFamily::Name("ui-bold".into())).color(Color32::WHITE));
        });
        ui.add_space(6.0);
        ui.label(RichText::new(t!(
            "Übernimmt gespeicherte Sitzungen inklusive Benutzer, Port, Jump-Host und Tunneln. Passwörter werden nicht übernommen – NebulaSSH fragt sie beim ersten Verbinden.",
            "Takes over saved sessions including user, port, jump host and tunnels. Passwords are not imported – NebulaSSH asks for them on first connect."
        )).color(MUTED));
        ui.add_space(10.0);
        let mut load: Option<(String, Result<Vec<Imported>, String>)> = None;
        ui.horizontal_wrapped(|ui| {
            let cfg = import::default_ssh_config();
            if ui.add_enabled(cfg.exists(), egui::Button::new("~/.ssh/config")).on_hover_text(cfg.display().to_string()).clicked() {
                load = Some(("~/.ssh/config".into(), import::from_file(&cfg)));
            }
            if ui.button("PuTTY").on_hover_text(t!("Sitzungen dieses Windows-Benutzers aus der Registry", "Sessions of this Windows user from the registry")).clicked() {
                load = Some(("PuTTY".into(), import::putty_sessions()));
            }
            let moba = import::find_mobaxterm_ini();
            if ui
                .add_enabled(moba.is_some(), egui::Button::new("MobaXterm"))
                .on_hover_text(moba.as_ref().map(|p| p.display().to_string()).unwrap_or_else(|| t!("MobaXterm.ini nicht gefunden – über „Datei…“ wählen", "MobaXterm.ini not found – pick it via \"File…\"").into()))
                .clicked()
            {
                if let Some(p) = &moba {
                    load = Some(("MobaXterm".into(), import::from_file(p)));
                }
            }
            if ui
                .button(t!("📄 Datei…", "📄 File…"))
                .on_hover_text(t!("ssh_config, PuTTY-.reg-Export oder MobaXterm.ini", "ssh_config, PuTTY .reg export or MobaXterm.ini"))
                .clicked()
            {
                if let Some(p) = rfd::FileDialog::new().set_title(t!("Datei zum Importieren wählen", "Choose a file to import")).pick_file() {
                    let name = p.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
                    load = Some((name, import::from_file(&p)));
                }
            }
        });
        if let Some((src, res)) = load {
            *source = src;
            match res {
                Ok(list) if list.is_empty() => *error = Some(t!("Keine Verbindungen gefunden.", "No connections found.").into()),
                Ok(list) => {
                    *rows = self.import_rows(list);
                    *error = None;
                }
                Err(e) => {
                    *error = Some(e);
                    rows.clear();
                }
            }
        }
        if let Some(e) = error {
            ui.add_space(6.0);
            ui.label(RichText::new(format!("⚠ {e}")).color(RED));
        }
        ui.add_space(8.0);
        if !rows.is_empty() {
            ui.label(RichText::new(tf!("{} Verbindungen aus {source}", "{} connections from {source}", rows.len())).color(MUTED).size(12.0));
            egui::Frame::new().fill(BG).corner_radius(8).inner_margin(6).show(ui, |ui| {
                ScrollArea::vertical().max_height(320.0).auto_shrink([false, true]).show(ui, |ui| {
                    egui::Grid::new("import_rows").num_columns(5).striped(true).spacing(vec2(12.0, 4.0)).show(ui, |ui| {
                        for r in rows.iter_mut() {
                            ui.checkbox(&mut r.selected, "");
                            let it = &r.item;
                            let kind = match it.kind.as_str() {
                                "serial" => "Serial",
                                "telnet" => "Telnet",
                                _ => "SSH",
                            };
                            ui.label(RichText::new(kind).size(11.5).color(MUTED));
                            ui.label(RichText::new(&it.name).strong().color(Color32::WHITE));
                            let addr = if it.kind == "serial" {
                                format!("{} · {}", it.serial_port, it.baud)
                            } else if it.user.is_empty() {
                                it.host.clone()
                            } else {
                                format!("{}@{}", it.user, it.host)
                            };
                            ui.label(RichText::new(addr).monospace().size(12.0));
                            let mut extra = Vec::new();
                            if r.exists {
                                extra.push(t!("wird aktualisiert", "will be updated").to_string());
                            }
                            if !it.jump.is_empty() {
                                extra.push(format!("↪ {}", it.jump));
                            }
                            if !it.forwards.is_empty() {
                                extra.push(tf!("⇄ {} Tunnel", "⇄ {} tunnels", it.forwards.len()));
                            }
                            if it.key_file.is_some() {
                                extra.push("🔑".into());
                            }
                            if it.use_agent {
                                extra.push("🗝 Agent".into());
                            }
                            ui.label(RichText::new(extra.join("  ")).size(11.5).color(if r.exists { AMBER } else { MUTED }));
                            ui.end_row();
                        }
                    });
                });
            });
            ui.horizontal(|ui| {
                if ui.small_button(t!("Alle", "All")).clicked() {
                    rows.iter_mut().for_each(|r| r.selected = true);
                }
                if ui.small_button(t!("Keine", "None")).clicked() {
                    rows.iter_mut().for_each(|r| r.selected = false);
                }
            });
        }
        ui.add_space(10.0);
        let n = rows.iter().filter(|r| r.selected).count();
        ui.horizontal(|ui| {
            if ui.add(egui::Button::new(RichText::new(t!("Abbrechen", "Cancel")).strong()).min_size(vec2(130.0, 34.0))).clicked() {
                keep = false;
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let b = egui::Button::new(RichText::new(tf!("{n} übernehmen", "Import {n}")).strong().color(Color32::WHITE)).fill(ACCENT).min_size(vec2(160.0, 34.0));
                if ui.add_enabled(n > 0, b).clicked() {
                    let (a, u, k) = self.apply_import(rows);
                    self.notify(tf!("Import: {a} neu, {u} aktualisiert, {k} Keys", "Import: {a} new, {u} updated, {k} keys"));
                    keep = false;
                }
            });
        });
        if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
            keep = false;
        }
        keep
    }
}
