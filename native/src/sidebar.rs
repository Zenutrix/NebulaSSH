//! Seitenleiste (Schnellverbindung, gespeicherte Geräte), Tab-Leiste und Statusleiste.

use crate::app::*;
use crate::data::{self, Host};
use eframe::egui::{self, vec2, Align, Color32, CornerRadius, Layout, Margin, RichText, Stroke, TextEdit, Ui};
use rfd::MessageLevel;

pub fn toggle(ui: &mut Ui, label: &str, on: bool, width: f32) -> bool {
    let text = RichText::new(label).strong().color(if on { Color32::WHITE } else { MUTED });
    let b = egui::Button::new(text)
        .fill(if on { ACCENT } else { FIELD })
        .stroke(Stroke::NONE)
        .corner_radius(CornerRadius::same(5))
        .min_size(vec2(width, 30.0));
    ui.add(b).clicked()
}

pub fn label(ui: &mut Ui, text: &str) {
    ui.label(RichText::new(text).size(12.5).color(MUTED));
}

pub fn field(ui: &mut Ui, title: &str, value: &mut String, hint: &str) -> egui::Response {
    label(ui, title);
    let r = ui.add(TextEdit::singleline(value).hint_text(hint).desired_width(f32::INFINITY));
    ui.add_space(6.0);
    r
}

pub fn section(ui: &mut Ui, text: &str) {
    ui.label(RichText::new(text).size(12.0).color(MUTED).strong());
    ui.add_space(4.0);
}

pub fn primary(ui: &mut Ui, text: &str, width: f32) -> egui::Response {
    ui.add(
        egui::Button::new(RichText::new(text).strong().color(Color32::WHITE))
            .fill(ACCENT)
            .min_size(vec2(width, 34.0)),
    )
}

/// Symbolknopf mit fester Größe (32×28), damit Zeilen nicht überlaufen.
/// Dezenter Symbolknopf ohne Hintergrund (Hintergrund erst beim Überfahren).
pub fn ghost_button(ui: &mut Ui, icon: &str, tip: &str) -> egui::Response {
    ui.add_sized(
        vec2(30.0, 30.0),
        egui::Button::new(RichText::new(icon).size(15.0).color(Color32::from_gray(200))).frame_when_inactive(false),
    )
    .on_hover_text(tip)
}

/// Karte mit abgerundeten Ecken für Bereiche in der Seitenleiste.
pub fn card<R>(ui: &mut Ui, add: impl FnOnce(&mut Ui) -> R) -> R {
    egui::Frame::new()
        .fill(Color32::from_rgb(0x1b, 0x1b, 0x29))
        .stroke(Stroke::new(1.0, Color32::from_rgb(0x29, 0x29, 0x3b)))
        .corner_radius(10)
        .inner_margin(Margin::same(12))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            add(ui)
        })
        .inner
}

/// Quadratische Kachel mit Symbol (Gerätetyp), optional mit Online-Punkt.
pub fn tile(ui: &mut Ui, serial: bool, size: f32, online: bool) {
    let (rect, _) = ui.allocate_exact_size(vec2(size, size), egui::Sense::hover());
    let (bg, fg, glyph, font) = if serial {
        (Color32::from_rgb(0x3a, 0x2a, 0x12), AMBER, "🔌", egui::FontId::proportional(size * 0.5))
    } else {
        (Color32::from_rgb(0x2a, 0x1b, 0x52), LAVENDER, ">_", egui::FontId::monospace(size * 0.42))
    };
    let p = ui.painter();
    p.rect_filled(rect, size * 0.26, bg);
    p.text(rect.center(), egui::Align2::CENTER_CENTER, glyph, font, fg);
    if online {
        let c = rect.right_bottom() - vec2(3.0, 3.0);
        p.circle_filled(c, 5.0, PANEL2);
        p.circle_filled(c, 3.5, GREEN);
    }
}

pub fn icon_button(ui: &mut Ui, icon: &str, tip: &str) -> egui::Response {
    ui.add_sized(vec2(32.0, 28.0), egui::Button::new(RichText::new(icon).size(15.0)))
        .on_hover_text(tip)
}

/// Breite für ein dehnbares Feld, wenn danach noch `buttons` Symbolknöpfe folgen.
pub fn fill_width(ui: &Ui, buttons: usize) -> f32 {
    (ui.available_width() - buttons as f32 * (32.0 + ui.spacing().item_spacing.x) - 4.0).max(60.0)
}

enum DeviceAction {
    Connect(usize),
    Edit(usize),
    Duplicate(usize),
    Favorite(usize),
    Delete(usize),
    Load(usize),
}

impl App {
    pub fn sidebar(&mut self, ui: &mut Ui) {
        ui.add_space(14.0);
        // Kopfzeile mit Logo und Schnellzugriffen
        ui.horizontal(|ui| {
            ui.add(egui::Image::new(&self.logo).fit_to_exact_size(vec2(32.0, 32.0)));
            ui.vertical(|ui| {
                ui.spacing_mut().item_spacing.y = 0.0;
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 0.0;
                    ui.label(RichText::new("Nebula").size(18.0).family(egui::FontFamily::Name("ui-bold".into())).color(Color32::WHITE));
                    ui.label(RichText::new("SSH").size(18.0).family(egui::FontFamily::Name("ui-bold".into())).color(LAVENDER));
                });
                ui.label(RichText::new(format!("Version {VERSION}")).size(10.5).color(MUTED));
            });
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 2.0;
                if ghost_button(ui, "⚙", "Einstellungen").clicked() {
                    self.open_settings(SettingsPage::Terminal);
                }
                if ghost_button(ui, "🔑", "SSH-Keys verwalten / erzeugen").clicked() {
                    self.dialog = Some(Dialog::Keys { selected: None });
                }
                if self.prefs.lock_hash.is_some() && ghost_button(ui, "🔒", "Jetzt sperren").clicked() {
                    self.lock_now();
                }
            });
        });
        ui.add_space(14.0);

        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            let open_id = egui::Id::new("quick_open");
            let mut open = ui.ctx().data_mut(|d| *d.get_temp_mut_or(open_id, true));
            card(ui, |ui| {
                let rect = ui
                    .horizontal(|ui| {
                        ui.label(RichText::new("⚡").color(AMBER));
                        ui.label(RichText::new("Neue Verbindung").strong().color(Color32::WHITE));
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            ui.label(RichText::new(if open { "▾" } else { "▸" }).color(MUTED));
                        });
                    })
                    .response
                    .rect;
                let head = ui.interact(rect, egui::Id::new("quick_head"), egui::Sense::click());
                if head.hovered() {
                    ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                }
                if head.clicked() {
                    open = !open;
                }
                if open {
                    ui.add_space(10.0);
                    self.quick_connect(ui);
                }
            });
            ui.ctx().data_mut(|d| d.insert_temp(open_id, open));
            ui.add_space(16.0);
            self.device_list(ui);
        });
    }

    fn quick_connect(&mut self, ui: &mut Ui) {
        let serial = self.form.is_serial();
        ui.horizontal(|ui| {
            let w = (ui.available_width() - 6.0) / 2.0;
            if toggle(ui, "SSH", !serial, w) {
                self.form.kind = "ssh".into();
            }
            if toggle(ui, "Serial", serial, w) && !serial {
                self.form.kind = "serial".into();
                self.refresh_ports();
            }
        });
        ui.add_space(8.0);

        let mut connect_now = false;
        if !self.form.is_serial() {
            field(ui, "Gerätename (optional)", &mut self.form.name, "z.B. Core-Switch");
            label(ui, "Host / IP *");
            let r = ui.add(
                TextEdit::singleline(&mut self.form.ip)
                    .id(egui::Id::new("quick_ip"))
                    .hint_text("192.168.1.50 oder host:2222")
                    .desired_width(f32::INFINITY),
            );
            if (r.lost_focus() || r.has_focus()) && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                connect_now = true;
            }
            ui.add_space(6.0);
            field(ui, "Benutzername *", &mut self.form.user, "root");

            label(ui, "Anmeldung");
            ui.horizontal(|ui| {
                let w = (ui.available_width() - 6.0) / 2.0;
                if toggle(ui, "Passwort", self.form.auth_method != "key", w) {
                    self.form.auth_method = "password".into();
                }
                if toggle(ui, "SSH-Key", self.form.auth_method == "key", w) {
                    self.form.auth_method = "key".into();
                    if self.form.key_name.is_empty() {
                        self.form.key_name = self.keys.first().map(|k| k.name.clone()).unwrap_or_default();
                    }
                }
            });
            ui.add_space(6.0);
            if self.form.auth_method != "key" {
                ui.horizontal(|ui| {
                    let r = ui.add(
                        TextEdit::singleline(&mut self.form.pass)
                            .password(true)
                            .hint_text("Passwort")
                            .desired_width(fill_width(ui, 1)),
                    );
                    if (r.lost_focus() || r.has_focus()) && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                        connect_now = true;
                    }
                    if ui
                        .add_enabled_ui(!self.form.pass.is_empty(), |ui| ui.add_sized(vec2(32.0, 28.0), egui::Button::new("📋")))
                        .inner
                        .on_hover_text(format!(
                            "Passwort kopieren (wird nach {} s aus der Zwischenablage gelöscht)",
                            self.prefs.clipboard_clear
                        ))
                        .clicked()
                    {
                        crate::clipboard::copy_secret(self.form.pass.clone(), self.prefs.clipboard_clear);
                        self.notify("Passwort kopiert");
                    }
                });
            } else {
                ui.horizontal(|ui| {
                    let label = if self.keys.is_empty() { "Keine Keys gespeichert".to_string() } else { self.form.key_name.clone() };
                    egui::ComboBox::from_id_salt("key")
                        .selected_text(label)
                        .width(fill_width(ui, 1) - 8.0)
                        .show_ui(ui, |ui| {
                            for k in &self.keys {
                                ui.selectable_value(&mut self.form.key_name, k.name.clone(), &k.name);
                            }
                        });
                    if icon_button(ui, "🔑", "SSH-Keys verwalten / erzeugen").clicked() {
                        self.dialog = Some(Dialog::Keys { selected: None });
                    }
                });
            }
            ui.add_space(8.0);
        } else {
            field(ui, "Gerätename (optional)", &mut self.form.name, "z.B. Konsole Rack 3");
            label(ui, "COM-Port *");
            ui.horizontal(|ui| {
                let label = if self.ports.is_empty() { "Keine Ports gefunden".to_string() } else { self.form.port.clone() };
                egui::ComboBox::from_id_salt("port")
                    .selected_text(label)
                    .width(fill_width(ui, 1) - 8.0)
                    .show_ui(ui, |ui| {
                        for p in &self.ports {
                            ui.selectable_value(&mut self.form.port, p.clone(), p);
                        }
                    });
                if icon_button(ui, "⟳", "Ports aktualisieren").clicked() {
                    self.refresh_ports();
                }
            });
            ui.add_space(6.0);
            label(ui, "Baudrate");
            ui.horizontal(|ui| {
                let custom = !data::default_bauds().iter().any(|d| d.label == self.baud_label);
                let w = fill_width(ui, if custom { 2 } else { 1 }) - 8.0;
                let before = self.baud_label.clone();
                egui::ComboBox::from_id_salt("baud")
                    .selected_text(self.baud_label.clone())
                    .width(w)
                    .show_ui(ui, |ui| {
                        for b in &self.bauds {
                            ui.selectable_value(&mut self.baud_label, b.label.clone(), &b.label);
                        }
                    });
                if before != self.baud_label {
                    if let Some(b) = self.bauds.iter().find(|b| b.label == self.baud_label) {
                        self.form.baud = b.rate;
                    }
                }
                if icon_button(ui, "+", "Eigenes Baudrate-Profil").clicked() {
                    self.dialog = Some(Dialog::AddBaud { name: String::new(), rate: String::new() });
                }
                if custom && icon_button(ui, "🗑", "Eigenes Profil löschen").clicked() {
                    let label = self.baud_label.clone();
                    self.bauds.retain(|b| b.label != label);
                    self.baud_label = "115200".into();
                    self.form.baud = 115200;
                    self.save_settings();
                }
            });
            ui.add_space(10.0);
        }

        ui.horizontal(|ui| {
            let w = (ui.available_width() - 6.0) / 2.0;
            let can = !self.form.is_serial() || !self.form.port.is_empty();
            if ui
                .add_enabled(can, egui::Button::new(RichText::new("Verbinden").strong().color(Color32::WHITE)).fill(ACCENT).min_size(vec2(w, 34.0)))
                .clicked()
            {
                connect_now = true;
            }
            if ui.add(egui::Button::new(RichText::new("Speichern").strong()).min_size(vec2(w, 34.0))).on_hover_text("Als Gerät speichern").clicked() {
                self.save_form_device();
            }
        });
        if connect_now {
            let h = self.form.clone();
            self.open(h, None);
        }
    }

    /// Seitenleiste öffnen, Schnellverbindung aufklappen und das Host-Feld fokussieren.
    pub fn focus_quick_connect(&mut self) {
        self.prefs.sidebar_open = true;
        self.ctx.data_mut(|d| d.insert_temp(egui::Id::new("quick_open"), true));
        self.form.kind = "ssh".into();
        self.ctx.memory_mut(|m| m.request_focus(egui::Id::new("quick_ip")));
    }

    pub fn refresh_ports(&mut self) {
        self.ports = nebula_core::serial::list_ports();
        if !self.ports.contains(&self.form.port) {
            self.form.port = self.ports.first().cloned().unwrap_or_default();
        }
    }

    fn save_form_device(&mut self) {
        let mut h = self.form.clone();
        if h.is_serial() {
            if h.port.is_empty() {
                alert("NebulaSSH", "Bitte einen COM-Port wählen.");
                return;
            }
        } else if h.ip.trim().is_empty() || h.user.trim().is_empty() {
            alert("NebulaSSH", "Host und Benutzer werden benötigt.");
            return;
        }
        if h.name.is_empty() {
            h.name = if h.is_serial() { h.port.clone() } else { h.ip.clone() };
        }
        match self.hosts.iter().position(|x| x.same_device(&h) || (x.name == h.name && x.kind == h.kind)) {
            Some(i) => {
                // Gruppe/Favorit/Statistik behalten
                let old = &self.hosts[i];
                h.group = old.group.clone();
                h.favorite = old.favorite;
                h.notes = old.notes.clone();
                h.last_used = old.last_used;
                h.use_count = old.use_count;
                self.hosts[i] = h;
            }
            None => self.hosts.push(h),
        }
        match data::save_hosts(&self.hosts) {
            Ok(()) => self.notify("Gerät gespeichert"),
            Err(e) => alert("Speichern", &e),
        }
    }

    fn device_list(&mut self, ui: &mut Ui) {
        ui.horizontal(|ui| {
            ui.label(RichText::new("Geräte").strong().color(Color32::WHITE));
            ui.label(RichText::new(self.hosts.len().to_string()).size(11.5).color(MUTED));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ghost_button(ui, "+", "Neues Gerät anlegen").clicked() {
                    self.dialog = Some(Dialog::EditHost {
                        index: None,
                        host: Host { user: self.prefs.default_user.clone(), ..Default::default() },
                        new_group: String::new(),
                    });
                }
            });
        });
        if self.hosts.len() > 3 {
            ui.add(TextEdit::singleline(&mut self.device_search).hint_text("🔍  Suchen…").desired_width(f32::INFINITY));
            ui.add_space(8.0);
        }
        if self.hosts.is_empty() {
            ui.add_space(4.0);
            ui.label(RichText::new("Noch keine Geräte gespeichert.\nVerbindung eingeben und „Speichern“ klicken.").italics().color(Color32::from_gray(115)));
            return;
        }

        let q = self.device_search.to_lowercase();
        let matches = |h: &Host| {
            q.is_empty()
                || h.name.to_lowercase().contains(&q)
                || h.ip.to_lowercase().contains(&q)
                || h.user.to_lowercase().contains(&q)
                || h.group.to_lowercase().contains(&q)
                || h.port.to_lowercase().contains(&q)
                || h.notes.to_lowercase().contains(&q)
        };
        let mut groups: Vec<String> = self.hosts.iter().map(|h| h.group.clone()).collect();
        groups.sort_by_key(|g| (g.is_empty(), g.to_lowercase()));
        groups.dedup();

        let mut action = None;
        let favs: Vec<usize> = (0..self.hosts.len()).filter(|&i| self.hosts[i].favorite && matches(&self.hosts[i])).collect();
        if !favs.is_empty() {
            group_header(ui, "★  Favoriten", favs.len(), AMBER);
            for i in favs {
                if let Some(a) = self.device_row(ui, i) {
                    action = Some(a);
                }
            }
            ui.add_space(6.0);
        }
        let single = groups.len() == 1 && groups[0].is_empty();
        for g in groups {
            let idx: Vec<usize> = (0..self.hosts.len())
                .filter(|&i| self.hosts[i].group == g && !self.hosts[i].favorite && matches(&self.hosts[i]))
                .collect();
            if idx.is_empty() {
                continue;
            }
            let mut body = |ui: &mut Ui| {
                for i in &idx {
                    if let Some(a) = self.device_row(ui, *i) {
                        action = Some(a);
                    }
                }
            };
            if single {
                body(ui);
            } else {
                let title = if g.is_empty() { "Ohne Gruppe".to_string() } else { g.clone() };
                let id = egui::Id::new(("grp", &g));
                let mut open = ui.ctx().data_mut(|d| *d.get_temp_mut_or(id, true));
                let arrow = if open { "▾" } else { "▸" };
                if group_header(ui, &format!("{arrow}  {title}"), idx.len(), Color32::from_gray(170)).clicked() {
                    open = !open;
                    ui.ctx().data_mut(|d| d.insert_temp(id, open));
                }
                if open {
                    body(ui);
                }
                ui.add_space(6.0);
            }
        }

        if let Some(a) = action {
            match a {
                DeviceAction::Connect(i) => {
                    let h = self.hosts[i].clone();
                    self.open(h, None);
                }
                DeviceAction::Load(i) => {
                    self.form = self.hosts[i].clone();
                    if let Some(b) = self.bauds.iter().find(|b| b.rate == self.form.baud) {
                        self.baud_label = b.label.clone();
                    }
                }
                DeviceAction::Edit(i) => {
                    self.dialog = Some(Dialog::EditHost { index: Some(i), host: self.hosts[i].clone(), new_group: String::new() });
                }
                DeviceAction::Duplicate(i) => {
                    let mut h = self.hosts[i].clone();
                    h.name = format!("{} (Kopie)", h.display_name());
                    h.last_used = 0;
                    h.use_count = 0;
                    self.hosts.insert(i + 1, h);
                    let _ = data::save_hosts(&self.hosts);
                }
                DeviceAction::Favorite(i) => {
                    self.hosts[i].favorite = !self.hosts[i].favorite;
                    let _ = data::save_hosts(&self.hosts);
                }
                DeviceAction::Delete(i) => {
                    let name = self.hosts[i].display_name();
                    if confirm("Gerät löschen", &format!("„{name}“ aus der Liste löschen?"), MessageLevel::Warning) {
                        self.hosts.remove(i);
                        let _ = data::save_hosts(&self.hosts);
                    }
                }
            }
        }
    }

    fn device_row(&self, ui: &mut Ui, i: usize) -> Option<DeviceAction> {
        let h = &self.hosts[i];
        let mut act = None;
        let online = self.tabs.iter().any(|t| t.status == Status::Connected && t.host.same_device(h));
        let frame = egui::Frame::new().fill(PANEL2).corner_radius(9).inner_margin(Margin { left: 8, right: 6, top: 7, bottom: 7 });
        let resp = frame
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    tile(ui, h.is_serial(), 34.0, online);
                    ui.add_space(4.0);
                    ui.vertical(|ui| {
                        ui.spacing_mut().item_spacing.y = 1.0;
                        ui.add(egui::Label::new(RichText::new(h.display_name()).strong().color(Color32::WHITE)).truncate());
                        let mut sub = h.subtitle();
                        if h.auth_method == "key" && !h.is_serial() {
                            sub.push_str("  🔑");
                        }
                        ui.add(egui::Label::new(RichText::new(sub).size(11.5).color(MUTED)).truncate());
                    });
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        let star = if h.favorite { RichText::new("★").color(AMBER) } else { RichText::new("☆").color(Color32::from_gray(90)) };
                        if ui.add(egui::Button::new(star.size(15.0)).frame(false)).on_hover_text("Favorit").clicked() {
                            act = Some(DeviceAction::Favorite(i));
                        }
                    });
                });
            })
            .response
            .interact(egui::Sense::click());
        if resp.hovered() {
            ui.painter().rect_stroke(resp.rect, 9.0, Stroke::new(1.0, ACCENT_HI), egui::StrokeKind::Inside);
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }
        let resp = resp.on_hover_ui(|ui| {
            ui.label(RichText::new(h.display_name()).strong());
            ui.label(h.subtitle());
            if !h.group.is_empty() {
                ui.label(format!("Gruppe: {}", h.group));
            }
            ui.label(format!("Zuletzt: {} · {} Verbindungen", data::ago(h.last_used), h.use_count));
            if !h.notes.is_empty() {
                ui.separator();
                ui.label(&h.notes);
            }
            ui.separator();
            ui.label(RichText::new("Klick = verbinden · Rechtsklick = mehr").size(11.0).color(MUTED));
        });
        if resp.clicked() && act.is_none() {
            act = Some(DeviceAction::Connect(i));
        }
        resp.context_menu(|ui| {
            if ui.button("▶  Verbinden").clicked() {
                act = Some(DeviceAction::Connect(i));
            }
            if ui.button("✏  Bearbeiten…").clicked() {
                act = Some(DeviceAction::Edit(i));
            }
            if ui.button("⤵  In Formular laden").clicked() {
                act = Some(DeviceAction::Load(i));
            }
            if ui.button("⧉  Duplizieren").clicked() {
                act = Some(DeviceAction::Duplicate(i));
            }
            if ui.button(if h.favorite { "☆  Kein Favorit mehr" } else { "★  Als Favorit" }).clicked() {
                act = Some(DeviceAction::Favorite(i));
            }
            ui.separator();
            if ui.button(RichText::new("🗑  Löschen").color(RED)).clicked() {
                act = Some(DeviceAction::Delete(i));
            }
        });
        ui.add_space(4.0);
        act
    }

    pub fn tab_bar(&mut self, ui: &mut Ui) {
        enum A {
            Switch(u64),
            Close(u64),
            Reconnect(u64),
            Duplicate(u64),
            CloseOthers(u64),
            SaveOutput(u64),
            Clear(u64),
        }
        let mut act = None;
        let mut new_tab = false;
        ui.horizontal_centered(|ui| {
            ui.spacing_mut().item_spacing.x = 0.0;
            // Seitenleiste ein/aus
            let side = if self.prefs.sidebar_open { "◀" } else { "▶" };
            if ui
                .add(egui::Button::new(RichText::new(side).color(MUTED)).frame(false).min_size(vec2(30.0, 40.0)))
                .on_hover_text("Seitenleiste ein/aus (Strg+Umschalt+B)")
                .clicked()
            {
                self.prefs.sidebar_open = !self.prefs.sidebar_open;
                self.save_settings();
            }
            egui::ScrollArea::horizontal()
                .max_width(ui.available_width() - 230.0)
                .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden)
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 0.0;
                        for t in &self.tabs {
                            let active = self.active == Some(t.id);
                            let dot = match t.status {
                                Status::Connecting => AMBER,
                                Status::Connected => GREEN,
                                Status::Closed => RED,
                            };
                            let accent = if t.host.is_serial() { AMBER } else { ACCENT_HI };
                            let fill = if active { BG } else { Color32::TRANSPARENT };
                            let resp = egui::Frame::new()
                                .fill(fill)
                                .corner_radius(CornerRadius { nw: 9, ne: 9, sw: 0, se: 0 })
                                .outer_margin(Margin { left: 0, right: 3, top: 6, bottom: 0 })
                                .inner_margin(Margin { left: 10, right: 6, top: 0, bottom: 0 })
                                .show(ui, |ui| {
                                    ui.set_height(35.0);
                                    ui.set_max_width(230.0);
                                    ui.horizontal_centered(|ui| {
                                        ui.spacing_mut().item_spacing.x = 6.0;
                                        let (glyph, gfont, gcol) = if t.host.is_serial() {
                                            ("🔌", egui::FontId::proportional(12.0), AMBER)
                                        } else {
                                            (">_", egui::FontId::monospace(11.5), LAVENDER)
                                        };
                                        let (r, _) = ui.allocate_exact_size(vec2(22.0, 18.0), egui::Sense::hover());
                                        ui.painter().rect_filled(r, 5.0, if active { Color32::from_rgb(0x24, 0x18, 0x45) } else { PANEL2 });
                                        ui.painter().text(r.center(), egui::Align2::CENTER_CENTER, glyph, gfont, gcol);
                                        let c = r.right_top() + vec2(-1.0, 1.0);
                                        ui.painter().circle_filled(c, 4.0, if active { BG } else { PANEL });
                                        ui.painter().circle_filled(c, 2.8, dot);
                                        let txt = RichText::new(&t.title)
                                            .size(13.0)
                                            .color(if active { Color32::WHITE } else { Color32::from_gray(185) });
                                        let l = ui.add(egui::Label::new(txt).sense(egui::Sense::click()).truncate());
                                        if l.clicked() {
                                            act = Some(A::Switch(t.id));
                                        }
                                        if ui
                                            .add(egui::Button::new(RichText::new("×").size(16.0).color(MUTED)).frame(false))
                                            .on_hover_text("Tab schließen (Strg+Umschalt+W)")
                                            .clicked()
                                        {
                                            act = Some(A::Close(t.id));
                                        }
                                    });
                                })
                                .response
                                .interact(egui::Sense::click());
                            let r = resp.rect;
                            if active {
                                ui.painter().line_segment(
                                    [r.left_top() + vec2(8.0, 1.0), r.right_top() + vec2(-8.0, 1.0)],
                                    Stroke::new(2.0, accent),
                                );
                            } else if resp.hovered() {
                                ui.painter().rect_stroke(r, CornerRadius { nw: 9, ne: 9, sw: 0, se: 0 }, Stroke::new(1.0, LINE), egui::StrokeKind::Inside);
                            }
                            if resp.clicked() {
                                act = Some(A::Switch(t.id));
                            }
                            if resp.middle_clicked() {
                                act = Some(A::Close(t.id));
                            }
                            let id = t.id;
                            resp.on_hover_text(format!("{}\n{}", t.title, t.host.subtitle())).context_menu(|ui| {
                                if ui.button("⟳  Neu verbinden").clicked() {
                                    act = Some(A::Reconnect(id));
                                }
                                if ui.button("⧉  Duplizieren").clicked() {
                                    act = Some(A::Duplicate(id));
                                }
                                if ui.button("💾  Ausgabe speichern…").clicked() {
                                    act = Some(A::SaveOutput(id));
                                }
                                if ui.button("🧹  Terminal leeren").clicked() {
                                    act = Some(A::Clear(id));
                                }
                                ui.separator();
                                if ui.button("Andere Tabs schließen").clicked() {
                                    act = Some(A::CloseOthers(id));
                                }
                                if ui.button(RichText::new("Schließen").color(RED)).clicked() {
                                    act = Some(A::Close(id));
                                }
                            });
                        }
                        ui.add_space(4.0);
                        if ghost_button(ui, "+", "Neue Verbindung (Strg+Umschalt+T)").clicked() {
                            new_tab = true;
                        }
                    });
                });

            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                ui.add_space(8.0);
                let is_ssh = self.active.is_some_and(|a| self.tabs.iter().any(|t| t.id == a && t.is_ssh()));
                if is_ssh {
                    let b = egui::Button::new(RichText::new("📂 Dateien").strong().color(Color32::WHITE))
                        .fill(if self.show_files { ACCENT } else { Color32::from_rgb(0x2e, 0x2e, 0x45) });
                    if ui.add(b).on_hover_text("SFTP-Datei-Explorer (Strg+Umschalt+E)").clicked() {
                        self.toggle_files();
                    }
                }
                if self.tabs.len() > 1 {
                    let b = egui::Button::new(RichText::new("📡 Alle").color(if self.broadcast { Color32::BLACK } else { Color32::WHITE }))
                        .fill(if self.broadcast { AMBER } else { Color32::from_rgb(0x2e, 0x2e, 0x45) });
                    if ui.add(b).on_hover_text("Broadcast: Eingaben gleichzeitig an alle verbundenen Tabs senden").clicked() {
                        self.broadcast = !self.broadcast;
                    }
                }
            });
        });

        if new_tab {
            self.focus_quick_connect();
        }
        match act {
            Some(A::Switch(id)) => self.switch_tab(id),
            Some(A::Close(id)) => self.close_tab(id, true),
            Some(A::Reconnect(id)) => self.reconnect(id),
            Some(A::Duplicate(id)) => self.duplicate(id),
            Some(A::Clear(id)) => {
                if let Some(t) = self.tab(id) {
                    t.term.clear();
                }
            }
            Some(A::SaveOutput(id)) => {
                if let Some(t) = self.tabs.iter().find(|t| t.id == id) {
                    let name = format!("{}_{}.txt", t.title.replace(['/', '\\', ':', '@', ' '], "_"), chrono::Local::now().format("%Y-%m-%d_%H-%M"));
                    if let Some(p) = rfd::FileDialog::new().set_title("Terminal-Ausgabe speichern").set_file_name(&name).save_file() {
                        match std::fs::write(&p, t.term.all_text()) {
                            Ok(()) => self.notify("Ausgabe gespeichert"),
                            Err(e) => alert("Speichern", &e.to_string()),
                        }
                    }
                }
            }
            Some(A::CloseOthers(id)) => {
                let others: Vec<u64> = self.tabs.iter().filter(|t| t.id != id).map(|t| t.id).collect();
                for o in others {
                    self.close_tab(o, false);
                }
                self.switch_tab(id);
            }
            None => {}
        }
    }

    pub fn status_bar(&mut self, ui: &mut Ui) {
        let mut run = None;
        let mut delete = None;
        ui.horizontal_centered(|ui| {
            ui.label(RichText::new("⚡").color(AMBER));
            let w = (ui.available_width() - 330.0).max(120.0);
            egui::ScrollArea::horizontal()
                .max_width(w)
                .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden)
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        for (i, s) in self.snippets.iter().enumerate() {
                            let r = ui
                                .add(egui::Button::new(RichText::new(&s.name).size(13.0)).fill(PANEL2).corner_radius(14).min_size(vec2(0.0, 26.0)))
                                .on_hover_text(format!("{}\n(Rechtsklick: löschen)", s.cmd.trim_end()));
                            if r.clicked() {
                                run = Some(s.cmd.clone());
                            }
                            if !s.is_default {
                                r.context_menu(|ui| {
                                    if ui.button(RichText::new("Makro löschen").color(RED)).clicked() {
                                        delete = Some(i);
                                    }
                                });
                            }
                        }
                        if ui.button(RichText::new("+ Makro").color(MUTED)).on_hover_text("Neues Makro").clicked() {
                            self.dialog = Some(Dialog::AddSnippet { name: String::new(), cmd: String::new() });
                        }
                    });
                });

            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                self.update_badge(ui);
                if let Some(t) = self.active.and_then(|id| self.tabs.iter().find(|t| t.id == id)) {
                    let (txt, col) = match t.status {
                        Status::Connected if t.is_ssh() => ("🔒 SSH verschlüsselt", GREEN),
                        Status::Connected => ("🔌 Serial verbunden", AMBER),
                        Status::Connecting => ("… verbinde", AMBER),
                        Status::Closed => ("✖ getrennt", RED),
                    };
                    ui.label(RichText::new(format!("{}×{}", t.term.size.cols, t.term.size.rows)).size(12.0).color(MUTED));
                    if t.log.is_some() {
                        ui.label(RichText::new("● REC").size(12.0).color(RED)).on_hover_text("Sitzung wird protokolliert");
                    }
                    if let Some(at) = t.connected_at.filter(|_| t.status == Status::Connected) {
                        let s = at.elapsed().as_secs();
                        ui.label(RichText::new(format!("{:02}:{:02}:{:02}", s / 3600, (s / 60) % 60, s % 60)).size(12.0).color(MUTED));
                        ui.ctx().request_repaint_after(std::time::Duration::from_secs(1));
                    }
                    egui::Frame::new()
                        .fill(col.gamma_multiply(0.14))
                        .corner_radius(12)
                        .inner_margin(Margin::symmetric(9, 3))
                        .show(ui, |ui| {
                            ui.label(RichText::new(txt).size(12.0).color(col));
                        });
                }
            });
        });
        if let Some(cmd) = run {
            self.run_snippet(cmd);
        }
        if let Some(i) = delete {
            self.snippets.remove(i);
            self.save_settings();
        }
    }
}

/// Zwischenüberschrift für Gruppen in der Geräteliste (klickbar).
fn group_header(ui: &mut Ui, title: &str, count: usize, color: Color32) -> egui::Response {
    let rect = ui
        .horizontal(|ui| {
            ui.label(RichText::new(title).size(12.5).strong().color(color));
            ui.label(RichText::new(count.to_string()).size(11.0).color(Color32::from_gray(100)));
        })
        .response
        .rect;
    let r = ui.interact(rect, egui::Id::new(("group_header", title)), egui::Sense::click());
    if r.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    ui.add_space(2.0);
    r
}
