//! Dialoge: Einstellungen, SSH-Keys, Key-Generator, Backup, Editor, Geräte, Abfragen.

use crate::app::*;
use crate::data::{self, BaudProfile, Snippet, SshKey};
use crate::sidebar::{field, label, primary, section, toggle};
use crate::theme;
use eframe::egui::{self, vec2, Align, Color32, Key, Layout, Modifiers, RichText, ScrollArea, Stroke, TextEdit, Ui};
use nebula_core::{backup, keys, ssh};
use rfd::MessageLevel;

/// Abbrechen/Bestätigen-Zeile. Some(true) = bestätigt, Some(false) = abgebrochen.
fn buttons(ui: &mut Ui, ok: &str, enabled: bool) -> Option<bool> {
    let mut res = None;
    ui.horizontal(|ui| {
        let w = (ui.available_width() - 8.0) / 2.0;
        if ui.add(egui::Button::new(RichText::new("Abbrechen").strong()).min_size(vec2(w, 34.0))).clicked() {
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
    ui.add(
        TextEdit::multiline(&mut t)
            .font(egui::TextStyle::Monospace)
            .desired_rows(rows)
            .desired_width(f32::INFINITY)
            .interactive(true),
    );
}

fn sanitize(name: &str) -> String {
    name.chars().map(|c| if c.is_alphanumeric() || "-_.".contains(c) { c } else { '_' }).collect()
}

fn user_at_host() -> String {
    let u = std::env::var("USERNAME").unwrap_or_else(|_| "user".into());
    let h = std::env::var("COMPUTERNAME").unwrap_or_else(|_| "pc".into());
    format!("{u}@{}", h.to_lowercase())
}

impl App {
    pub fn open_settings(&mut self, page: SettingsPage) {
        self.dialog = Some(Dialog::Settings { page, prefs: self.prefs.clone(), lock1: String::new(), lock2: String::new() });
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
            alert("Key hinterlegen", "Bitte zuerst per SSH (z.B. mit Passwort) mit dem Server verbinden – dann kann der Key dort automatisch hinterlegt werden.");
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
        let Some(dir) = rfd::FileDialog::new().set_title("Ordner für den Export wählen").pick_folder() else { return };
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
            Ok(()) => self.notify(format!("Exportiert: {}", written.join(", "))),
            Err(e) => alert("Export", &e.to_string()),
        }
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
            .show(ctx, |ui| {
                match &mut dialog {
                    Dialog::AddBaud { name, rate } => {
                        ui.set_width(360.0);
                        heading(ui, "🔌", "Neues Baudrate-Profil");
                        field(ui, "Geräteklasse / Name (optional)", name, "z.B. Cisco Switch");
                        field(ui, "Baudrate *", rate, "z.B. 115200");
                        ui.add_space(8.0);
                        match buttons(ui, "Hinzufügen", true) {
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
                                _ => alert("Baudrate", "Bitte eine gültige Zahl für die Baudrate eingeben."),
                            },
                            Some(false) => keep = false,
                            None => {}
                        }
                    }
                    Dialog::AddSnippet { name, cmd } => {
                        ui.set_width(440.0);
                        heading(ui, "⚡", "Neues Makro");
                        field(ui, "Name *", name, "z.B. Speicher prüfen");
                        label(ui, "Befehl(e) *");
                        ui.add(TextEdit::multiline(cmd).font(egui::TextStyle::Monospace).desired_rows(4).desired_width(f32::INFINITY).hint_text("df -h && free -m"));
                        ui.label(RichText::new("Enter wird automatisch angehängt. Mehrere Zeilen = mehrere Befehle.").size(11.5).color(MUTED));
                        ui.add_space(10.0);
                        match buttons(ui, "Speichern", !name.trim().is_empty() && !cmd.trim().is_empty()) {
                            Some(true) => {
                                let mut c = cmd.replace("\r\n", "\n");
                                if !c.ends_with('\n') {
                                    c.push('\n');
                                }
                                self.snippets.retain(|s| s.name != name.trim());
                                self.snippets.push(Snippet { name: name.trim().into(), cmd: c, is_default: false });
                                self.save_settings();
                                keep = false;
                            }
                            Some(false) => keep = false,
                            None => {}
                        }
                    }
                    Dialog::EditHost { index, host, new_group } => {
                        ui.set_width(460.0);
                        heading(ui, if host.is_serial() { "🔌" } else { "🖧" }, if index.is_some() { "Gerät bearbeiten" } else { "Neues Gerät" });
                        ui.horizontal(|ui| {
                            let w = (ui.available_width() - 6.0) / 2.0;
                            if toggle(ui, "SSH", !host.is_serial(), w) {
                                host.kind = "ssh".into();
                            }
                            if toggle(ui, "Serial", host.is_serial(), w) {
                                host.kind = "serial".into();
                                if self.ports.is_empty() {
                                    self.ports = nebula_core::serial::list_ports();
                                }
                            }
                        });
                        ui.add_space(8.0);
                        field(ui, "Name", &mut host.name, "z.B. Core-Switch");
                        if host.is_serial() {
                            label(ui, "COM-Port");
                            egui::ComboBox::from_id_salt("eh_port").selected_text(host.port.clone()).width(ui.available_width()).show_ui(ui, |ui| {
                                for p in &self.ports {
                                    ui.selectable_value(&mut host.port, p.clone(), p);
                                }
                            });
                            ui.add_space(6.0);
                            label(ui, "Baudrate");
                            egui::ComboBox::from_id_salt("eh_baud").selected_text(host.baud.to_string()).width(ui.available_width()).show_ui(ui, |ui| {
                                for b in &self.bauds {
                                    ui.selectable_value(&mut host.baud, b.rate, &b.label);
                                }
                            });
                            ui.add_space(6.0);
                        } else {
                            field(ui, "Host / IP (optional mit :Port)", &mut host.ip, "192.168.1.50:22");
                            field(ui, "Benutzer", &mut host.user, "root");
                            label(ui, "Anmeldung");
                            ui.horizontal(|ui| {
                                let w = (ui.available_width() - 6.0) / 2.0;
                                if toggle(ui, "Passwort", host.auth_method != "key", w) {
                                    host.auth_method = "password".into();
                                }
                                if toggle(ui, "SSH-Key", host.auth_method == "key", w) {
                                    host.auth_method = "key".into();
                                }
                            });
                            ui.add_space(6.0);
                            if host.auth_method == "key" {
                                egui::ComboBox::from_id_salt("eh_key").selected_text(host.key_name.clone()).width(ui.available_width()).show_ui(ui, |ui| {
                                    for k in &self.keys {
                                        ui.selectable_value(&mut host.key_name, k.name.clone(), &k.name);
                                    }
                                });
                            } else {
                                ui.checkbox(&mut host.ask_password, "Passwort nicht speichern – beim Verbinden fragen");
                                if !host.ask_password {
                                    ui.add(TextEdit::singleline(&mut host.pass).password(true).hint_text("Passwort").desired_width(f32::INFINITY));
                                }
                            }
                            ui.add_space(6.0);
                        }
                        label(ui, "Gruppe");
                        let mut groups: Vec<String> = self.hosts.iter().map(|h| h.group.clone()).filter(|g| !g.is_empty()).collect();
                        groups.sort();
                        groups.dedup();
                        ui.horizontal(|ui| {
                            egui::ComboBox::from_id_salt("eh_group")
                                .selected_text(if host.group.is_empty() { "Ohne Gruppe".to_string() } else { host.group.clone() })
                                .width(ui.available_width() * 0.5)
                                .show_ui(ui, |ui| {
                                    ui.selectable_value(&mut host.group, String::new(), "Ohne Gruppe");
                                    for g in &groups {
                                        ui.selectable_value(&mut host.group, g.clone(), g);
                                    }
                                });
                            let r = ui.add(TextEdit::singleline(new_group).hint_text("oder neue Gruppe…").desired_width(f32::INFINITY));
                            if r.changed() && !new_group.trim().is_empty() {
                                host.group = new_group.trim().to_string();
                            }
                        });
                        ui.add_space(6.0);
                        label(ui, "Notizen");
                        ui.add(TextEdit::multiline(&mut host.notes).desired_rows(2).desired_width(f32::INFINITY).hint_text("z.B. Standort, Seriennummer, Zuständigkeit"));
                        ui.checkbox(&mut host.favorite, "★ Favorit");
                        ui.add_space(10.0);
                        match buttons(ui, "Speichern", true) {
                            Some(true) => {
                                let mut h = host.clone();
                                if h.ask_password {
                                    h.pass.clear();
                                }
                                if h.name.trim().is_empty() {
                                    h.name = if h.is_serial() { h.port.clone() } else { h.ip.clone() };
                                }
                                match index {
                                    Some(i) if *i < self.hosts.len() => self.hosts[*i] = h,
                                    _ => self.hosts.push(h),
                                }
                                match data::save_hosts(&self.hosts) {
                                    Ok(()) => self.notify("Gerät gespeichert"),
                                    Err(e) => alert("Speichern", &e),
                                }
                                keep = false;
                            }
                            Some(false) => keep = false,
                            None => {}
                        }
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
                                ui.label(RichText::new("● ungespeichert").color(AMBER).size(12.0));
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
                                        // Zeilennummern
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
                                        // Beim Öffnen direkt in den Text springen
                                        if ui.memory(|m| m.focused().is_none()) {
                                            r.request_focus();
                                        }
                                    });
                                });
                            });
                            ui.add_space(4.0);
                            ui.label(RichText::new(format!("{lines} {} · {} · UTF-8 · Strg+S speichert · Esc schließt", if lines == 1 { "Zeile" } else { "Zeilen" }, human_size(content.len() as u64))).size(11.5).color(MUTED));
                        }
                        ui.add_space(8.0);
                        let ctrl_s = ui.input_mut(|i| i.consume_key(Modifiers::CTRL, Key::S));
                        let mut save = ctrl_s;
                        let mut close = false;
                        ui.horizontal(|ui| {
                            if ui.add(egui::Button::new(RichText::new("Schließen").strong()).min_size(vec2(130.0, 34.0))).clicked() {
                                close = true;
                            }
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                let label = if *saving { "Speichere…" } else { "💾 Speichern" };
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
                                    alert("Editor", "Keine aktive SSH-Verbindung – Speichern nicht möglich.");
                                }
                            }
                        }
                        if close && (!dirty || confirm("Ungespeicherte Änderungen", "Die Änderungen wurden nicht gespeichert. Trotzdem schließen?", MessageLevel::Warning)) {
                            keep = false;
                        }
                    }
                    Dialog::Rename { tab, old, new } => {
                        ui.set_width(380.0);
                        heading(ui, "✏", "Umbenennen");
                        let r = field(ui, &format!("Neuer Name für „{old}“"), new, "");
                        r.request_focus();
                        let enter = (r.lost_focus() || r.has_focus()) && ui.input(|i| i.key_pressed(Key::Enter));
                        ui.add_space(6.0);
                        let res = buttons(ui, "Umbenennen", !new.trim().is_empty());
                        if (res == Some(true) || enter) && !new.trim().is_empty() && new != old {
                            let id = *tab;
                            let dir = self.tabs.iter().find(|t| t.id == id).map(|t| t.sftp.path.clone()).unwrap_or_default();
                            let (o, n) = (ssh::join(&dir, old), ssh::join(&dir, new.trim()));
                            self.sftp_op(id, "Umbenennen", move |s| async move { ssh::rename(&s, &o, &n).await });
                            keep = false;
                        } else if res.is_some() {
                            keep = false;
                        }
                    }
                    Dialog::MkDir { tab, name } => {
                        ui.set_width(380.0);
                        heading(ui, "📁", "Neuer Ordner");
                        let r = field(ui, "Ordnername", name, "z.B. backup");
                        r.request_focus();
                        let enter = (r.lost_focus() || r.has_focus()) && ui.input(|i| i.key_pressed(Key::Enter));
                        ui.add_space(6.0);
                        let res = buttons(ui, "Anlegen", !name.trim().is_empty());
                        if (res == Some(true) || enter) && !name.trim().is_empty() {
                            let id = *tab;
                            let dir = self.tabs.iter().find(|t| t.id == id).map(|t| t.sftp.path.clone()).unwrap_or_default();
                            let p = ssh::join(&dir, name.trim());
                            self.sftp_op(id, "Ordner anlegen", move |s| async move { ssh::mkdir(&s, &p).await });
                            keep = false;
                        } else if res.is_some() {
                            keep = false;
                        }
                    }
                    Dialog::NewFile { tab, name } => {
                        ui.set_width(380.0);
                        heading(ui, "📄", "Neue Datei");
                        let r = field(ui, "Dateiname", name, "z.B. notizen.txt");
                        r.request_focus();
                        let enter = (r.lost_focus() || r.has_focus()) && ui.input(|i| i.key_pressed(Key::Enter));
                        ui.add_space(6.0);
                        let res = buttons(ui, "Erstellen & bearbeiten", !name.trim().is_empty());
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
                        heading(ui, "🔐", "Rechte ändern");
                        ui.label(RichText::new(name.as_str()).monospace());
                        ui.add_space(8.0);
                        // Kästchen für Besitzer / Gruppe / Andere
                        let mut bits: Vec<u32> = mode.chars().filter_map(|c| c.to_digit(8)).collect();
                        bits.resize(3, 0);
                        egui::Grid::new("chmod").spacing(vec2(18.0, 6.0)).show(ui, |ui| {
                            ui.label("");
                            ui.label(RichText::new("Lesen").color(MUTED));
                            ui.label(RichText::new("Schreiben").color(MUTED));
                            ui.label(RichText::new("Ausführen").color(MUTED));
                            ui.end_row();
                            for (i, who) in ["Besitzer", "Gruppe", "Andere"].iter().enumerate() {
                                ui.label(*who);
                                for (bit, _) in [(4, 'r'), (2, 'w'), (1, 'x')] {
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
                            ui.label("Oktal:");
                            ui.add(TextEdit::singleline(mode).desired_width(60.0).font(egui::TextStyle::Monospace));
                        });
                        ui.add_space(10.0);
                        match buttons(ui, "Übernehmen", true) {
                            Some(true) => match u32::from_str_radix(mode.trim(), 8) {
                                Ok(m) if m <= 0o7777 => {
                                    let (id, p) = (*tab, path.clone());
                                    self.sftp_op(id, "Rechte ändern", move |s| async move { ssh::chmod(&s, &p, m).await });
                                    keep = false;
                                }
                                _ => alert("Rechte", "Ungültiger Oktalwert, z.B. 644 oder 755."),
                            },
                            Some(false) => keep = false,
                            None => {}
                        }
                    }
                    Dialog::Update => {
                        keep = self.update_dialog(ui);
                    }
                    Dialog::Keys { selected } => {
                        keep = self.keys_dialog(ui, selected);
                    }
                    Dialog::KeyGen { kind, name, comment, pass1, pass2, busy, result, install_status } => {
                        ui.set_width(600.0);
                        match result {
                            None => {
                                heading(ui, "🔑", "Neuen SSH-Key erzeugen");
                                label(ui, "Schlüsseltyp");
                                ui.horizontal(|ui| {
                                    for k in [keys::KeyType::Ed25519, keys::KeyType::Rsa4096, keys::KeyType::EcdsaP256] {
                                        ui.selectable_value(kind, k, k.label());
                                    }
                                });
                                ui.add_space(6.0);
                                field(ui, "Name in NebulaSSH", name, "z.B. Produktion");
                                field(ui, "Kommentar (erscheint auf dem Server)", comment, "benutzer@pc");
                                label(ui, "Passphrase (optional, empfohlen)");
                                ui.horizontal(|ui| {
                                    let w = (ui.available_width() - 6.0) / 2.0;
                                    ui.add(TextEdit::singleline(pass1).password(true).hint_text("Passphrase").desired_width(w));
                                    ui.add(TextEdit::singleline(pass2).password(true).hint_text("wiederholen").desired_width(w));
                                });
                                if !pass1.is_empty() && pass1 != pass2 {
                                    ui.label(RichText::new("Die Passphrasen stimmen nicht überein.").color(RED).size(12.0));
                                }
                                ui.label(RichText::new("Mit Passphrase ist der Key auch dann geschützt, wenn jemand die Datei kopiert. NebulaSSH fragt sie beim Verbinden ab.").size(11.5).color(MUTED));
                                ui.add_space(12.0);
                                if *busy {
                                    ui.horizontal(|ui| {
                                        ui.spinner();
                                        ui.label("Erzeuge Schlüssel…");
                                    });
                                } else {
                                    let ok = !name.trim().is_empty() && (pass1.is_empty() || pass1 == pass2);
                                    match buttons(ui, "Erzeugen", ok) {
                                        Some(true) => {
                                            if self.keys.iter().any(|k| k.name == name.trim())
                                                && !confirm("SSH-Key", "Es gibt bereits einen Key mit diesem Namen. Ersetzen?", MessageLevel::Warning)
                                            {
                                            } else {
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
                                heading(ui, "✅", "SSH-Key erzeugt");
                                ui.label(RichText::new(format!("Gespeichert als „{}“ · {}", name, k.fingerprint)).color(MUTED).size(12.0));
                                ui.add_space(10.0);
                                section(ui, "1. Public Key (öffentlich – darf weitergegeben werden)");
                                code_box(ui, &k.public_line, 2);
                                ui.horizontal(|ui| {
                                    if ui.button("📋 Public Key kopieren").clicked() {
                                        ctx.copy_text(k.public_line.clone());
                                        self.notify("Public Key kopiert");
                                    }
                                    let has_ssh = self.active_ssh().is_some();
                                    if ui
                                        .add_enabled(has_ssh, egui::Button::new(RichText::new("⚡ Auf verbundenem Server hinterlegen").color(Color32::WHITE)).fill(ACCENT))
                                        .on_disabled_hover_text("Zuerst per SSH mit dem Server verbinden")
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
                                section(ui, "2. Manuell auf einem Linux-Server hinterlegen");
                                let instr = keys::install_instructions(&k.public_line);
                                code_box(ui, &instr, 8);
                                if ui.button("📋 Befehle kopieren").clicked() {
                                    ctx.copy_text(instr);
                                    self.notify("Befehle kopiert");
                                }
                                ui.add_space(8.0);
                                section(ui, "3. Danach verbinden");
                                ui.label(RichText::new("Im Verbindungsformular „SSH-Key“ wählen und diesen Key auswählen. Funktioniert der Login, kann die Passwort-Anmeldung auf dem Server deaktiviert werden (PasswordAuthentication no in /etc/ssh/sshd_config).").size(12.5).color(MUTED));
                                ui.add_space(12.0);
                                ui.horizontal(|ui| {
                                    if ui.button("💾 Key exportieren…").clicked() {
                                        let key = SshKey { name: name.clone(), content: k.private_openssh.clone() };
                                        self.export_key(&key);
                                    }
                                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                        if primary(ui, "Fertig", 120.0).clicked() {
                                            keep = false;
                                        }
                                    });
                                });
                            }
                        }
                    }
                    Dialog::ImportKey { name, content } => {
                        ui.set_width(520.0);
                        heading(ui, "📥", "SSH-Key importieren");
                        field(ui, "Name *", name, "z.B. Produktion");
                        label(ui, "Private Key");
                        ScrollArea::vertical().max_height(200.0).show(ui, |ui| {
                            ui.add(TextEdit::multiline(content).font(egui::TextStyle::Monospace).hint_text("-----BEGIN OPENSSH PRIVATE KEY-----").desired_rows(9).desired_width(f32::INFINITY));
                        });
                        ui.add_space(6.0);
                        if ui.button("📄 Aus Datei laden (id_ed25519, id_rsa, .pem)").clicked() {
                            if let Some(p) = rfd::FileDialog::new().set_title("Private Key auswählen").pick_file() {
                                match std::fs::metadata(&p) {
                                    Ok(m) if m.len() > 1024 * 1024 => alert("SSH-Key", "Die Datei ist zu groß für einen SSH-Key."),
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
                        if !content.trim().is_empty() {
                            match keys::public_from_private(content) {
                                Ok((_, fp)) => ui.label(RichText::new(format!("✔ Gültiger Key · {fp}")).color(GREEN).size(12.0)),
                                Err(_) if keys::is_encrypted(content) => ui.label(RichText::new("🔒 Verschlüsselter Key – Passphrase wird beim Verbinden abgefragt").color(AMBER).size(12.0)),
                                Err(e) => ui.label(RichText::new(format!("✖ {e}")).color(RED).size(12.0)),
                            };
                        }
                        ui.add_space(10.0);
                        match buttons(ui, "Importieren", !name.trim().is_empty() && !content.trim().is_empty()) {
                            Some(true) => {
                                let n = name.trim().to_string();
                                self.keys.retain(|k| k.name != n);
                                self.keys.push(SshKey { name: n.clone(), content: content.clone() });
                                self.form.key_name = n;
                                match data::save_keys(&self.keys) {
                                    Ok(()) => self.notify("Key importiert"),
                                    Err(e) => alert("Speichern", &e),
                                }
                                keep = false;
                            }
                            Some(false) => keep = false,
                            None => {}
                        }
                    }
                    Dialog::Settings { page, prefs, lock1, lock2 } => {
                        keep = self.settings_dialog(ui, page, prefs, lock1, lock2);
                    }
                    Dialog::Backup { export, pass1, pass2, status } => {
                        ui.set_width(460.0);
                        if *export {
                            heading(ui, "📦", "Backup erstellen");
                            ui.label(RichText::new("Sichert alle Geräte, SSH-Keys, Makros, Einstellungen und bekannten Host-Keys in eine verschlüsselte Datei – ideal für den Umzug auf einen neuen PC.").color(MUTED));
                            ui.add_space(10.0);
                            label(ui, "Backup-Passwort (mind. 8 Zeichen)");
                            ui.add(TextEdit::singleline(pass1).password(true).desired_width(f32::INFINITY));
                            ui.add(TextEdit::singleline(pass2).password(true).hint_text("wiederholen").desired_width(f32::INFINITY));
                            if !pass2.is_empty() && pass1 != pass2 {
                                ui.label(RichText::new("Passwörter stimmen nicht überein.").color(RED).size(12.0));
                            }
                            ui.label(RichText::new("Ohne dieses Passwort lässt sich das Backup nicht wiederherstellen.").size(11.5).color(AMBER));
                        } else {
                            heading(ui, "♻", "Backup wiederherstellen");
                            ui.label(RichText::new("Achtung: Vorhandene Geräte, Keys und Einstellungen auf diesem PC werden ersetzt.").color(AMBER));
                            ui.add_space(10.0);
                            label(ui, "Backup-Passwort");
                            ui.add(TextEdit::singleline(pass1).password(true).desired_width(f32::INFINITY));
                        }
                        if let Some(s) = status {
                            ui.label(RichText::new(s.as_str()).color(RED));
                        }
                        ui.add_space(12.0);
                        let ok = if *export { pass1.chars().count() >= 8 && pass1 == pass2 } else { !pass1.is_empty() };
                        match buttons(ui, if *export { "Speichern unter…" } else { "Datei wählen…" }, ok) {
                            Some(true) if *export => match backup::export(pass1) {
                                Ok((bytes, sum)) => {
                                    let name = format!("NebulaSSH-Backup_{}.nebula", chrono::Local::now().format("%Y-%m-%d"));
                                    if let Some(p) = rfd::FileDialog::new().set_title("Backup speichern").set_file_name(&name).add_filter("NebulaSSH-Backup", &["nebula"]).save_file() {
                                        match std::fs::write(&p, bytes) {
                                            Ok(()) => {
                                                self.notify(format!("Backup gespeichert: {} Geräte, {} Keys", sum.hosts, sum.keys));
                                                keep = false;
                                            }
                                            Err(e) => *status = Some(e.to_string()),
                                        }
                                    }
                                }
                                Err(e) => *status = Some(e),
                            },
                            Some(true) => {
                                if let Some(p) = rfd::FileDialog::new().set_title("Backup öffnen").add_filter("NebulaSSH-Backup", &["nebula"]).pick_file() {
                                    match std::fs::read(&p).map_err(|e| e.to_string()).and_then(|b| backup::import(&b, pass1)) {
                                        Ok(sum) => {
                                            // Alles neu laden
                                            self.hosts = data::load_hosts();
                                            self.keys = data::load_keys();
                                            let s = data::load_settings();
                                            self.snippets = s.snippets;
                                            self.bauds = s.bauds;
                                            self.prefs = s.prefs;
                                            self.notify(format!("Backup wiederhergestellt: {} Geräte, {} Keys", sum.hosts, sum.keys));
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
                    Dialog::Ask { tab, passphrase, value, remember } => {
                        ui.set_width(380.0);
                        let title = self.tabs.iter().find(|t| t.id == *tab).map(|t| t.title.clone()).unwrap_or_default();
                        heading(ui, "🔒", if *passphrase { "Passphrase für SSH-Key" } else { "Passwort eingeben" });
                        ui.label(RichText::new(format!("Verbindung: {title}")).color(MUTED));
                        ui.add_space(8.0);
                        let r = ui.add(TextEdit::singleline(value).password(true).hint_text(if *passphrase { "Passphrase" } else { "Passwort" }).desired_width(f32::INFINITY));
                        r.request_focus();
                        let enter = (r.lost_focus() || r.has_focus()) && ui.input(|i| i.key_pressed(Key::Enter));
                        if !*passphrase {
                            ui.checkbox(remember, "Passwort beim Gerät speichern");
                        }
                        ui.add_space(10.0);
                        let res = buttons(ui, "Verbinden", !value.is_empty());
                        if (res == Some(true) || enter) && !value.is_empty() {
                            let (id, v, rem, pp) = (*tab, value.clone(), *remember, *passphrase);
                            keep = false;
                            self.dialog = None;
                            if let Some(t) = self.tab(id) {
                                t.secret = v.clone();
                                if rem && !pp {
                                    t.host.pass = v.clone();
                                    t.host.ask_password = false;
                                    let h = t.host.clone();
                                    if let Some(x) = self.hosts.iter_mut().find(|x| x.same_device(&h)) {
                                        x.pass = v;
                                        x.ask_password = false;
                                        let _ = data::save_hosts(&self.hosts);
                                    }
                                }
                            }
                            self.start(id);
                        } else if res == Some(false) {
                            let id = *tab;
                            if let Some(t) = self.tab(id) {
                                t.status = Status::Closed;
                                t.term.print("\x1b[90m[Anmeldung abgebrochen]\x1b[0m\n");
                            }
                            keep = false;
                        }
                    }
                }
            });
        let _ = resp;
        if keep && self.dialog.is_none() {
            self.dialog = Some(dialog);
        } else if !keep {
            self.focus_term = true;
        }
    }

    fn keys_dialog(&mut self, ui: &mut Ui, selected: &mut Option<usize>) -> bool {
        let mut keep = true;
        ui.set_width(620.0);
        heading(ui, "🔑", "SSH-Keys");
        ui.horizontal(|ui| {
            if ui.add(egui::Button::new(RichText::new("✨ Neuen Key erzeugen").color(Color32::WHITE)).fill(ACCENT)).clicked() {
                self.open_keygen();
                keep = false;
            }
            if ui.button("📥 Importieren…").clicked() {
                self.dialog = Some(Dialog::ImportKey { name: String::new(), content: String::new() });
                keep = false;
            }
        });
        ui.add_space(10.0);
        if self.keys.is_empty() {
            ui.label(RichText::new("Noch keine Keys gespeichert. Erzeuge einen neuen Key – NebulaSSH zeigt dir danach, wie du ihn auf deinem Linux-Server hinterlegst.").color(MUTED));
        }
        let mut delete = None;
        ScrollArea::vertical().max_height(260.0).show(ui, |ui| {
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
                                    Err(_) if keys::is_encrypted(&k.content) => "verschlüsselt (Passphrase)".into(),
                                    Err(_) => "Format unbekannt".into(),
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
            ui.label(RichText::new(format!("Ausgewählt: {}", k.name)).strong());
            ui.horizontal_wrapped(|ui| {
                if let Ok((line, _)) = keys::public_from_private(&k.content) {
                    if ui.button("📋 Public Key kopieren").clicked() {
                        ui.ctx().copy_text(line.clone());
                        self.notify("Public Key kopiert");
                    }
                    if ui.add_enabled(self.active_ssh().is_some(), egui::Button::new("⚡ Auf verbundenem Server hinterlegen")).on_disabled_hover_text("Zuerst per SSH verbinden").clicked() {
                        self.install_key_on_active(line.clone());
                    }
                    if ui.button("📋 Anleitung kopieren").clicked() {
                        ui.ctx().copy_text(keys::install_instructions(&line));
                        self.notify("Anleitung kopiert");
                    }
                }
                if ui.button("💾 Exportieren…").clicked() {
                    self.export_key(&k);
                }
                if ui.button(RichText::new("🗑 Löschen").color(RED)).clicked() {
                    delete = Some(i);
                }
            });
        }
        if let Some(i) = delete {
            let name = self.keys[i].name.clone();
            let used = self.hosts.iter().filter(|h| h.key_name == name).count();
            let warn = if used > 0 { format!("\n\n{used} gespeicherte(s) Gerät(e) verwenden diesen Key.") } else { String::new() };
            if confirm("SSH-Key löschen", &format!("Key „{name}“ wirklich löschen?{warn}\n\nTipp: Vorher exportieren!"), MessageLevel::Warning) {
                self.keys.remove(i);
                *selected = None;
                if let Err(e) = data::save_keys(&self.keys) {
                    alert("Speichern", &e);
                }
            }
        }
        ui.add_space(12.0);
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if primary(ui, "Schließen", 120.0).clicked() {
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
        heading(ui, "⚙", "Einstellungen");
        ui.allocate_ui_with_layout(vec2(720.0, 440.0), Layout::left_to_right(Align::Min), |ui| {
            // Navigation
            ui.vertical(|ui| {
                ui.set_width(150.0);
                for (pg, label) in [
                    (SettingsPage::Terminal, "🖥  Terminal"),
                    (SettingsPage::Connection, "🌐  Verbindung"),
                    (SettingsPage::Security, "🛡  Sicherheit"),
                    (SettingsPage::Sessions, "🗂  Sitzungen"),
                    (SettingsPage::Backup, "📦  Daten & Backup"),
                    (SettingsPage::About, "ℹ  Über"),
                ] {
                    let on = *page == pg;
                    let b = egui::Button::new(RichText::new(label).color(if on { Color32::WHITE } else { Color32::from_gray(200) }))
                        .fill(if on { ACCENT } else { Color32::TRANSPARENT })
                        .min_size(vec2(150.0, 32.0));
                    if ui.add(b).clicked() {
                        *page = pg;
                    }
                }
            });
            ui.add_space(14.0);
            ScrollArea::vertical().max_height(440.0).auto_shrink([false, false]).show(ui, |ui| {
                ui.with_layout(Layout::top_down(Align::Min), |ui| {
                ui.set_width(540.0);
                match page {
                    SettingsPage::Terminal => {
                        section(ui, "Darstellung");
                        ui.add(egui::Slider::new(&mut p.font_size, 9.0..=28.0).text("Schriftgröße").step_by(1.0));
                        ui.add_space(6.0);
                        ui.horizontal(|ui| {
                            ui.label("Farbschema");
                            egui::ComboBox::from_id_salt("theme").selected_text(theme::by_id(&p.theme).name).show_ui(ui, |ui| {
                                for t in theme::THEMES {
                                    ui.selectable_value(&mut p.theme, t.id.to_string(), t.name);
                                }
                            });
                        });
                        // Vorschau
                        let th = theme::by_id(&p.theme);
                        egui::Frame::new().fill(th.bg).corner_radius(6).inner_margin(8).show(ui, |ui| {
                            ui.horizontal(|ui| {
                                ui.label(RichText::new("user@server:~$ ").monospace().color(th.ansi[10]));
                                ui.label(RichText::new("ls ").monospace().color(th.fg));
                                ui.label(RichText::new("ordner ").monospace().color(th.ansi[12]));
                                ui.label(RichText::new("fehler ").monospace().color(th.ansi[9]));
                                ui.label(RichText::new("warnung").monospace().color(th.ansi[11]));
                            });
                        });
                        ui.add_space(6.0);
                        ui.horizontal(|ui| {
                            ui.label("Cursor");
                            ui.selectable_value(&mut p.cursor, "block".into(), "█ Block");
                            ui.selectable_value(&mut p.cursor, "beam".into(), "▏ Strich");
                            ui.selectable_value(&mut p.cursor, "underline".into(), "▁ Unterstrich");
                        });
                        ui.add_space(10.0);
                        section(ui, "Verhalten");
                        ui.checkbox(&mut p.copy_on_select, "Markierten Text automatisch kopieren");
                        ui.checkbox(&mut p.right_click_paste, "Rechtsklick fügt ein");
                        ui.horizontal(|ui| {
                            ui.label("Verlauf (Zeilen)");
                            ui.add(egui::DragValue::new(&mut p.scrollback).range(1_000..=1_000_000).speed(1000));
                        });
                    }
                    SettingsPage::Connection => {
                        section(ui, "SSH");
                        ui.horizontal(|ui| {
                            ui.label("Keepalive alle");
                            ui.add(egui::DragValue::new(&mut p.keepalive).range(0..=600).suffix(" s"));
                            ui.label(RichText::new("(0 = aus)").color(MUTED));
                        });
                        ui.checkbox(&mut p.legacy_algorithms, "Ältere Verfahren erlauben (SHA1, CBC, 3DES) – für alte Switches/Router");
                        ui.label(RichText::new("Moderne Verfahren werden immer bevorzugt. Abschalten erhöht die Sicherheit, kann aber alte Geräte aussperren.").size(11.5).color(MUTED));
                        ui.add_space(8.0);
                        ui.horizontal(|ui| {
                            ui.label("Standard-Benutzer");
                            ui.add(TextEdit::singleline(&mut p.default_user).desired_width(160.0));
                        });
                        ui.checkbox(&mut p.confirm_close, "Vor dem Schließen aktiver Verbindungen nachfragen");
                    }
                    SettingsPage::Security => {
                        section(ui, "Server-Identität");
                        ui.checkbox(&mut p.verify_new_hosts, "Bei neuen Servern den Fingerprint bestätigen lassen");
                        ui.label(RichText::new("Geänderte Host-Keys werden immer gemeldet (Schutz vor Man-in-the-Middle).").size(11.5).color(MUTED));
                        ui.add_space(6.0);
                        egui::CollapsingHeader::new(format!("Bekannte Server ({})", ssh::known_hosts().len())).show(ui, |ui| {
                            ScrollArea::vertical().max_height(120.0).show(ui, |ui| {
                                for (host, fp) in ssh::known_hosts() {
                                    ui.horizontal(|ui| {
                                        if ui.small_button("🗑").on_hover_text("Vergessen").clicked() {
                                            ssh::forget_host_key(&host);
                                        }
                                        ui.label(RichText::new(&host).monospace().size(12.0));
                                        ui.label(RichText::new(&fp).monospace().size(10.5).color(MUTED));
                                    });
                                }
                            });
                        });
                        ui.add_space(8.0);
                        section(ui, "Zwischenablage");
                        ui.horizontal(|ui| {
                            ui.label("Kopierte Passwörter löschen nach");
                            ui.add(egui::DragValue::new(&mut p.clipboard_clear).range(0..=600).suffix(" s"));
                        });
                        ui.add_space(8.0);
                        section(ui, "App-Sperre");
                        if p.lock_hash.is_some() {
                            ui.label(RichText::new("✔ NebulaSSH ist mit einem App-Passwort geschützt.").color(GREEN));
                            ui.horizontal(|ui| {
                                ui.label("Automatisch sperren nach");
                                ui.add(egui::DragValue::new(&mut p.autolock_minutes).range(0..=240).suffix(" min"));
                                ui.label(RichText::new("(0 = nie)").color(MUTED));
                            });
                            if ui.button("App-Passwort entfernen").clicked() && confirm("App-Sperre", "App-Passwort wirklich entfernen?", MessageLevel::Warning) {
                                p.lock_hash = None;
                            }
                        } else {
                            ui.label(RichText::new("Schützt den Start von NebulaSSH mit einem Passwort (z.B. auf geteilten PCs).").size(12.0).color(MUTED));
                            ui.horizontal(|ui| {
                                ui.add(TextEdit::singleline(lock1).password(true).hint_text("Neues App-Passwort").desired_width(170.0));
                                ui.add(TextEdit::singleline(lock2).password(true).hint_text("wiederholen").desired_width(170.0));
                                let ok = lock1.chars().count() >= 4 && lock1 == lock2;
                                if ui.add_enabled(ok, egui::Button::new("Aktivieren")).clicked() {
                                    match backup::hash_password(lock1) {
                                        Ok(h) => {
                                            p.lock_hash = Some(h);
                                            lock1.clear();
                                            lock2.clear();
                                        }
                                        Err(e) => alert("App-Sperre", &e),
                                    }
                                }
                            });
                        }
                    }
                    SettingsPage::Sessions => {
                        section(ui, "Beim Start");
                        ui.checkbox(&mut p.restore_tabs, "Offene Verbindungen beim nächsten Start wiederherstellen");
                        ui.checkbox(&mut p.splash, "Startbildschirm anzeigen");
                        ui.add_space(8.0);
                        section(ui, "Protokollierung");
                        ui.checkbox(&mut p.log_sessions, "Alle Sitzungen als Textdatei mitschreiben");
                        ui.horizontal(|ui| {
                            let shown = if p.log_dir.is_empty() {
                                nebula_core::storage::data_dir().join("logs").to_string_lossy().to_string()
                            } else {
                                p.log_dir.clone()
                            };
                            ui.label(RichText::new(shown).monospace().size(11.5).color(MUTED));
                        });
                        ui.horizontal(|ui| {
                            if ui.button("Ordner wählen…").clicked() {
                                if let Some(d) = rfd::FileDialog::new().pick_folder() {
                                    p.log_dir = d.to_string_lossy().to_string();
                                }
                            }
                            if ui.button("Ordner öffnen").clicked() {
                                let dir = if p.log_dir.is_empty() { nebula_core::storage::data_dir().join("logs") } else { p.log_dir.clone().into() };
                                let _ = std::fs::create_dir_all(&dir);
                                let _ = std::process::Command::new("explorer").arg(dir).spawn();
                            }
                        });
                        ui.label(RichText::new("Gilt für neu geöffnete Verbindungen. Logs können Passwörter enthalten, die im Terminal eingegeben werden.").size(11.5).color(AMBER));
                    }
                    SettingsPage::Backup => {
                        section(ui, "Umzug auf einen neuen PC");
                        ui.label(RichText::new("Ein Backup enthält Geräte, SSH-Keys, Makros, Einstellungen und bekannte Server – verschlüsselt mit einem Passwort deiner Wahl.").color(MUTED));
                        ui.add_space(8.0);
                        ui.horizontal(|ui| {
                            if ui.add(egui::Button::new(RichText::new("📦 Backup erstellen…").color(Color32::WHITE)).fill(ACCENT)).clicked() {
                                self.pending_backup = Some(true);
                            }
                            if ui.button("♻ Backup wiederherstellen…").clicked() {
                                self.pending_backup = Some(false);
                            }
                        });
                        ui.add_space(12.0);
                        section(ui, "Speicherort");
                        let dir = nebula_core::storage::data_dir();
                        ui.label(RichText::new(dir.to_string_lossy()).monospace().size(12.0));
                        if ui.button("Datenordner öffnen").clicked() {
                            let _ = std::process::Command::new("explorer").arg(&dir).spawn();
                        }
                        ui.label(RichText::new("hosts.json und keys.json sind mit AES-256 verschlüsselt; der Schlüssel liegt in der Windows-Anmeldeinformationsverwaltung dieses Benutzers.").size(11.5).color(MUTED));
                    }
                    SettingsPage::About => {
                        ui.vertical_centered(|ui| {
                            ui.add_space(10.0);
                            ui.add(egui::Image::new(&self.logo).fit_to_exact_size(vec2(96.0, 96.0)));
                            ui.add_space(8.0);
                            ui.label(RichText::new("NebulaSSH").size(24.0).family(egui::FontFamily::Name("ui-bold".into())).color(Color32::WHITE));
                            ui.label(RichText::new(format!("Version {VERSION}")).color(MUTED));
                            ui.add_space(8.0);
                            ui.label("SSH-, SFTP- und Serial-Client für Windows");
                            ui.label(RichText::new("by Thomas Schöpf").color(LAVENDER));
                            ui.add_space(14.0);
                            ui.label(RichText::new("Gebaut mit Rust · egui · russh · alacritty_terminal").size(12.0).color(MUTED));
                        });
                        self.update_settings(ui, p);
                    }
                }
                });
            });
        });
        ui.add_space(12.0);
        let res = buttons(ui, "Übernehmen", true);
        match res {
            Some(true) => {
                let changed_term = p.scrollback != self.prefs.scrollback || p.cursor != self.prefs.cursor;
                self.prefs = p.clone();
                self.save_settings();
                if changed_term {
                    for t in &self.tabs {
                        t.term.apply_prefs(self.prefs.scrollback, &self.prefs.cursor);
                    }
                }
                self.notify("Einstellungen gespeichert");
                keep = false;
            }
            Some(false) => keep = false,
            None => {}
        }
        if let Some(export) = self.pending_backup.take() {
            // Einstellungen übernehmen und zum Backup-Dialog wechseln
            self.prefs = p.clone();
            self.save_settings();
            self.dialog = Some(Dialog::Backup { export, pass1: String::new(), pass2: String::new(), status: None });
            return false;
        }
        keep
    }
}
