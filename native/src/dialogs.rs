//! Dialoge: Einstellungen, SSH-Keys, Key-Generator, Backup, Editor, Geräte, Abfragen.

use crate::app::*;
use crate::data::{self, BaudProfile, SshKey};
use crate::sidebar::{auth_index, field, label, primary, section, segmented, AUTH_IDS};
use crate::theme;
use eframe::egui::{self, vec2, Align, Color32, Key, Layout, Modifiers, RichText, ScrollArea, Stroke, TextEdit, Ui};
use nebula_core::ssh::Forward;
use nebula_core::{backup, keys, ssh, t, tf};
use rfd::MessageLevel;

/// Abbrechen/Bestätigen-Zeile. Some(true) = bestätigt, Some(false) = abgebrochen.
fn buttons(ui: &mut Ui, ok: &str, enabled: bool) -> Option<bool> {
    let mut res = None;
    ui.horizontal(|ui| {
        let w = (ui.available_width() - 8.0) / 2.0;
        if ui.add(egui::Button::new(RichText::new(t!("Abbrechen", "Cancel")).strong()).min_size(vec2(w, 34.0))).clicked() {
            res = Some(false);
        }
        if ui
            .add_enabled(enabled, egui::Button::new(RichText::new(ok).strong().color(Color32::WHITE)).fill(ACCENT).min_size(vec2(w, 34.0)))
            .clicked()
        {
            res = Some(true);
        }
    });
    if ui.input(|i| i.key_pressed(Key::Escape)) {
        res = Some(false);
    }
    res
}

fn heading(ui: &mut Ui, icon: &str, text: &str) {
    ui.horizontal(|ui| {
        ui.label(RichText::new(icon).size(20.0));
        ui.label(RichText::new(text).size(19.0).family(egui::FontFamily::Name("ui-bold".into())).color(Color32::WHITE));
    });
    ui.add_space(12.0);
}

fn code_box(ui: &mut Ui, text: &str, rows: usize) {
    let mut t = text.to_string();
    ui.add(TextEdit::multiline(&mut t).font(egui::TextStyle::Monospace).desired_rows(rows).desired_width(f32::INFINITY).interactive(true));
}

fn hint(ui: &mut Ui, text: &str) {
    ui.label(RichText::new(text).size(11.5).color(MUTED));
}

fn sanitize(name: &str) -> String {
    name.chars().map(|c| if c.is_alphanumeric() || "-_.".contains(c) { c } else { '_' }).collect()
}

fn user_at_host() -> String {
    let u = std::env::var("USERNAME").unwrap_or_else(|_| "user".into());
    let h = std::env::var("COMPUTERNAME").unwrap_or_else(|_| "pc".into());
    format!("{u}@{}", h.to_lowercase())
}

/// Reiter-Leiste oben in einem Dialog
fn page_tabs<T: PartialEq + Copy>(ui: &mut Ui, page: &mut T, pages: &[(T, &str)]) {
    ui.horizontal(|ui| {
        for (p, l) in pages {
            let on = *page == *p;
            let b = egui::Button::new(RichText::new(*l).color(if on { Color32::WHITE } else { Color32::from_gray(190) }))
                .fill(if on { ACCENT } else { Color32::TRANSPARENT })
                .min_size(vec2(0.0, 30.0));
            if ui.add(b).clicked() {
                *page = *p;
            }
        }
    });
    ui.separator();
    ui.add_space(6.0);
}

impl App {
    pub fn open_settings(&mut self, page: SettingsPage) {
        self.dialog = Some(Dialog::Settings { page, prefs: self.prefs.clone(), lock1: String::new(), lock2: String::new() });
    }

    pub fn open_keys(&mut self) {
        self.dialog = Some(Dialog::Keys { selected: None, agent: None });
    }

    pub fn open_keygen(&mut self) {
        self.dialog = Some(Dialog::KeyGen {
            kind: keys::KeyType::Ed25519,
            name: format!("nebula-{}", chrono::Local::now().format("%Y%m%d")),
            comment: user_at_host(),
            pass1: String::new(),
            pass2: String::new(),
            busy: false,
            result: None,
            install_status: None,
        });
    }

    fn install_key_on_active(&mut self, public_line: String) {
        let Some(s) = self.active_ssh() else {
            alert(
                t!("Key hinterlegen", "Install key"),
                t!(
                    "Bitte zuerst per SSH (z.B. mit Passwort) mit dem Server verbinden – dann kann der Key dort automatisch hinterlegt werden.",
                    "Please connect to the server via SSH first (e.g. with a password) – then the key can be installed there automatically."
                ),
            );
            return;
        };
        let tx = self.tx.clone();
        let ctx = self.ctx.clone();
        self.rt.spawn(async move {
            let res = ssh::install_public_key(&s, &public_line).await;
            let _ = tx.send(Msg::KeyInstalled(res));
            ctx.request_repaint();
        });
    }

    fn export_key(&mut self, key: &SshKey) {
        let Some(dir) = rfd::FileDialog::new().set_title(t!("Ordner für den Export wählen", "Choose a folder for the export")).pick_folder() else { return };
        let base = sanitize(&key.name);
        let mut content = key.content.clone();
        if !content.ends_with('\n') {
            content.push('\n');
        }
        let mut written = vec![];
        let res = (|| -> std::io::Result<()> {
            std::fs::write(dir.join(&base), &content)?;
            written.push(base.clone());
            if let Ok((pubk, _)) = keys::public_from_private(&key.content) {
                std::fs::write(dir.join(format!("{base}.pub")), format!("{pubk}\n"))?;
                written.push(format!("{base}.pub"));
            }
            Ok(())
        })();
        match res {
            Ok(()) => self.notify(tf!("Exportiert: {}", "Exported: {}", written.join(", "))),
            Err(e) => alert("Export", &e.to_string()),
        }
    }

    fn load_agent_keys(&mut self) {
        let tx = self.tx.clone();
        let ctx = self.ctx.clone();
        self.rt.spawn(async move {
            let res = ssh::agent_keys().await;
            let _ = tx.send(Msg::AgentKeys(res));
            ctx.request_repaint();
        });
    }

    pub fn dialogs(&mut self, ctx: &egui::Context) {
        let Some(mut dialog) = self.dialog.take() else { return };
        let mut keep = true;
        let modal_frame = egui::Frame::new()
            .fill(Color32::from_rgb(0x1a, 0x1a, 0x26))
            .stroke(Stroke::new(1.0, LINE))
            .corner_radius(14)
            .inner_margin(24)
            .shadow(egui::Shadow { offset: [0, 10], blur: 40, spread: 0, color: Color32::from_black_alpha(160) });
        let screen = ctx.content_rect();
        let resp = egui::Modal::new(egui::Id::new(("dialog", std::mem::discriminant(&dialog))))
            .frame(modal_frame)
            .backdrop_color(Color32::from_black_alpha(170))
            .show(ctx, |ui| match &mut dialog {
                Dialog::AddBaud { name, rate } => {
                    ui.set_width(360.0);
                    heading(ui, "🔌", t!("Neues Baudrate-Profil", "New baud rate profile"));
                    field(ui, t!("Geräteklasse / Name (optional)", "Device class / name (optional)"), name, t!("z.B. Cisco Switch", "e.g. Cisco switch"));
                    field(ui, t!("Baudrate *", "Baud rate *"), rate, t!("z.B. 115200", "e.g. 115200"));
                    ui.add_space(8.0);
                    match buttons(ui, t!("Hinzufügen", "Add"), true) {
                        Some(true) => match rate.trim().parse::<u32>() {
                            Ok(r) if r > 0 => {
                                let label = if name.trim().is_empty() { format!("{r} (Custom)") } else { format!("{r} ({})", name.trim()) };
                                self.bauds.retain(|b| b.label != label);
                                self.bauds.push(BaudProfile { rate: r, label: label.clone() });
                                self.bauds.sort_by_key(|b| b.rate);
                                self.baud_label = label;
                                self.form.baud = r;
                                self.save_settings();
                                keep = false;
                            }
                            _ => alert(t!("Baudrate", "Baud rate"), t!("Bitte eine gültige Zahl für die Baudrate eingeben.", "Please enter a valid number for the baud rate.")),
                        },
                        Some(false) => keep = false,
                        None => {}
                    }
                }
                Dialog::EditHost { index, host, new_group, page, new_fwd } => {
                    keep = self.host_dialog(ui, index, host, new_group, page, new_fwd);
                }
                Dialog::Editor { tab, path, content, original, loading, saving } => {
                    let w = (screen.width() * 0.8).min(1300.0);
                    let h = screen.height() * 0.72;
                    ui.set_width(w);
                    let dirty = content != original;
                    ui.horizontal(|ui| {
                        ui.label(RichText::new("📝").size(18.0));
                        ui.label(RichText::new(path.as_str()).monospace().size(14.0).color(Color32::WHITE));
                        if dirty {
                            ui.label(RichText::new(t!("● ungespeichert", "● unsaved")).color(AMBER).size(12.0));
                        }
                    });
                    ui.add_space(8.0);
                    if *loading {
                        ui.add_sized(vec2(w, h), egui::Spinner::new());
                    } else {
                        let lines = content.lines().count().max(1) + usize::from(content.ends_with('\n'));
                        let font = egui::TextStyle::Monospace;
                        egui::Frame::new().fill(BG).corner_radius(8).stroke(Stroke::new(1.0, LINE)).inner_margin(6).show(ui, |ui| {
                            ScrollArea::both().max_height(h).min_scrolled_height(h).auto_shrink([false, false]).show(ui, |ui| {
                                ui.horizontal_top(|ui| {
                                    let nums: String = (1..=lines).map(|n| format!("{n:>4}\n")).collect();
                                    ui.add(egui::Label::new(RichText::new(nums).text_style(font.clone()).color(Color32::from_gray(90))).selectable(false));
                                    let r = ui.add(
                                        TextEdit::multiline(content)
                                            .id(egui::Id::new("editor_text"))
                                            .code_editor()
                                            .frame(egui::Frame::NONE)
                                            .desired_width(f32::INFINITY)
                                            .desired_rows(40)
                                            .lock_focus(true),
                                    );
                                    if ui.memory(|m| m.focused().is_none()) {
                                        r.request_focus();
                                    }
                                });
                            });
                        });
                        ui.add_space(4.0);
                        hint(
                            ui,
                            &tf!(
                                "{lines} Zeilen · {} · UTF-8 · Strg+S speichert · Esc schließt",
                                "{lines} lines · {} · UTF-8 · Ctrl+S saves · Esc closes",
                                human_size(content.len() as u64)
                            ),
                        );
                    }
                    ui.add_space(8.0);
                    let ctrl_s = ui.input_mut(|i| i.consume_key(Modifiers::CTRL, Key::S));
                    let mut save = ctrl_s;
                    let mut close = false;
                    ui.horizontal(|ui| {
                        if ui.add(egui::Button::new(RichText::new(t!("Schließen", "Close")).strong()).min_size(vec2(130.0, 34.0))).clicked() {
                            close = true;
                        }
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            let label = if *saving { t!("Speichere…", "Saving…") } else { t!("💾 Speichern", "💾 Save") };
                            if primary(ui, label, 150.0).clicked() {
                                save = true;
                            }
                        });
                    });
                    if ui.input(|i| i.key_pressed(Key::Escape)) {
                        close = true;
                    }
                    if save && !*loading && !*saving {
                        *saving = true;
                        let (id, p, c) = (*tab, path.clone(), content.clone());
                        match self.tabs.iter().find(|t| t.id == id).and_then(|t| t.ssh.clone()) {
                            Some(s) => {
                                let tx = self.tx.clone();
                                let ctx2 = ctx.clone();
                                self.rt.spawn(async move {
                                    let res = ssh::write_file(&s, &p, &c).await;
                                    let _ = tx.send(Msg::FileSaved(id, res));
                                    ctx2.request_repaint();
                                });
                            }
                            None => {
                                *saving = false;
                                alert("Editor", t!("Keine aktive SSH-Verbindung – Speichern nicht möglich.", "No active SSH connection – cannot save."));
                            }
                        }
                    }
                    if close
                        && (!dirty
                            || confirm(
                                t!("Ungespeicherte Änderungen", "Unsaved changes"),
                                t!("Die Änderungen wurden nicht gespeichert. Trotzdem schließen?", "The changes have not been saved. Close anyway?"),
                                MessageLevel::Warning,
                            ))
                    {
                        keep = false;
                    }
                }
                Dialog::Rename { tab, old, new } => {
                    ui.set_width(380.0);
                    heading(ui, "✎", t!("Umbenennen", "Rename"));
                    let r = field(ui, &tf!("Neuer Name für „{old}“", "New name for \"{old}\""), new, "");
                    r.request_focus();
                    let enter = (r.lost_focus() || r.has_focus()) && ui.input(|i| i.key_pressed(Key::Enter));
                    ui.add_space(6.0);
                    let res = buttons(ui, t!("Umbenennen", "Rename"), !new.trim().is_empty());
                    if (res == Some(true) || enter) && !new.trim().is_empty() && new != old {
                        let id = *tab;
                        let dir = self.tabs.iter().find(|t| t.id == id).map(|t| t.sftp.path.clone()).unwrap_or_default();
                        let (o, n) = (ssh::join(&dir, old), ssh::join(&dir, new.trim()));
                        self.sftp_op(id, t!("Umbenennen", "Rename").into(), move |s| async move { ssh::rename(&s, &o, &n).await });
                        keep = false;
                    } else if res.is_some() {
                        keep = false;
                    }
                }
                Dialog::MkDir { tab, name } => {
                    ui.set_width(380.0);
                    heading(ui, "📁", t!("Neuer Ordner", "New folder"));
                    let r = field(ui, t!("Ordnername", "Folder name"), name, t!("z.B. backup", "e.g. backup"));
                    r.request_focus();
                    let enter = (r.lost_focus() || r.has_focus()) && ui.input(|i| i.key_pressed(Key::Enter));
                    ui.add_space(6.0);
                    let res = buttons(ui, t!("Anlegen", "Create"), !name.trim().is_empty());
                    if (res == Some(true) || enter) && !name.trim().is_empty() {
                        let id = *tab;
                        let dir = self.tabs.iter().find(|t| t.id == id).map(|t| t.sftp.path.clone()).unwrap_or_default();
                        let p = ssh::join(&dir, name.trim());
                        self.sftp_op(id, t!("Ordner anlegen", "Create folder").into(), move |s| async move { ssh::mkdir(&s, &p).await });
                        keep = false;
                    } else if res.is_some() {
                        keep = false;
                    }
                }
                Dialog::NewFile { tab, name } => {
                    ui.set_width(380.0);
                    heading(ui, "📄", t!("Neue Datei", "New file"));
                    let r = field(ui, t!("Dateiname", "File name"), name, t!("z.B. notizen.txt", "e.g. notes.txt"));
                    r.request_focus();
                    let enter = (r.lost_focus() || r.has_focus()) && ui.input(|i| i.key_pressed(Key::Enter));
                    ui.add_space(6.0);
                    let res = buttons(ui, t!("Erstellen & bearbeiten", "Create & edit"), !name.trim().is_empty());
                    if (res == Some(true) || enter) && !name.trim().is_empty() {
                        let id = *tab;
                        let dir = self.tabs.iter().find(|t| t.id == id).map(|t| t.sftp.path.clone()).unwrap_or_default();
                        let p = ssh::join(&dir, name.trim());
                        keep = false;
                        self.open_editor(id, p, true);
                    } else if res.is_some() {
                        keep = false;
                    }
                }
                Dialog::Chmod { tab, path, name, mode } => {
                    ui.set_width(380.0);
                    heading(ui, "🔐", t!("Rechte ändern", "Change permissions"));
                    ui.label(RichText::new(name.as_str()).monospace());
                    ui.add_space(8.0);
                    let mut bits: Vec<u32> = mode.chars().filter_map(|c| c.to_digit(8)).collect();
                    bits.resize(3, 0);
                    egui::Grid::new("chmod").spacing(vec2(18.0, 6.0)).show(ui, |ui| {
                        ui.label("");
                        ui.label(RichText::new(t!("Lesen", "Read")).color(MUTED));
                        ui.label(RichText::new(t!("Schreiben", "Write")).color(MUTED));
                        ui.label(RichText::new(t!("Ausführen", "Execute")).color(MUTED));
                        ui.end_row();
                        for (i, who) in [t!("Besitzer", "Owner"), t!("Gruppe", "Group"), t!("Andere", "Others")].iter().enumerate() {
                            ui.label(*who);
                            for bit in [4, 2, 1] {
                                let mut on = bits[i] & bit != 0;
                                if ui.checkbox(&mut on, "").changed() {
                                    bits[i] = if on { bits[i] | bit } else { bits[i] & !bit };
                                }
                            }
                            ui.end_row();
                        }
                    });
                    *mode = bits.iter().map(|b| char::from_digit(*b, 8).unwrap()).collect();
                    ui.add_space(6.0);
                    ui.horizontal(|ui| {
                        ui.label(t!("Oktal:", "Octal:"));
                        ui.add(TextEdit::singleline(mode).desired_width(60.0).font(egui::TextStyle::Monospace));
                    });
                    ui.add_space(10.0);
                    match buttons(ui, t!("Übernehmen", "Apply"), true) {
                        Some(true) => match u32::from_str_radix(mode.trim(), 8) {
                            Ok(m) if m <= 0o7777 => {
                                let (id, p) = (*tab, path.clone());
                                self.sftp_op(id, t!("Rechte ändern", "Change permissions").into(), move |s| async move { ssh::chmod(&s, &p, m).await });
                                keep = false;
                            }
                            _ => alert(t!("Rechte", "Permissions"), t!("Ungültiger Oktalwert, z.B. 644 oder 755.", "Invalid octal value, e.g. 644 or 755.")),
                        },
                        Some(false) => keep = false,
                        None => {}
                    }
                }
                Dialog::Update => keep = self.update_dialog(ui),
                Dialog::Keys { selected, agent } => keep = self.keys_dialog(ui, selected, agent),
                Dialog::KeyGen { kind, name, comment, pass1, pass2, busy, result, install_status } => {
                    ui.set_width(600.0);
                    match result {
                        None => {
                            heading(ui, "🔑", t!("Neuen SSH-Key erzeugen", "Create a new SSH key"));
                            label(ui, t!("Schlüsseltyp", "Key type"));
                            ui.horizontal(|ui| {
                                for k in [keys::KeyType::Ed25519, keys::KeyType::Rsa4096, keys::KeyType::EcdsaP256] {
                                    ui.selectable_value(kind, k, k.label());
                                }
                            });
                            ui.add_space(6.0);
                            field(ui, t!("Name in NebulaSSH", "Name in NebulaSSH"), name, t!("z.B. Produktion", "e.g. production"));
                            field(ui, t!("Kommentar (erscheint auf dem Server)", "Comment (shown on the server)"), comment, t!("benutzer@pc", "user@pc"));
                            label(ui, t!("Passphrase (optional, empfohlen)", "Passphrase (optional, recommended)"));
                            ui.horizontal(|ui| {
                                let w = (ui.available_width() - 6.0) / 2.0;
                                ui.add(TextEdit::singleline(pass1).password(true).hint_text("Passphrase").desired_width(w));
                                ui.add(TextEdit::singleline(pass2).password(true).hint_text(t!("wiederholen", "repeat")).desired_width(w));
                            });
                            if !pass1.is_empty() && pass1 != pass2 {
                                ui.label(RichText::new(t!("Die Passphrasen stimmen nicht überein.", "The passphrases do not match.")).color(RED).size(12.0));
                            }
                            hint(
                                ui,
                                t!(
                                    "Mit Passphrase ist der Key auch dann geschützt, wenn jemand die Datei kopiert. NebulaSSH fragt sie beim Verbinden ab.",
                                    "With a passphrase the key stays protected even if someone copies the file. NebulaSSH asks for it when connecting."
                                ),
                            );
                            ui.add_space(12.0);
                            if *busy {
                                ui.horizontal(|ui| {
                                    ui.spinner();
                                    ui.label(t!("Erzeuge Schlüssel…", "Generating key…"));
                                });
                            } else {
                                let ok = !name.trim().is_empty() && (pass1.is_empty() || pass1 == pass2);
                                match buttons(ui, t!("Erzeugen", "Generate"), ok) {
                                    Some(true) => {
                                        let exists = self.keys.iter().any(|k| k.name == name.trim());
                                        if !exists || confirm("SSH-Key", t!("Es gibt bereits einen Key mit diesem Namen. Ersetzen?", "A key with this name already exists. Replace it?"), MessageLevel::Warning) {
                                            *busy = true;
                                            let (k, c, p) = (*kind, comment.clone(), pass1.clone());
                                            let tx = self.tx.clone();
                                            let ctx2 = ctx.clone();
                                            self.rt.spawn_blocking(move || {
                                                let _ = tx.send(Msg::KeyGenerated(keys::generate(k, &c, &p)));
                                                ctx2.request_repaint();
                                            });
                                        }
                                    }
                                    Some(false) => keep = false,
                                    None => {}
                                }
                            }
                        }
                        Some(k) => {
                            heading(ui, "✅", t!("SSH-Key erzeugt", "SSH key created"));
                            ui.label(RichText::new(tf!("Gespeichert als „{}“ · {}", "Saved as \"{}\" · {}", name, k.fingerprint)).color(MUTED).size(12.0));
                            ui.add_space(10.0);
                            section(ui, t!("1. Public Key (öffentlich – darf weitergegeben werden)", "1. Public key (public – may be shared)"));
                            code_box(ui, &k.public_line, 2);
                            ui.horizontal(|ui| {
                                if ui.button(t!("📋 Public Key kopieren", "📋 Copy public key")).clicked() {
                                    ctx.copy_text(k.public_line.clone());
                                    self.notify(t!("Public Key kopiert", "Public key copied"));
                                }
                                let has_ssh = self.active_ssh().is_some();
                                if ui
                                    .add_enabled(has_ssh, egui::Button::new(RichText::new(t!("⚡ Auf verbundenem Server hinterlegen", "⚡ Install on connected server")).color(Color32::WHITE)).fill(ACCENT))
                                    .on_disabled_hover_text(t!("Zuerst per SSH mit dem Server verbinden", "Connect to the server via SSH first"))
                                    .clicked()
                                {
                                    let line = k.public_line.clone();
                                    self.install_key_on_active(line);
                                }
                            });
                            if let Some(s) = install_status {
                                ui.label(RichText::new(s.as_str()).color(if s.starts_with('✔') { GREEN } else { RED }));
                            }
                            ui.add_space(10.0);
                            section(ui, t!("2. Manuell auf einem Linux-Server hinterlegen", "2. Install manually on a Linux server"));
                            let instr = keys::install_instructions(&k.public_line);
                            code_box(ui, &instr, 8);
                            if ui.button(t!("📋 Befehle kopieren", "📋 Copy commands")).clicked() {
                                ctx.copy_text(instr);
                                self.notify(t!("Befehle kopiert", "Commands copied"));
                            }
                            ui.add_space(8.0);
                            section(ui, t!("3. Danach verbinden", "3. Then connect"));
                            ui.label(
                                RichText::new(t!(
                                    "Im Verbindungsformular „Key“ wählen und diesen Key auswählen. Funktioniert der Login, kann die Passwort-Anmeldung auf dem Server deaktiviert werden (PasswordAuthentication no in /etc/ssh/sshd_config).",
                                    "Choose \"Key\" in the connection form and select this key. Once the login works, password login can be disabled on the server (PasswordAuthentication no in /etc/ssh/sshd_config)."
                                ))
                                .size(12.5)
                                .color(MUTED),
                            );
                            ui.add_space(12.0);
                            ui.horizontal(|ui| {
                                if ui.button(t!("💾 Key exportieren…", "💾 Export key…")).clicked() {
                                    let key = SshKey { name: name.clone(), content: k.private_openssh.clone() };
                                    self.export_key(&key);
                                }
                                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                    if primary(ui, t!("Fertig", "Done"), 120.0).clicked() {
                                        keep = false;
                                    }
                                });
                            });
                        }
                    }
                }
                Dialog::ImportKey { name, content } => {
                    ui.set_width(520.0);
                    heading(ui, "📥", t!("SSH-Key importieren", "Import SSH key"));
                    field(ui, t!("Name *", "Name *"), name, t!("z.B. Produktion", "e.g. production"));
                    label(ui, "Private Key");
                    ScrollArea::vertical().max_height(200.0).show(ui, |ui| {
                        ui.add(TextEdit::multiline(content).font(egui::TextStyle::Monospace).hint_text("-----BEGIN OPENSSH PRIVATE KEY-----").desired_rows(9).desired_width(f32::INFINITY));
                    });
                    ui.add_space(6.0);
                    if ui.button(t!("📄 Aus Datei laden (id_ed25519, id_rsa, .pem)", "📄 Load from file (id_ed25519, id_rsa, .pem)")).clicked() {
                        if let Some(p) = rfd::FileDialog::new().set_title(t!("Private Key auswählen", "Choose private key")).pick_file() {
                            match std::fs::metadata(&p) {
                                Ok(m) if m.len() > 1024 * 1024 => alert("SSH-Key", t!("Die Datei ist zu groß für einen SSH-Key.", "The file is too large for an SSH key.")),
                                _ => match std::fs::read_to_string(&p) {
                                    Ok(c) => {
                                        *content = c;
                                        if name.is_empty() {
                                            *name = p.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_default();
                                        }
                                    }
                                    Err(e) => alert("SSH-Key", &e.to_string()),
                                },
                            }
                        }
                    }
                    if content.contains("PuTTY-User-Key-File") {
                        ui.label(
                            RichText::new(t!(
                                "PuTTY-Key (.ppk): bitte mit PuTTYgen → Conversions → Export OpenSSH key umwandeln – oder den Key in Pageant laden und „Agent“ als Anmeldung wählen.",
                                "PuTTY key (.ppk): please convert it with PuTTYgen → Conversions → Export OpenSSH key – or load it into Pageant and choose \"Agent\" as login."
                            ))
                            .color(AMBER)
                            .size(12.0),
                        );
                    } else if !content.trim().is_empty() {
                        match keys::public_from_private(content) {
                            Ok((_, fp)) => ui.label(RichText::new(tf!("✔ Gültiger Key · {fp}", "✔ Valid key · {fp}")).color(GREEN).size(12.0)),
                            Err(_) if keys::is_encrypted(content) => ui.label(
                                RichText::new(t!("🔒 Verschlüsselter Key – Passphrase wird beim Verbinden abgefragt", "🔒 Encrypted key – the passphrase is asked for when connecting")).color(AMBER).size(12.0),
                            ),
                            Err(e) => ui.label(RichText::new(format!("✖ {e}")).color(RED).size(12.0)),
                        };
                    }
                    ui.add_space(10.0);
                    match buttons(ui, t!("Importieren", "Import"), !name.trim().is_empty() && !content.trim().is_empty()) {
                        Some(true) => {
                            let n = name.trim().to_string();
                            self.keys.retain(|k| k.name != n);
                            self.keys.push(SshKey { name: n.clone(), content: content.clone() });
                            self.form.key_name = n;
                            match data::save_keys(&self.keys) {
                                Ok(()) => self.notify(t!("Key importiert", "Key imported")),
                                Err(e) => alert(t!("Speichern", "Save"), &e),
                            }
                            keep = false;
                        }
                        Some(false) => keep = false,
                        None => {}
                    }
                }
                Dialog::Settings { page, prefs, lock1, lock2 } => keep = self.settings_dialog(ui, page, prefs, lock1, lock2),
                Dialog::Backup { export, pass1, pass2, status } => {
                    ui.set_width(460.0);
                    if *export {
                        heading(ui, "📦", t!("Backup erstellen", "Create backup"));
                        ui.label(
                            RichText::new(t!(
                                "Sichert alle Geräte, SSH-Keys, Snippets, Einstellungen und bekannten Host-Keys in eine verschlüsselte Datei – ideal für den Umzug auf einen neuen PC.",
                                "Saves all devices, SSH keys, snippets, settings and known host keys into an encrypted file – ideal for moving to a new PC."
                            ))
                            .color(MUTED),
                        );
                        ui.add_space(10.0);
                        label(ui, t!("Backup-Passwort (mind. 8 Zeichen)", "Backup password (at least 8 characters)"));
                        ui.add(TextEdit::singleline(pass1).password(true).desired_width(f32::INFINITY));
                        ui.add(TextEdit::singleline(pass2).password(true).hint_text(t!("wiederholen", "repeat")).desired_width(f32::INFINITY));
                        if !pass2.is_empty() && pass1 != pass2 {
                            ui.label(RichText::new(t!("Passwörter stimmen nicht überein.", "Passwords do not match.")).color(RED).size(12.0));
                        }
                        ui.label(RichText::new(t!("Ohne dieses Passwort lässt sich das Backup nicht wiederherstellen.", "The backup cannot be restored without this password.")).size(11.5).color(AMBER));
                    } else {
                        heading(ui, "♻", t!("Backup wiederherstellen", "Restore backup"));
                        ui.label(RichText::new(t!("Achtung: Vorhandene Geräte, Keys und Einstellungen auf diesem PC werden ersetzt.", "Warning: existing devices, keys and settings on this PC will be replaced.")).color(AMBER));
                        ui.add_space(10.0);
                        label(ui, t!("Backup-Passwort", "Backup password"));
                        ui.add(TextEdit::singleline(pass1).password(true).desired_width(f32::INFINITY));
                    }
                    if let Some(s) = status {
                        ui.label(RichText::new(s.as_str()).color(RED));
                    }
                    ui.add_space(12.0);
                    let ok = if *export { pass1.chars().count() >= 8 && pass1 == pass2 } else { !pass1.is_empty() };
                    match buttons(ui, if *export { t!("Speichern unter…", "Save as…") } else { t!("Datei wählen…", "Choose file…") }, ok) {
                        Some(true) if *export => match backup::export(pass1) {
                            Ok((bytes, sum)) => {
                                let name = format!("NebulaSSH-Backup_{}.nebula", chrono::Local::now().format("%Y-%m-%d"));
                                if let Some(p) = rfd::FileDialog::new().set_title(t!("Backup speichern", "Save backup")).set_file_name(&name).add_filter("NebulaSSH-Backup", &["nebula"]).save_file() {
                                    match std::fs::write(&p, bytes) {
                                        Ok(()) => {
                                            self.notify(tf!("Backup gespeichert: {} Geräte, {} Keys", "Backup saved: {} devices, {} keys", sum.hosts, sum.keys));
                                            keep = false;
                                        }
                                        Err(e) => *status = Some(e.to_string()),
                                    }
                                }
                            }
                            Err(e) => *status = Some(e),
                        },
                        Some(true) => {
                            if let Some(p) = rfd::FileDialog::new().set_title(t!("Backup öffnen", "Open backup")).add_filter("NebulaSSH-Backup", &["nebula"]).pick_file() {
                                match std::fs::read(&p).map_err(|e| e.to_string()).and_then(|b| backup::import(&b, pass1)) {
                                    Ok(sum) => {
                                        self.hosts = data::load_hosts();
                                        self.keys = data::load_keys();
                                        let s = data::load_settings();
                                        self.snippets = s.snippets;
                                        self.bauds = s.bauds;
                                        self.prefs = s.prefs;
                                        nebula_core::i18n::apply(&self.prefs.language);
                                        self.notify(tf!("Backup wiederhergestellt: {} Geräte, {} Keys", "Backup restored: {} devices, {} keys", sum.hosts, sum.keys));
                                        keep = false;
                                    }
                                    Err(e) => *status = Some(e),
                                }
                            }
                        }
                        Some(false) => keep = false,
                        None => {}
                    }
                }
                Dialog::Ask { tab, passphrase, value, remember, jump } => {
                    ui.set_width(400.0);
                    let tabref = self.tabs.iter().find(|t| t.id == *tab);
                    let title = tabref.map(|t| t.title.clone()).unwrap_or_default();
                    let jump_name = tabref.and_then(|t| self.jump_host(&t.host)).map(|h| h.display_name());
                    let head = match (*passphrase, *jump) {
                        (true, false) => t!("Passphrase für SSH-Key", "Passphrase for SSH key"),
                        (false, false) => t!("Passwort eingeben", "Enter password"),
                        (true, true) => t!("Passphrase für den Jump-Host-Key", "Passphrase for the jump host key"),
                        (false, true) => t!("Passwort für den Jump-Host", "Password for the jump host"),
                    };
                    heading(ui, "🔒", head);
                    ui.label(RichText::new(tf!("Verbindung: {title}", "Connection: {title}")).color(MUTED));
                    if *jump {
                        ui.label(RichText::new(tf!("Jump-Host: {}", "Jump host: {}", jump_name.clone().unwrap_or_default())).color(MUTED));
                    }
                    ui.add_space(8.0);
                    let r = ui.add(TextEdit::singleline(value).password(true).hint_text(if *passphrase { "Passphrase" } else { t!("Passwort", "Password") }).desired_width(f32::INFINITY));
                    r.request_focus();
                    let enter = (r.lost_focus() || r.has_focus()) && ui.input(|i| i.key_pressed(Key::Enter));
                    if !*passphrase {
                        ui.checkbox(remember, t!("Passwort beim Gerät speichern", "Save password with the device"));
                    }
                    ui.add_space(10.0);
                    let res = buttons(ui, t!("Verbinden", "Connect"), !value.is_empty());
                    if (res == Some(true) || enter) && !value.is_empty() {
                        let (id, v, rem, pp, j) = (*tab, value.clone(), *remember, *passphrase, *jump);
                        keep = false;
                        self.dialog = None;
                        let jump_host = self.tabs.iter().find(|t| t.id == id).and_then(|t| self.jump_host(&t.host));
                        if let Some(t) = self.tab(id) {
                            if j {
                                t.jump_secret = v.clone();
                            } else {
                                t.secret = v.clone();
                            }
                            if rem && !pp {
                                let target = if j { jump_host } else { Some(t.host.clone()) };
                                if !j {
                                    t.host.pass = v.clone();
                                    t.host.ask_password = false;
                                }
                                if let Some(h) = target {
                                    if let Some(x) = self.hosts.iter_mut().find(|x| x.same_device(&h)) {
                                        x.pass = v;
                                        x.ask_password = false;
                                        let _ = data::save_hosts(&self.hosts);
                                    }
                                }
                            }
                        }
                        self.start(id);
                    } else if res == Some(false) {
                        let id = *tab;
                        if let Some(t) = self.tab(id) {
                            t.status = Status::Closed;
                            t.term.print(t!("\x1b[90m[Anmeldung abgebrochen]\x1b[0m\n", "\x1b[90m[Login cancelled]\x1b[0m\n"));
                        }
                        keep = false;
                    }
                }
                Dialog::Snippets { query, edit, focus } => keep = self.snippets_dialog(ui, query, edit, focus),
                Dialog::RunSnippet { snippet, values, enter } => keep = self.run_snippet_dialog(ui, snippet, values, enter),
                Dialog::Tunnels { tab, new, save } => keep = self.tunnels_dialog(ui, *tab, new, save),
                Dialog::Import { rows, source, error } => keep = self.import_dialog(ui, rows, source, error),
                Dialog::Sync(s) => keep = self.sync_dialog(ui, s),
            });
        let _ = resp;
        if keep && self.dialog.is_none() {
            self.dialog = Some(dialog);
        } else if !keep {
            self.focus_term = true;
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn host_dialog(
        &mut self,
        ui: &mut Ui,
        index: &mut Option<usize>,
        host: &mut data::Host,
        new_group: &mut String,
        page: &mut HostPage,
        new_fwd: &mut Forward,
    ) -> bool {
        let mut keep = true;
        ui.set_width(540.0);
        let icon = match host.kind.as_str() {
            "serial" => "🔌",
            "telnet" => "🖧",
            _ => "🖥",
        };
        heading(ui, icon, if index.is_some() { t!("Gerät bearbeiten", "Edit device") } else { t!("Neues Gerät", "New device") });
        let mut pages = vec![(HostPage::General, t!("Allgemein", "General"))];
        if host.is_ssh() {
            pages.push((HostPage::Auth, t!("Anmeldung", "Login")));
            pages.push((HostPage::Tunnels, t!("Tunnel", "Tunnels")));
        }
        pages.push((HostPage::Advanced, t!("Erweitert", "Advanced")));
        if !pages.iter().any(|(p, _)| p == page) {
            *page = HostPage::General;
        }
        page_tabs(ui, page, &pages);
        // Feste Höhe, damit der Dialog beim Reiterwechsel nicht springt
        ScrollArea::vertical().max_height(440.0).min_scrolled_height(440.0).auto_shrink([false, false]).show(ui, |ui| {
            ui.set_width(520.0);
            match page {
                HostPage::General => {
                    let kinds = ["ssh", "telnet", "serial"];
                    let cur = kinds.iter().position(|k| *k == host.kind).unwrap_or(0);
                    if let Some(i) = segmented(ui, &["SSH", "Telnet", "Serial"], cur) {
                        host.kind = kinds[i].into();
                        if i == 2 && self.ports.is_empty() {
                            self.ports = nebula_core::serial::list_ports();
                        }
                    }
                    ui.add_space(8.0);
                    field(ui, t!("Name", "Name"), &mut host.name, t!("z.B. Core-Switch", "e.g. core switch"));
                    if host.is_serial() {
                        label(ui, t!("COM-Port", "COM port"));
                        egui::ComboBox::from_id_salt("eh_port").selected_text(host.port.clone()).width(ui.available_width()).show_ui(ui, |ui| {
                            for p in &self.ports {
                                ui.selectable_value(&mut host.port, p.clone(), p);
                            }
                        });
                        ui.add_space(6.0);
                        label(ui, t!("Baudrate", "Baud rate"));
                        egui::ComboBox::from_id_salt("eh_baud").selected_text(host.baud.to_string()).width(ui.available_width()).show_ui(ui, |ui| {
                            for b in &self.bauds {
                                ui.selectable_value(&mut host.baud, b.rate, &b.label);
                            }
                        });
                        ui.add_space(6.0);
                    } else {
                        let title = if host.is_telnet() { t!("Host / IP (optional mit :Port, Standard 23)", "Host / IP (optionally with :port, default 23)") } else { t!("Host / IP (optional mit :Port)", "Host / IP (optionally with :port)") };
                        field(ui, title, &mut host.ip, "192.168.1.50:22");
                        if host.is_ssh() {
                            field(ui, t!("Benutzer", "User"), &mut host.user, "root");
                        }
                    }
                    label(ui, t!("Gruppe", "Group"));
                    let mut groups: Vec<String> = self.hosts.iter().map(|h| h.group.clone()).filter(|g| !g.is_empty()).collect();
                    groups.sort();
                    groups.dedup();
                    ui.horizontal(|ui| {
                        egui::ComboBox::from_id_salt("eh_group")
                            .selected_text(if host.group.is_empty() { t!("Ohne Gruppe", "No group").to_string() } else { host.group.clone() })
                            .width(ui.available_width() * 0.5)
                            .show_ui(ui, |ui| {
                                ui.selectable_value(&mut host.group, String::new(), t!("Ohne Gruppe", "No group"));
                                for g in &groups {
                                    ui.selectable_value(&mut host.group, g.clone(), g);
                                }
                            });
                        let r = ui.add(TextEdit::singleline(new_group).hint_text(t!("oder neue Gruppe…", "or new group…")).desired_width(f32::INFINITY));
                        if r.changed() && !new_group.trim().is_empty() {
                            host.group = new_group.trim().to_string();
                        }
                    });
                    ui.add_space(6.0);
                    label(ui, t!("Notizen", "Notes"));
                    ui.add(TextEdit::multiline(&mut host.notes).desired_rows(2).desired_width(f32::INFINITY).hint_text(t!("z.B. Standort, Seriennummer, Zuständigkeit", "e.g. location, serial number, owner")));
                    ui.checkbox(&mut host.favorite, t!("★ Favorit", "★ Favorite"));
                }
                HostPage::Auth => {
                    if let Some(i) = segmented(ui, &[t!("Passwort", "Password"), "Key", "Agent"], auth_index(&host.auth_method)) {
                        host.auth_method = AUTH_IDS[i].into();
                        if i == 1 && host.key_name.is_empty() {
                            host.key_name = self.keys.first().map(|k| k.name.clone()).unwrap_or_default();
                        }
                    }
                    ui.add_space(8.0);
                    match host.auth_method.as_str() {
                        "key" => {
                            label(ui, "SSH-Key");
                            egui::ComboBox::from_id_salt("eh_key").selected_text(host.key_name.clone()).width(ui.available_width()).show_ui(ui, |ui| {
                                for k in &self.keys {
                                    ui.selectable_value(&mut host.key_name, k.name.clone(), &k.name);
                                }
                            });
                            hint(ui, t!("Keys erzeugen oder importieren: 🔑 oben in der Seitenleiste.", "Create or import keys: 🔑 at the top of the sidebar."));
                        }
                        "agent" => {
                            hint(
                                ui,
                                t!(
                                    "Die Anmeldung erfolgt mit den Schlüsseln des laufenden SSH-Agents: Pageant (PuTTY), Windows-OpenSSH-Agent (Dienst „ssh-agent“), 1Password oder KeePassXC. Ein Passwort wird nur verwendet, falls keiner der Schlüssel passt.",
                                    "Login uses the keys of the running SSH agent: Pageant (PuTTY), the Windows OpenSSH agent (service \"ssh-agent\"), 1Password or KeePassXC. A password is only used if none of the keys fit."
                                ),
                            );
                        }
                        _ => {
                            ui.checkbox(&mut host.ask_password, t!("Passwort nicht speichern – beim Verbinden fragen", "Don't save the password – ask when connecting"));
                            if !host.ask_password {
                                ui.add(TextEdit::singleline(&mut host.pass).password(true).hint_text(t!("Passwort", "Password")).desired_width(f32::INFINITY));
                            }
                        }
                    }
                    ui.add_space(12.0);
                    section(ui, t!("Jump-Host (Bastion)", "Jump host (bastion)"));
                    hint(
                        ui,
                        t!(
                            "Verbindet zuerst mit diesem gespeicherten Gerät und von dort weiter zum Ziel – wie ProxyJump / ssh -J.",
                            "Connects to this saved device first and from there to the target – like ProxyJump / ssh -J."
                        ),
                    );
                    let candidates: Vec<String> = self.hosts.iter().filter(|h| h.is_ssh() && !h.same_device(host)).map(|h| h.display_name()).collect();
                    let shown = if host.jump.is_empty() { t!("– direkt verbinden –", "– connect directly –").to_string() } else { host.jump.clone() };
                    egui::ComboBox::from_id_salt("eh_jump").selected_text(shown).width(ui.available_width()).show_ui(ui, |ui| {
                        ui.selectable_value(&mut host.jump, String::new(), t!("– direkt verbinden –", "– connect directly –"));
                        for c in &candidates {
                            ui.selectable_value(&mut host.jump, c.clone(), c);
                        }
                    });
                    if !host.jump.is_empty() && !candidates.contains(&host.jump) {
                        hint(ui, &tf!("„{}“ ist kein gespeichertes Gerät – Anmeldung dort über den SSH-Agent.", "\"{}\" is not a saved device – login there uses the SSH agent.", host.jump));
                    }
                    ui.add_space(12.0);
                    section(ui, t!("Weiterleitungen", "Forwarding"));
                    ui.checkbox(&mut host.agent_forward, t!("SSH-Agent weiterreichen (ssh -A)", "Forward the SSH agent (ssh -A)"));
                    hint(ui, t!("Damit kann der Server deine Agent-Schlüssel für weitere Verbindungen nutzen – nur bei vertrauenswürdigen Servern aktivieren.", "Lets the server use your agent keys for further connections – only enable for trusted servers."));
                    ui.checkbox(&mut host.x11, t!("X11-Weiterleitung (grafische Programme)", "X11 forwarding (graphical programs)"));
                    hint(
                        ui,
                        &tf!(
                            "Benötigt einen X-Server auf diesem PC, z.B. VcXsrv („Disable access control“) oder X410, auf Display :{} (Einstellungen → Verbindung).",
                            "Needs an X server on this PC, e.g. VcXsrv (\"Disable access control\") or X410, on display :{} (Settings → Connection).",
                            self.prefs.x11_display
                        ),
                    );
                }
                HostPage::Tunnels => {
                    hint(ui, t!("Diese Tunnel werden bei jedem Verbinden automatisch geöffnet.", "These tunnels are opened automatically on every connect."));
                    ui.add_space(6.0);
                    let mut remove = None;
                    if host.forwards.is_empty() {
                        ui.label(RichText::new(t!("Noch keine Tunnel.", "No tunnels yet.")).color(MUTED));
                    }
                    for (i, f) in host.forwards.iter_mut().enumerate() {
                        ui.horizontal(|ui| {
                            ui.checkbox(&mut f.enabled, "");
                            ui.label(RichText::new(f.label()).color(if f.enabled { Color32::WHITE } else { MUTED }));
                            ui.label(RichText::new(f.short()).monospace().size(11.5).color(MUTED));
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                if ui.small_button("🗑").clicked() {
                                    remove = Some(i);
                                }
                            });
                        });
                    }
                    if let Some(i) = remove {
                        host.forwards.remove(i);
                    }
                    ui.add_space(10.0);
                    ui.separator();
                    ui.label(RichText::new(t!("Tunnel hinzufügen", "Add tunnel")).strong());
                    if crate::snippets::forward_editor(ui, new_fwd) {
                        host.forwards.push(new_fwd.clone());
                        *new_fwd = Forward { kind: new_fwd.kind.clone(), ..Default::default() };
                    }
                }
                HostPage::Advanced => {
                    if host.is_network() {
                        section(ui, "Wake-on-LAN");
                        field(ui, t!("MAC-Adresse", "MAC address"), &mut host.mac, "aa:bb:cc:dd:ee:ff");
                        if !host.mac.trim().is_empty() {
                            if let Err(e) = nebula_core::net::parse_mac(&host.mac) {
                                ui.label(RichText::new(e).color(RED).size(12.0));
                            }
                        }
                        hint(ui, t!("Rechtsklick auf das Gerät → „Aufwecken“ sendet das Magic Packet ins lokale Netz.", "Right-click the device → \"Wake up\" sends the magic packet into the local network."));
                        ui.add_space(10.0);
                    }
                    section(ui, t!("Statistik", "Statistics"));
                    ui.label(RichText::new(tf!("Zuletzt verbunden: {} · {} Verbindungen", "Last connected: {} · {} connections", data::ago(host.last_used), host.use_count)).color(MUTED));
                }
            }
        });
        ui.add_space(10.0);
        match buttons(ui, t!("Speichern", "Save"), true) {
            Some(true) => {
                let mut h = host.clone();
                if h.ask_password {
                    h.pass.clear();
                }
                if h.name.trim().is_empty() {
                    h.name = if h.is_serial() { h.port.clone() } else { h.ip.clone() };
                }
                if !h.mac.trim().is_empty() && nebula_core::net::parse_mac(&h.mac).is_err() {
                    alert("Wake-on-LAN", t!("Die MAC-Adresse ist ungültig.", "The MAC address is invalid."));
                    return true;
                }
                match index {
                    Some(i) if *i < self.hosts.len() => self.hosts[*i] = h,
                    _ => self.hosts.push(h),
                }
                match data::save_hosts(&self.hosts) {
                    Ok(()) => self.notify(t!("Gerät gespeichert", "Device saved")),
                    Err(e) => alert(t!("Speichern", "Save"), &e),
                }
                keep = false;
            }
            Some(false) => keep = false,
            None => {}
        }
        keep
    }

    fn keys_dialog(&mut self, ui: &mut Ui, selected: &mut Option<usize>, agent: &mut Option<Result<Vec<(String, String, String)>, String>>) -> bool {
        let mut keep = true;
        ui.set_width(620.0);
        heading(ui, "🔑", t!("SSH-Keys", "SSH keys"));
        ui.horizontal(|ui| {
            if ui.add(egui::Button::new(RichText::new(t!("✨ Neuen Key erzeugen", "✨ Create new key")).color(Color32::WHITE)).fill(ACCENT)).clicked() {
                self.open_keygen();
                keep = false;
            }
            if ui.button(t!("📥 Importieren…", "📥 Import…")).clicked() {
                self.dialog = Some(Dialog::ImportKey { name: String::new(), content: String::new() });
                keep = false;
            }
        });
        ui.add_space(10.0);
        if self.keys.is_empty() {
            ui.label(
                RichText::new(t!(
                    "Noch keine Keys gespeichert. Erzeuge einen neuen Key – NebulaSSH zeigt dir danach, wie du ihn auf deinem Linux-Server hinterlegst.",
                    "No keys saved yet. Create a new key – NebulaSSH then shows you how to install it on your Linux server."
                ))
                .color(MUTED),
            );
        }
        let mut delete = None;
        ScrollArea::vertical().max_height(220.0).show(ui, |ui| {
            for (i, k) in self.keys.iter().enumerate() {
                let info = keys::public_from_private(&k.content);
                let sel = *selected == Some(i);
                let r = egui::Frame::new()
                    .fill(if sel { Color32::from_rgb(0x2c, 0x1d, 0x55) } else { PANEL2 })
                    .corner_radius(8)
                    .inner_margin(10)
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.horizontal(|ui| {
                            ui.label(RichText::new("🔑").size(18.0));
                            ui.vertical(|ui| {
                                ui.label(RichText::new(&k.name).strong().color(Color32::WHITE));
                                let sub = match &info {
                                    Ok((line, fp)) => format!("{} · {}", line.split_whitespace().next().unwrap_or(""), fp),
                                    Err(_) if keys::is_encrypted(&k.content) => t!("verschlüsselt (Passphrase)", "encrypted (passphrase)").into(),
                                    Err(_) => t!("Format unbekannt", "unknown format").into(),
                                };
                                ui.label(RichText::new(sub).size(11.5).color(MUTED));
                            });
                        });
                    })
                    .response
                    .interact(egui::Sense::click());
                if r.clicked() {
                    *selected = Some(i);
                }
                ui.add_space(4.0);
            }
        });
        if let Some(i) = selected.filter(|i| *i < self.keys.len()) {
            let k = self.keys[i].clone();
            ui.add_space(8.0);
            ui.separator();
            ui.add_space(6.0);
            ui.label(RichText::new(tf!("Ausgewählt: {}", "Selected: {}", k.name)).strong());
            ui.horizontal_wrapped(|ui| {
                if let Ok((line, _)) = keys::public_from_private(&k.content) {
                    if ui.button(t!("📋 Public Key kopieren", "📋 Copy public key")).clicked() {
                        ui.ctx().copy_text(line.clone());
                        self.notify(t!("Public Key kopiert", "Public key copied"));
                    }
                    if ui
                        .add_enabled(self.active_ssh().is_some(), egui::Button::new(t!("⚡ Auf verbundenem Server hinterlegen", "⚡ Install on connected server")))
                        .on_disabled_hover_text(t!("Zuerst per SSH verbinden", "Connect via SSH first"))
                        .clicked()
                    {
                        self.install_key_on_active(line.clone());
                    }
                    if ui.button(t!("📋 Anleitung kopieren", "📋 Copy instructions")).clicked() {
                        ui.ctx().copy_text(keys::install_instructions(&line));
                        self.notify(t!("Anleitung kopiert", "Instructions copied"));
                    }
                }
                if ui.button(t!("💾 Exportieren…", "💾 Export…")).clicked() {
                    self.export_key(&k);
                }
                if ui.button(RichText::new(t!("🗑 Löschen", "🗑 Delete")).color(RED)).clicked() {
                    delete = Some(i);
                }
            });
        }
        if let Some(i) = delete {
            let name = self.keys[i].name.clone();
            let used = self.hosts.iter().filter(|h| h.key_name == name).count();
            let warn = if used > 0 { tf!("\n\n{used} gespeicherte(s) Gerät(e) verwenden diesen Key.", "\n\n{used} saved device(s) use this key.") } else { String::new() };
            if confirm(t!("SSH-Key löschen", "Delete SSH key"), &tf!("Key „{name}“ wirklich löschen?{warn}\n\nTipp: Vorher exportieren!", "Really delete key \"{name}\"?{warn}\n\nTip: export it first!"), MessageLevel::Warning) {
                self.keys.remove(i);
                *selected = None;
                if let Err(e) = data::save_keys(&self.keys) {
                    alert(t!("Speichern", "Save"), &e);
                }
            }
        }
        // SSH-Agent
        ui.add_space(12.0);
        ui.separator();
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            ui.label(RichText::new(t!("🗝 SSH-Agent", "🗝 SSH agent")).strong());
            if ui.small_button(t!("⟳ Prüfen", "⟳ Check")).clicked() {
                *agent = None;
                self.load_agent_keys();
            }
        });
        match agent {
            None => {
                hint(ui, t!("Pageant, Windows-OpenSSH-Agent, 1Password oder KeePassXC – „Prüfen“ zeigt die geladenen Schlüssel.", "Pageant, Windows OpenSSH agent, 1Password or KeePassXC – \"Check\" lists the loaded keys."));
            }
            Some(Err(e)) => {
                ui.label(RichText::new(e.as_str()).color(AMBER).size(12.0));
            }
            Some(Ok(list)) if list.is_empty() => {
                ui.label(RichText::new(t!("Agent läuft, enthält aber keine Schlüssel.", "The agent is running but holds no keys.")).color(AMBER).size(12.0));
            }
            Some(Ok(list)) => {
                for (alg, comment, fp) in list.iter() {
                    ui.label(RichText::new(format!("✔ {alg} · {comment} · {fp}")).size(12.0).color(GREEN));
                }
            }
        }
        ui.add_space(12.0);
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if primary(ui, t!("Schließen", "Close"), 120.0).clicked() {
                keep = false;
            }
        });
        if ui.input(|i| i.key_pressed(Key::Escape)) {
            keep = false;
        }
        keep
    }

    fn settings_dialog(&mut self, ui: &mut Ui, page: &mut SettingsPage, p: &mut data::Prefs, lock1: &mut String, lock2: &mut String) -> bool {
        let mut keep = true;
        ui.set_width(720.0);
        heading(ui, "⚙", t!("Einstellungen", "Settings"));
        ui.allocate_ui_with_layout(vec2(720.0, 450.0), Layout::left_to_right(Align::Min), |ui| {
            ui.vertical(|ui| {
                ui.set_width(160.0);
                for (pg, label) in [
                    (SettingsPage::Terminal, t!("🖥  Terminal", "🖥  Terminal")),
                    (SettingsPage::Connection, t!("🌐  Verbindung", "🌐  Connection")),
                    (SettingsPage::Security, t!("🛡  Sicherheit", "🛡  Security")),
                    (SettingsPage::Sessions, t!("🗂  Sitzungen", "🗂  Sessions")),
                    (SettingsPage::Backup, t!("📦  Daten & Backup", "📦  Data & backup")),
                    (SettingsPage::About, t!("ℹ  Über", "ℹ  About")),
                ] {
                    let on = *page == pg;
                    let b = egui::Button::new(RichText::new(label).color(if on { Color32::WHITE } else { Color32::from_gray(200) }))
                        .fill(if on { ACCENT } else { Color32::TRANSPARENT })
                        .min_size(vec2(160.0, 32.0));
                    if ui.add(b).clicked() {
                        *page = pg;
                    }
                }
            });
            ui.add_space(14.0);
            ScrollArea::vertical().max_height(450.0).auto_shrink([false, false]).show(ui, |ui| {
                ui.with_layout(Layout::top_down(Align::Min), |ui| {
                    ui.set_width(530.0);
                    match page {
                        SettingsPage::Terminal => {
                            section(ui, t!("Sprache", "Language"));
                            ui.horizontal(|ui| {
                                ui.selectable_value(&mut p.language, "auto".into(), t!("Automatisch (Windows)", "Automatic (Windows)"));
                                ui.selectable_value(&mut p.language, "de".into(), "Deutsch");
                                ui.selectable_value(&mut p.language, "en".into(), "English");
                            });
                            ui.add_space(10.0);
                            section(ui, t!("Darstellung", "Appearance"));
                            ui.add(egui::Slider::new(&mut p.font_size, 9.0..=28.0).text(t!("Schriftgröße", "Font size")).step_by(1.0));
                            ui.add_space(6.0);
                            ui.horizontal(|ui| {
                                ui.label(t!("Farbschema", "Color scheme"));
                                egui::ComboBox::from_id_salt("theme").selected_text(theme::by_id(&p.theme).label()).show_ui(ui, |ui| {
                                    for t in theme::THEMES {
                                        ui.selectable_value(&mut p.theme, t.id.to_string(), t.label());
                                    }
                                });
                            });
                            let th = theme::by_id(&p.theme);
                            egui::Frame::new().fill(th.bg).corner_radius(6).inner_margin(8).show(ui, |ui| {
                                ui.horizontal(|ui| {
                                    ui.label(RichText::new("user@server:~$ ").monospace().color(th.ansi[10]));
                                    ui.label(RichText::new("ls ").monospace().color(th.fg));
                                    ui.label(RichText::new(t!("ordner ", "folder ")).monospace().color(th.ansi[12]));
                                    ui.label(RichText::new(t!("fehler ", "error ")).monospace().color(th.ansi[9]));
                                    ui.label(RichText::new(t!("warnung", "warning")).monospace().color(th.ansi[11]));
                                });
                            });
                            ui.add_space(6.0);
                            ui.horizontal(|ui| {
                                ui.label("Cursor");
                                ui.selectable_value(&mut p.cursor, "block".into(), t!("█ Block", "█ Block"));
                                ui.selectable_value(&mut p.cursor, "beam".into(), t!("▏ Strich", "▏ Beam"));
                                ui.selectable_value(&mut p.cursor, "underline".into(), t!("▁ Unterstrich", "▁ Underline"));
                            });
                            ui.add_space(10.0);
                            section(ui, t!("Verhalten", "Behavior"));
                            ui.checkbox(&mut p.copy_on_select, t!("Markierten Text automatisch kopieren", "Copy selected text automatically"));
                            ui.checkbox(&mut p.right_click_paste, t!("Rechtsklick fügt ein", "Right-click pastes"));
                            ui.horizontal(|ui| {
                                ui.label(t!("Verlauf (Zeilen)", "Scrollback (lines)"));
                                ui.add(egui::DragValue::new(&mut p.scrollback).range(1_000..=1_000_000).speed(1000));
                            });
                            ui.checkbox(&mut p.server_stats, t!("CPU, RAM und Festplatte des Servers in der Statusleiste (Linux)", "Show server CPU, RAM and disk in the status bar (Linux)"));
                        }
                        SettingsPage::Connection => {
                            section(ui, "SSH");
                            ui.horizontal(|ui| {
                                ui.label(t!("Keepalive alle", "Keepalive every"));
                                ui.add(egui::DragValue::new(&mut p.keepalive).range(0..=600).suffix(" s"));
                                ui.label(RichText::new(t!("(0 = aus)", "(0 = off)")).color(MUTED));
                            });
                            ui.checkbox(&mut p.legacy_algorithms, t!("Ältere Verfahren erlauben (SHA1, CBC, 3DES) – für alte Switches/Router", "Allow older algorithms (SHA1, CBC, 3DES) – for old switches/routers"));
                            hint(
                                ui,
                                t!(
                                    "Moderne Verfahren werden immer bevorzugt. Abschalten erhöht die Sicherheit, kann aber alte Geräte aussperren.",
                                    "Modern algorithms are always preferred. Turning this off increases security but can lock out old devices."
                                ),
                            );
                            ui.add_space(8.0);
                            ui.horizontal(|ui| {
                                ui.label(t!("Standard-Benutzer", "Default user"));
                                ui.add(TextEdit::singleline(&mut p.default_user).desired_width(160.0));
                            });
                            ui.checkbox(&mut p.confirm_close, t!("Vor dem Schließen aktiver Verbindungen nachfragen", "Ask before closing active connections"));
                            ui.add_space(10.0);
                            section(ui, t!("X11-Weiterleitung", "X11 forwarding"));
                            ui.horizontal(|ui| {
                                ui.label(t!("Lokales X-Display", "Local X display"));
                                ui.add(egui::DragValue::new(&mut p.x11_display).range(0..=63).prefix(":"));
                                ui.label(RichText::new(tf!("(Port {})", "(port {})", 6000 + p.x11_display)).color(MUTED));
                            });
                            hint(
                                ui,
                                t!(
                                    "Für grafische Linux-Programme wird ein X-Server auf diesem PC benötigt (z.B. VcXsrv mit „Disable access control“ oder X410). X11 wird pro Gerät unter Bearbeiten → Anmeldung eingeschaltet.",
                                    "Graphical Linux programs need an X server on this PC (e.g. VcXsrv with \"Disable access control\" or X410). X11 is enabled per device under Edit → Login."
                                ),
                            );
                            ui.add_space(10.0);
                            section(ui, "SFTP");
                            ui.horizontal(|ui| {
                                ui.label(t!("Gleichzeitige Übertragungen", "Parallel transfers"));
                                ui.add(egui::DragValue::new(&mut p.parallel_transfers).range(1..=8));
                            });
                        }
                        SettingsPage::Security => {
                            section(ui, t!("Server-Identität", "Server identity"));
                            ui.checkbox(&mut p.verify_new_hosts, t!("Bei neuen Servern den Fingerprint bestätigen lassen", "Ask to confirm the fingerprint of new servers"));
                            hint(ui, t!("Geänderte Host-Keys werden immer gemeldet (Schutz vor Man-in-the-Middle).", "Changed host keys are always reported (protection against man-in-the-middle)."));
                            ui.add_space(6.0);
                            egui::CollapsingHeader::new(tf!("Bekannte Server ({})", "Known servers ({})", ssh::known_hosts().len())).show(ui, |ui| {
                                ScrollArea::vertical().max_height(120.0).show(ui, |ui| {
                                    for (host, fp) in ssh::known_hosts() {
                                        ui.horizontal(|ui| {
                                            if ui.small_button("🗑").on_hover_text(t!("Vergessen", "Forget")).clicked() {
                                                ssh::forget_host_key(&host);
                                            }
                                            ui.label(RichText::new(&host).monospace().size(12.0));
                                            ui.label(RichText::new(&fp).monospace().size(10.5).color(MUTED));
                                        });
                                    }
                                });
                            });
                            ui.add_space(8.0);
                            section(ui, t!("Zwischenablage", "Clipboard"));
                            ui.horizontal(|ui| {
                                ui.label(t!("Kopierte Passwörter löschen nach", "Clear copied passwords after"));
                                ui.add(egui::DragValue::new(&mut p.clipboard_clear).range(0..=600).suffix(" s"));
                            });
                            ui.add_space(8.0);
                            section(ui, t!("App-Sperre", "App lock"));
                            if p.lock_hash.is_some() {
                                ui.label(RichText::new(t!("✔ NebulaSSH ist mit einem App-Passwort geschützt.", "✔ NebulaSSH is protected with an app password.")).color(GREEN));
                                ui.horizontal(|ui| {
                                    ui.label(t!("Automatisch sperren nach", "Lock automatically after"));
                                    ui.add(egui::DragValue::new(&mut p.autolock_minutes).range(0..=240).suffix(" min"));
                                    ui.label(RichText::new(t!("(0 = nie)", "(0 = never)")).color(MUTED));
                                });
                                if ui.button(t!("App-Passwort entfernen", "Remove app password")).clicked()
                                    && confirm(t!("App-Sperre", "App lock"), t!("App-Passwort wirklich entfernen?", "Really remove the app password?"), MessageLevel::Warning)
                                {
                                    p.lock_hash = None;
                                }
                            } else {
                                hint(ui, t!("Schützt den Start von NebulaSSH mit einem Passwort (z.B. auf geteilten PCs).", "Protects starting NebulaSSH with a password (e.g. on shared PCs)."));
                                ui.horizontal(|ui| {
                                    ui.add(TextEdit::singleline(lock1).password(true).hint_text(t!("Neues App-Passwort", "New app password")).desired_width(170.0));
                                    ui.add(TextEdit::singleline(lock2).password(true).hint_text(t!("wiederholen", "repeat")).desired_width(170.0));
                                    let ok = lock1.chars().count() >= 4 && lock1 == lock2;
                                    if ui.add_enabled(ok, egui::Button::new(t!("Aktivieren", "Enable"))).clicked() {
                                        match backup::hash_password(lock1) {
                                            Ok(h) => {
                                                p.lock_hash = Some(h);
                                                lock1.clear();
                                                lock2.clear();
                                            }
                                            Err(e) => alert(t!("App-Sperre", "App lock"), &e),
                                        }
                                    }
                                });
                            }
                        }
                        SettingsPage::Sessions => {
                            section(ui, t!("Beim Start", "On start"));
                            ui.checkbox(&mut p.restore_tabs, t!("Offene Verbindungen beim nächsten Start wiederherstellen", "Restore open connections on next start"));
                            ui.checkbox(&mut p.splash, t!("Startbildschirm anzeigen", "Show splash screen"));
                            ui.add_space(8.0);
                            section(ui, t!("Protokollierung", "Logging"));
                            ui.checkbox(&mut p.log_sessions, t!("Alle Sitzungen als Textdatei mitschreiben", "Write all sessions to text files"));
                            ui.horizontal(|ui| {
                                let shown = if p.log_dir.is_empty() { nebula_core::storage::data_dir().join("logs").to_string_lossy().to_string() } else { p.log_dir.clone() };
                                ui.label(RichText::new(shown).monospace().size(11.5).color(MUTED));
                            });
                            ui.horizontal(|ui| {
                                if ui.button(t!("Ordner wählen…", "Choose folder…")).clicked() {
                                    if let Some(d) = rfd::FileDialog::new().pick_folder() {
                                        p.log_dir = d.to_string_lossy().to_string();
                                    }
                                }
                                if ui.button(t!("Ordner öffnen", "Open folder")).clicked() {
                                    let dir = if p.log_dir.is_empty() { nebula_core::storage::data_dir().join("logs") } else { p.log_dir.clone().into() };
                                    let _ = std::fs::create_dir_all(&dir);
                                    let _ = std::process::Command::new("explorer").arg(dir).spawn();
                                }
                            });
                            ui.label(
                                RichText::new(t!(
                                    "Gilt für neu geöffnete Verbindungen. Logs können Passwörter enthalten, die im Terminal eingegeben werden.",
                                    "Applies to newly opened connections. Logs can contain passwords typed into the terminal."
                                ))
                                .size(11.5)
                                .color(AMBER),
                            );
                        }
                        SettingsPage::Backup => {
                            section(ui, t!("Umzug auf einen neuen PC", "Moving to a new PC"));
                            ui.label(
                                RichText::new(t!(
                                    "Ein Backup enthält Geräte, SSH-Keys, Snippets, Einstellungen und bekannte Server – verschlüsselt mit einem Passwort deiner Wahl.",
                                    "A backup contains devices, SSH keys, snippets, settings and known servers – encrypted with a password of your choice."
                                ))
                                .color(MUTED),
                            );
                            ui.add_space(8.0);
                            ui.horizontal(|ui| {
                                if ui.add(egui::Button::new(RichText::new(t!("📦 Backup erstellen…", "📦 Create backup…")).color(Color32::WHITE)).fill(ACCENT)).clicked() {
                                    self.pending_backup = Some(true);
                                }
                                if ui.button(t!("♻ Backup wiederherstellen…", "♻ Restore backup…")).clicked() {
                                    self.pending_backup = Some(false);
                                }
                            });
                            ui.add_space(12.0);
                            section(ui, t!("Speicherort", "Storage location"));
                            let dir = nebula_core::storage::data_dir();
                            ui.label(RichText::new(dir.to_string_lossy()).monospace().size(12.0));
                            if ui.button(t!("Datenordner öffnen", "Open data folder")).clicked() {
                                let _ = std::process::Command::new("explorer").arg(&dir).spawn();
                            }
                            hint(
                                ui,
                                t!(
                                    "hosts.json und keys.json sind mit AES-256 verschlüsselt; der Schlüssel liegt in der Windows-Anmeldeinformationsverwaltung dieses Benutzers.",
                                    "hosts.json and keys.json are encrypted with AES-256; the key is stored in this user's Windows Credential Manager."
                                ),
                            );
                        }
                        SettingsPage::About => {
                            ui.vertical_centered(|ui| {
                                ui.add_space(10.0);
                                ui.add(egui::Image::new(&self.logo).fit_to_exact_size(vec2(96.0, 96.0)));
                                ui.add_space(8.0);
                                ui.label(RichText::new("NebulaSSH").size(24.0).family(egui::FontFamily::Name("ui-bold".into())).color(Color32::WHITE));
                                ui.label(RichText::new(format!("Version {VERSION}")).color(MUTED));
                                ui.add_space(8.0);
                                ui.label(t!("SSH-, SFTP-, Telnet- und Serial-Client für Windows", "SSH, SFTP, Telnet and serial client for Windows"));
                                ui.label(RichText::new("by Thomas Schöpf").color(LAVENDER));
                                ui.add_space(14.0);
                                ui.label(RichText::new(t!("Gebaut mit Rust · egui · russh · alacritty_terminal", "Built with Rust · egui · russh · alacritty_terminal")).size(12.0).color(MUTED));
                            });
                            self.update_settings(ui, p);
                        }
                    }
                });
            });
        });
        ui.add_space(12.0);
        let res = buttons(ui, t!("Übernehmen", "Apply"), true);
        match res {
            Some(true) => {
                let changed_term = p.scrollback != self.prefs.scrollback || p.cursor != self.prefs.cursor;
                self.prefs = p.clone();
                nebula_core::i18n::apply(&self.prefs.language);
                self.save_settings();
                if changed_term {
                    for t in &self.tabs {
                        t.term.apply_prefs(self.prefs.scrollback, &self.prefs.cursor);
                    }
                }
                self.notify(t!("Einstellungen gespeichert", "Settings saved"));
                keep = false;
            }
            Some(false) => keep = false,
            None => {}
        }
        if let Some(export) = self.pending_backup.take() {
            self.prefs = p.clone();
            self.save_settings();
            self.dialog = Some(Dialog::Backup { export, pass1: String::new(), pass2: String::new(), status: None });
            return false;
        }
        keep
    }
}
