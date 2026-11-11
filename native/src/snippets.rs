//! Snippet-Bibliothek: gespeicherte Befehle mit Platzhaltern, durchsuchbar (Strg+Umschalt+P),
//! sowie der Tunnel-Dialog für laufende Verbindungen.

use crate::app::*;
use crate::data::{self, Snippet, AUTO_PLACEHOLDERS};
use crate::sidebar::{field, label, segmented};
use eframe::egui::{self, vec2, Align, Color32, Key, Layout, RichText, ScrollArea, TextEdit, Ui};
use nebula_core::ssh::Forward;
use nebula_core::{t, tf};

use crate::dialogs::heading;

impl App {
    pub fn open_snippets(&mut self) {
        self.dialog = Some(Dialog::Snippets { query: String::new(), edit: None, focus: true });
    }

    /// Führt ein Snippet aus; fragt vorher fehlende Platzhalter ab.
    pub fn run_snippet(&mut self, s: Snippet) {
        let auto = self.auto_values();
        let missing: Vec<(String, String)> = s
            .placeholders()
            .into_iter()
            .filter(|p| !AUTO_PLACEHOLDERS.contains(&p.as_str()))
            .map(|p| (p, String::new()))
            .collect();
        if missing.is_empty() {
            let cmd = s.fill(&auto);
            self.send_text(cmd);
        } else {
            let enter = s.cmd.ends_with('\n');
            self.dialog = Some(Dialog::RunSnippet { snippet: s, values: missing, enter });
        }
    }

    fn auto_values(&self) -> Vec<(String, String)> {
        let host = self.active.and_then(|id| self.tabs.iter().find(|t| t.id == id)).map(|t| t.host.clone()).unwrap_or_default();
        vec![("host".into(), host.host_only()), ("ip".into(), host.ip.clone()), ("user".into(), host.user.clone())]
    }

    pub fn snippets_dialog(&mut self, ui: &mut Ui, query: &mut String, edit: &mut Option<(Option<usize>, Snippet)>, focus: &mut bool) -> bool {
        let mut keep = true;
        ui.set_width(680.0);
        if let Some((idx, s)) = edit {
            heading(ui, Icon::Bolt, if idx.is_some() { t!("Snippet bearbeiten", "Edit snippet") } else { t!("Neues Snippet", "New snippet") });
            field(ui, t!("Name *", "Name *"), &mut s.name, t!("z.B. Speicher prüfen", "e.g. check memory"));
            field(ui, t!("Gruppe", "Group"), &mut s.group, t!("z.B. Docker", "e.g. Docker"));
            label(ui, t!("Befehl(e) *", "Command(s) *"));
            ui.add(TextEdit::multiline(&mut s.cmd).font(egui::TextStyle::Monospace).desired_rows(5).desired_width(f32::INFINITY).hint_text("df -h && free -m"));
            ui.label(
                RichText::new(t!(
                    "Platzhalter: {{name}} wird vor dem Ausführen abgefragt. {{host}}, {{ip}} und {{user}} werden automatisch mit den Daten der aktiven Verbindung gefüllt.",
                    "Placeholders: {{name}} is asked for before running. {{host}}, {{ip}} and {{user}} are filled in from the active connection."
                ))
                .size(11.5)
                .color(MUTED),
            );
            let mut enter = s.cmd.ends_with('\n');
            if ui.checkbox(&mut enter, t!("Sofort ausführen (Enter anhängen)", "Run immediately (append Enter)")).changed() {
                if enter {
                    s.cmd.push('\n');
                } else {
                    while s.cmd.ends_with('\n') {
                        s.cmd.pop();
                    }
                }
            }
            ui.checkbox(&mut s.pinned, t!("In der Statusleiste anzeigen", "Show in the status bar"));
            ui.add_space(10.0);
            let mut done = None;
            ui.horizontal(|ui| {
                if ui.add(egui::Button::new(RichText::new(t!("Zurück", "Back")).strong()).min_size(vec2(130.0, 34.0))).clicked() || ui.input(|i| i.key_pressed(Key::Escape)) {
                    done = Some(false);
                }
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    let ok = !s.name.trim().is_empty() && !s.cmd.trim().is_empty();
                    if ui.add_enabled(ok, egui::Button::new(RichText::new(t!("Speichern", "Save")).strong().color(Color32::WHITE)).fill(ACCENT).min_size(vec2(140.0, 34.0))).clicked() {
                        done = Some(true);
                    }
                });
            });
            match done {
                Some(true) => {
                    let mut n = s.clone();
                    n.name = n.name.trim().to_string();
                    n.cmd = n.cmd.replace("\r\n", "\n");
                    n.is_default = false;
                    match idx {
                        Some(i) if *i < self.snippets.len() && self.snippets[*i].is_default == false => self.snippets[*i] = n,
                        _ => {
                            self.snippets.retain(|x| x.name != n.name || x.is_default);
                            self.snippets.push(n);
                        }
                    }
                    self.save_settings();
                    *edit = None;
                }
                Some(false) => *edit = None,
                None => {}
            }
            return keep;
        }

        heading(ui, Icon::Bolt, t!("Snippets", "Snippets"));
        let mut run: Option<Snippet> = None;
        let mut insert: Option<Snippet> = None;
        let mut new_edit: Option<(Option<usize>, Snippet)> = None;
        let mut delete = None;
        let mut pin = None;
        ui.horizontal(|ui| {
            let r = ui.add(
                TextEdit::singleline(query)
                    .hint_text(t!("Suchen – Enter führt den ersten Treffer aus", "Search – Enter runs the first match"))
                    .desired_width(ui.available_width() - 150.0),
            );
            if *focus {
                r.request_focus();
                *focus = false;
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui.add(egui::Button::new(RichText::new(t!("+ Neues Snippet", "+ New snippet")).color(Color32::WHITE)).fill(ACCENT)).clicked() {
                    new_edit = Some((None, Snippet { name: String::new(), cmd: String::new(), is_default: false, group: String::new(), pinned: true }));
                }
            });
        });
        ui.add_space(6.0);
        let q = query.to_lowercase();
        let hits: Vec<usize> = (0..self.snippets.len())
            .filter(|&i| {
                let s = &self.snippets[i];
                q.is_empty() || s.name.to_lowercase().contains(&q) || s.cmd.to_lowercase().contains(&q) || s.group.to_lowercase().contains(&q)
            })
            .collect();
        if ui.input(|i| i.key_pressed(Key::Enter)) {
            if let Some(&i) = hits.first() {
                run = Some(self.snippets[i].clone());
            }
        }
        let mut groups: Vec<String> = hits.iter().map(|&i| self.snippets[i].group.clone()).collect();
        groups.sort_by_key(|g| (g.is_empty(), g.to_lowercase()));
        groups.dedup();
        ScrollArea::vertical().max_height(380.0).auto_shrink([false, true]).show(ui, |ui| {
            if hits.is_empty() {
                ui.label(RichText::new(t!("Keine Treffer.", "No matches.")).color(MUTED));
            }
            for g in groups {
                ui.label(RichText::new(if g.is_empty() { t!("Allgemein", "General").to_string() } else { g.clone() }).size(12.0).strong().color(MUTED));
                for &i in hits.iter().filter(|&&i| self.snippets[i].group == g) {
                    let s = &self.snippets[i];
                    egui::Frame::new().fill(PANEL2).corner_radius(8).inner_margin(egui::Margin::symmetric(10, 6)).show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        ui.horizontal(|ui| {
                            ui.vertical(|ui| {
                                ui.spacing_mut().item_spacing.y = 2.0;
                                ui.horizontal(|ui| {
                                    ui.label(RichText::new(&s.name).strong().color(Color32::WHITE));
                                    if s.pinned {
                                        icons::show(ui, Icon::StarFill, 12.0, AMBER).on_hover_text(t!("In der Statusleiste", "In the status bar"));
                                    }
                                    let ph = s.placeholders();
                                    if !ph.is_empty() {
                                        ui.label(RichText::new(ph.iter().map(|p| format!("{{{{{p}}}}}")).collect::<Vec<_>>().join(" ")).size(11.0).color(AMBER));
                                    }
                                });
                                let first = s.cmd.lines().next().unwrap_or("");
                                let more = if s.cmd.trim_end().lines().count() > 1 { " …" } else { "" };
                                ui.add(egui::Label::new(RichText::new(format!("{first}{more}")).monospace().size(12.0).color(MUTED)).truncate());
                            });
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                ui.spacing_mut().item_spacing.x = 3.0;
                                if !s.is_default && crate::sidebar::icon_button(ui, Icon::Trash, "").on_hover_text(t!("Löschen", "Delete")).clicked() {
                                    delete = Some(i);
                                }
                                if crate::sidebar::icon_button(ui, Icon::Pencil, "").on_hover_text(t!("Bearbeiten (als Kopie bei Standard-Snippets)", "Edit (as a copy for built-in snippets)")).clicked() {
                                    let mut c = s.clone();
                                    if c.is_default {
                                        c.name = tf!("{} (eigen)", "{} (custom)", c.name);
                                        new_edit = Some((None, c));
                                    } else {
                                        new_edit = Some((Some(i), c));
                                    }
                                }
                                if crate::sidebar::icon_button(ui, Icon::StarFill, "").on_hover_text(t!("In der Statusleiste an/aus", "Toggle in the status bar")).clicked() {
                                    pin = Some(i);
                                }
                                if ui.small_button(t!("Einfügen", "Insert")).on_hover_text(t!("Ins Terminal schreiben, ohne Enter", "Write into the terminal without Enter")).clicked() {
                                    insert = Some(s.clone());
                                }
                                if ui.add(egui::Button::image_and_text(icons::image(ui.ctx(), Icon::Play, 14.0, Color32::WHITE), RichText::new(t!("Ausführen", "Run")).color(Color32::WHITE)).fill(ACCENT)).clicked() {
                                    run = Some(s.clone());
                                }
                            });
                        });
                    });
                    ui.add_space(3.0);
                }
                ui.add_space(6.0);
            }
        });
        ui.add_space(8.0);
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if ui.add(egui::Button::new(RichText::new(t!("Schließen", "Close")).strong()).min_size(vec2(120.0, 32.0))).clicked() {
                keep = false;
            }
        });
        if ui.input(|i| i.key_pressed(Key::Escape)) {
            keep = false;
        }
        if let Some(i) = delete {
            self.snippets.remove(i);
            self.save_settings();
        }
        if let Some(i) = pin {
            self.snippets[i].pinned = !self.snippets[i].pinned;
            if self.snippets[i].is_default {
                // Standard-Snippets werden nicht gespeichert -> als eigene Kopie übernehmen
                let mut c = self.snippets[i].clone();
                c.is_default = false;
                self.snippets[i] = c;
            }
            self.save_settings();
        }
        if new_edit.is_some() {
            *edit = new_edit;
        }
        if let Some(mut s) = insert {
            while s.cmd.ends_with('\n') {
                s.cmd.pop();
            }
            keep = false;
            self.dialog = None;
            self.run_snippet(s);
        } else if let Some(s) = run {
            keep = false;
            self.dialog = None;
            self.run_snippet(s);
        }
        keep
    }

    pub fn run_snippet_dialog(&mut self, ui: &mut Ui, snippet: &Snippet, values: &mut [(String, String)], enter: &mut bool) -> bool {
        let mut keep = true;
        ui.set_width(460.0);
        heading(ui, Icon::Bolt, &snippet.name);
        ui.label(RichText::new(snippet.cmd.trim_end()).monospace().size(12.0).color(MUTED));
        ui.add_space(8.0);
        let mut submit = false;
        for (i, (name, value)) in values.iter_mut().enumerate() {
            label(ui, name);
            let r = ui.add(TextEdit::singleline(value).desired_width(f32::INFINITY));
            if i == 0 && ui.memory(|m| m.focused().is_none()) {
                r.request_focus();
            }
            if (r.lost_focus() || r.has_focus()) && ui.input(|i| i.key_pressed(Key::Enter)) {
                submit = true;
            }
            ui.add_space(4.0);
        }
        ui.checkbox(enter, t!("Sofort ausführen (Enter)", "Run immediately (Enter)"));
        ui.add_space(10.0);
        ui.horizontal(|ui| {
            if ui.add(egui::Button::new(RichText::new(t!("Abbrechen", "Cancel")).strong()).min_size(vec2(130.0, 34.0))).clicked() || ui.input(|i| i.key_pressed(Key::Escape)) {
                keep = false;
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui.add(egui::Button::image_and_text(icons::image(ui.ctx(), Icon::Play, 14.0, Color32::WHITE), RichText::new(t!("Senden", "Send")).color(Color32::WHITE).strong()).fill(ACCENT).min_size(vec2(140.0, 34.0))).clicked() {
                    submit = true;
                }
            });
        });
        if submit {
            let mut all = self.auto_values();
            all.extend(values.iter().cloned());
            let mut cmd = snippet.fill(&all);
            while cmd.ends_with('\n') {
                cmd.pop();
            }
            if *enter {
                cmd.push('\n');
            }
            self.send_text(cmd);
            keep = false;
        }
        keep
    }

    // ------------------------------------------------------------------
    // Tunnel einer laufenden Verbindung
    // ------------------------------------------------------------------

    pub fn open_tunnels(&mut self, tab: u64) {
        self.dialog = Some(Dialog::Tunnels { tab, new: Forward::default(), save: true });
    }

    pub fn tunnels_dialog(&mut self, ui: &mut Ui, tab: u64, new: &mut Forward, save: &mut bool) -> bool {
        let mut keep = true;
        ui.set_width(600.0);
        heading(ui, Icon::Tunnel, t!("Tunnel (Port-Weiterleitung)", "Tunnels (port forwarding)"));
        let Some((sess, title, host)) = self.tabs.iter().find(|t| t.id == tab).map(|t| (t.ssh.clone(), t.title.clone(), t.host.clone())) else {
            return false;
        };
        ui.label(RichText::new(tf!("Verbindung: {title}", "Connection: {title}")).color(MUTED));
        ui.add_space(8.0);
        let Some(sess) = sess else {
            ui.label(RichText::new(t!("Nicht verbunden – Tunnel werden beim nächsten Verbinden geöffnet.", "Not connected – tunnels are opened on the next connect.")).color(AMBER));
            if ui.button(t!("Schließen", "Close")).clicked() || ui.input(|i| i.key_pressed(Key::Escape)) {
                keep = false;
            }
            return keep;
        };
        let list = sess.tunnels();
        let mut close = None;
        if list.is_empty() {
            ui.label(RichText::new(t!("Noch keine Tunnel offen.", "No tunnels open yet.")).color(MUTED));
        }
        for (i, tun) in list.iter().enumerate() {
            egui::Frame::new().fill(PANEL2).corner_radius(8).inner_margin(egui::Margin::symmetric(10, 6)).show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    let (dot, col) = if tun.error.is_some() { ("●", RED) } else { ("●", GREEN) };
                    ui.label(RichText::new(dot).color(col));
                    ui.vertical(|ui| {
                        ui.label(RichText::new(tun.fwd.label()).strong().color(Color32::WHITE));
                        let sub = match &tun.error {
                            Some(e) => e.clone(),
                            None => tf!("{} · {} Verbindungen", "{} · {} connections", tun.fwd.short(), tun.conns),
                        };
                        ui.label(RichText::new(sub).size(11.5).color(if tun.error.is_some() { RED } else { MUTED }));
                    });
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if ui.small_button(t!("Schließen", "Close")).clicked() {
                            close = Some(i);
                        }
                    });
                });
            });
            ui.add_space(3.0);
        }
        if let Some(i) = close {
            let s = sess.clone();
            self.rt.spawn(async move { s.close_tunnel(i).await });
        }
        ui.add_space(10.0);
        ui.separator();
        ui.add_space(6.0);
        ui.label(RichText::new(t!("Neuen Tunnel öffnen", "Open a new tunnel")).strong());
        ui.add_space(4.0);
        if forward_editor(ui, new) {
            let f = new.clone();
            let label = f.label();
            if *save {
                if let Some(h) = self.hosts.iter_mut().find(|h| h.same_device(&host)) {
                    if !h.forwards.contains(&f) {
                        h.forwards.push(f.clone());
                        let _ = data::save_hosts(&self.hosts);
                    }
                }
            }
            let tx = self.tx.clone();
            let ctx = self.ctx.clone();
            let s = sess.clone();
            self.rt.spawn(async move {
                let res = s.open_tunnel(f).await;
                let _ = tx.send(Msg::TunnelOpened(tab, label, res));
                ctx.request_repaint();
            });
            *new = Forward { kind: new.kind.clone(), ..Default::default() };
        }
        let saved = self.hosts.iter().any(|h| h.same_device(&host));
        ui.add_enabled(saved, egui::Checkbox::new(save, t!("Auch beim Gerät speichern (beim nächsten Verbinden automatisch öffnen)", "Also save with the device (open automatically on next connect)")));
        ui.add_space(10.0);
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if ui.add(egui::Button::new(RichText::new(t!("Fertig", "Done")).strong()).min_size(vec2(120.0, 32.0))).clicked() {
                keep = false;
            }
        });
        if ui.input(|i| i.key_pressed(Key::Escape)) {
            keep = false;
        }
        ui.ctx().request_repaint_after(std::time::Duration::from_millis(500));
        keep
    }
}

/// Eingabe für einen Tunnel. Liefert true, wenn „Öffnen/Hinzufügen“ geklickt wurde.
pub fn forward_editor(ui: &mut Ui, f: &mut Forward) -> bool {
    let kinds = ["local", "remote", "dynamic"];
    let cur = kinds.iter().position(|k| *k == f.kind).unwrap_or(0);
    if let Some(i) = segmented(ui, &[t!("Lokal (-L)", "Local (-L)"), t!("Remote (-R)", "Remote (-R)"), t!("SOCKS (-D)", "SOCKS (-D)")], cur) {
        f.kind = kinds[i].into();
    }
    ui.add_space(4.0);
    let help = match f.kind.as_str() {
        "remote" => t!(
            "Ein Port auf dem Server wird zu einem Ziel erreichbar, das dieser PC sieht (z.B. lokaler Webserver für den Server freigeben).",
            "A port on the server forwards to a target that this PC can reach (e.g. share a local web server with the server)."
        ),
        "dynamic" => t!(
            "SOCKS-Proxy auf diesem PC: Browser oder Programme surfen über den Server (Proxy 127.0.0.1:Port eintragen).",
            "SOCKS proxy on this PC: browsers or programs go through the server (set proxy 127.0.0.1:port)."
        ),
        _ => t!(
            "Ein Port auf diesem PC führt durch den Tunnel zu einem Ziel, das der Server erreicht (z.B. Weboberfläche oder Datenbank hinter dem Server).",
            "A port on this PC leads through the tunnel to a target the server can reach (e.g. a web UI or database behind the server)."
        ),
    };
    ui.label(RichText::new(help).size(11.5).color(MUTED));
    ui.add_space(4.0);
    let mut go = false;
    ui.horizontal(|ui| {
        let bind_hint = if f.kind == "remote" { t!("Port auf dem Server, z.B. 9000", "port on the server, e.g. 9000") } else { t!("Port hier, z.B. 8080", "local port, e.g. 8080") };
        let w = if f.kind == "dynamic" { ui.available_width() - 120.0 } else { 170.0 };
        ui.add(TextEdit::singleline(&mut f.bind).hint_text(bind_hint).desired_width(w));
        if f.kind != "dynamic" {
            ui.label("→");
            let target_hint = if f.kind == "remote" { t!("Ziel von hier, z.B. localhost:3000", "target from here, e.g. localhost:3000") } else { t!("Ziel vom Server, z.B. 10.0.0.5:443", "target from the server, e.g. 10.0.0.5:443") };
            ui.add(TextEdit::singleline(&mut f.target).hint_text(target_hint).desired_width(ui.available_width() - 120.0));
        }
        let valid = nebula_core::ssh::parse_bind(&f.bind, "127.0.0.1").is_ok() && (f.kind == "dynamic" || nebula_core::ssh::parse_target(&f.target).is_ok());
        if ui.add_enabled(valid, egui::Button::new(RichText::new(t!("+ Hinzufügen", "+ Add")).color(Color32::WHITE)).fill(ACCENT).min_size(vec2(110.0, 28.0))).clicked() {
            go = true;
        }
    });
    go
}
