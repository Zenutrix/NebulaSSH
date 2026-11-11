//! Seitenleiste (Schnellverbindung, gespeicherte Geräte), Tab-Leiste und Statusleiste.

use crate::app::*;
use crate::data::{self, Host};
use eframe::egui::{self, vec2, Align, Color32, CornerRadius, Layout, Margin, RichText, Stroke, TextEdit, Ui};
use nebula_core::{t, tf};
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

/// Mehrere Umschalter nebeneinander; liefert den angeklickten Index.
pub fn segmented(ui: &mut Ui, labels: &[&str], current: usize) -> Option<usize> {
    let mut res = None;
    ui.horizontal(|ui| {
        let n = labels.len() as f32;
        let w = (ui.available_width() - 6.0 * (n - 1.0)) / n;
        for (i, l) in labels.iter().enumerate() {
            if toggle(ui, l, i == current, w) {
                res = Some(i);
            }
        }
    });
    res
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
    ui.add(egui::Button::new(RichText::new(text).strong().color(Color32::WHITE)).fill(ACCENT).min_size(vec2(width, 34.0)))
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

/// Quadratische Kachel mit Symbol (Gerätetyp), optional mit Statuspunkt.
pub fn tile(ui: &mut Ui, h: &Host, size: f32, dot: Option<Color32>) {
    let (rect, _) = ui.allocate_exact_size(vec2(size, size), egui::Sense::hover());
    let (icon, fg) = kind_icon(h);
    let r = size * 0.28;
    ui.painter().rect_filled(rect, r, fg.gamma_multiply(0.13));
    ui.painter().rect_stroke(rect, r, Stroke::new(1.0, fg.gamma_multiply(0.22)), egui::StrokeKind::Inside);
    icons::paint(ui, egui::Rect::from_center_size(rect.center(), vec2(size * 0.52, size * 0.52)), icon, fg);
    if let Some(c) = dot {
        let c0 = rect.right_bottom() - vec2(2.0, 2.0);
        ui.painter().circle_filled(c0, 5.5, PANEL);
        ui.painter().circle_filled(c0, 3.8, c);
    }
}

/// Symbolknopf mit Rahmen (neben Eingabefeldern).
pub fn icon_button(ui: &mut Ui, icon: Icon, tip: &str) -> egui::Response {
    // Feste Größe (32×28) – ein egui-Button mit Bild wäre durch den Innenabstand breiter
    let sense = if ui.is_enabled() { egui::Sense::click() } else { egui::Sense::hover() };
    let (rect, resp) = ui.allocate_exact_size(vec2(32.0, 28.0), sense);
    let v = ui.style().interact(&resp);
    ui.painter().rect(rect, v.corner_radius, v.weak_bg_fill, v.bg_stroke, egui::StrokeKind::Inside);
    let col = if ui.is_enabled() { Color32::from_gray(225) } else { Color32::from_gray(110) };
    icons::paint(ui, egui::Rect::from_center_size(rect.center(), vec2(15.0, 15.0)), icon, col);
    if tip.is_empty() { resp } else { resp.on_hover_text(tip) }
}

/// Kleines Icon in einem farbig hinterlegten Quadrat.
pub fn badge_icon(ui: &mut Ui, icon: Icon, size: f32, color: Color32) {
    let (rect, _) = ui.allocate_exact_size(vec2(size, size), egui::Sense::hover());
    ui.painter().rect_filled(rect, size * 0.3, color.gamma_multiply(0.16));
    icons::paint(ui, egui::Rect::from_center_size(rect.center(), vec2(size * 0.58, size * 0.58)), icon, color);
}

/// Breite für ein dehnbares Feld, wenn danach noch `buttons` Symbolknöpfe folgen.
pub fn fill_width(ui: &Ui, buttons: usize) -> f32 {
    (ui.available_width() - buttons as f32 * (32.0 + ui.spacing().item_spacing.x) - 4.0).max(60.0)
}

/// Kleiner farbiger Chip (z.B. in der Statusleiste)
pub fn chip(ui: &mut Ui, icon: Option<Icon>, text: &str, color: Color32) -> egui::Response {
    egui::Frame::new()
        .fill(color.gamma_multiply(0.14))
        .corner_radius(12)
        .inner_margin(Margin::symmetric(9, 3))
        .show(ui, |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 5.0;
                // In rechtsbündigen Leisten läuft die Reihenfolge rückwärts – Icon soll trotzdem links stehen
                let rtl = ui.layout().prefer_right_to_left();
                if rtl {
                    ui.label(RichText::new(text).size(12.0).color(color));
                }
                if let Some(i) = icon {
                    icons::show(ui, i, 13.0, color);
                }
                if !rtl {
                    ui.label(RichText::new(text).size(12.0).color(color));
                }
            });
        })
        .response
        .interact(egui::Sense::click())
}

enum DeviceAction {
    Connect(usize),
    Edit(usize),
    Duplicate(usize),
    Favorite(usize),
    Delete(usize),
    Load(usize),
    Check(usize),
    Wake(usize),
    Tunnels(usize),
}

pub fn auth_index(m: &str) -> usize {
    match m {
        "key" => 1,
        "agent" => 2,
        _ => 0,
    }
}

pub const AUTH_IDS: [&str; 3] = ["password", "key", "agent"];

impl App {
    pub fn sidebar(&mut self, ui: &mut Ui) {
        ui.add_space(16.0);
        ui.horizontal(|ui| {
            ui.add(egui::Image::new(&self.logo).fit_to_exact_size(vec2(34.0, 34.0)));
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
                ui.spacing_mut().item_spacing.x = 0.0;
                if icons::button(ui, Icon::Settings, t!("Einstellungen", "Settings")).clicked() {
                    self.open_settings(SettingsPage::Terminal);
                }
                if self.prefs.lock_hash.is_some() && icons::button(ui, Icon::Lock, t!("Jetzt sperren", "Lock now")).clicked() {
                    self.lock_now();
                }
            });
        });
        ui.add_space(12.0);
        // Werkzeugleiste: Import, Keys, Snippets
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            let w = (ui.available_width() - 12.0) / 3.0;
            let tool = |ui: &mut Ui, icon: Icon, text: &str, tip: &str, color: Color32| {
                let (rect, resp) = ui.allocate_exact_size(vec2(w, 54.0), egui::Sense::click());
                let hov = resp.hovered();
                ui.painter().rect_filled(rect, 10.0, if hov { icons::HOVER_BG } else { Color32::from_rgb(0x1b, 0x1b, 0x29) });
                ui.painter().rect_stroke(rect, 10.0, Stroke::new(1.0, if hov { color.gamma_multiply(0.55) } else { Color32::from_rgb(0x29, 0x29, 0x3b) }), egui::StrokeKind::Inside);
                icons::paint(ui, egui::Rect::from_center_size(rect.center_top() + vec2(0.0, 19.0), vec2(18.0, 18.0)), icon, color);
                ui.painter().text(
                    rect.center_top() + vec2(0.0, 40.0),
                    egui::Align2::CENTER_CENTER,
                    text,
                    egui::FontId::proportional(11.5),
                    if hov { Color32::WHITE } else { Color32::from_gray(190) },
                );
                if hov {
                    ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                }
                resp.on_hover_text(tip).clicked()
            };
            if tool(ui, Icon::Import, t!("Import", "Import"), t!("Verbindungen importieren (PuTTY, MobaXterm, ssh_config)", "Import connections (PuTTY, MobaXterm, ssh_config)"), CYAN) {
                self.open_import();
            }
            if tool(ui, Icon::Key, t!("SSH-Keys", "SSH keys"), t!("SSH-Keys verwalten / erzeugen", "Manage / create SSH keys"), LAVENDER) {
                self.open_keys();
            }
            if tool(ui, Icon::Bolt, "Snippets", t!("Befehlsbibliothek (Strg+Umschalt+P)", "Command library (Ctrl+Shift+P)"), AMBER) {
                self.open_snippets();
            }
        });
        ui.add_space(12.0);

        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            let open_id = egui::Id::new("quick_open");
            let mut open = ui.ctx().data_mut(|d| *d.get_temp_mut_or(open_id, true));
            card(ui, |ui| {
                let rect = ui
                    .horizontal(|ui| {
                        badge_icon(ui, Icon::Plus, 24.0, LAVENDER);
                        ui.label(RichText::new(t!("Neue Verbindung", "New connection")).strong().color(Color32::WHITE));
                        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                            icons::show(ui, if open { Icon::ChevronDown } else { Icon::ChevronRight }, 15.0, MUTED);
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
        let kinds = ["ssh", "telnet", "serial"];
        let cur = kinds.iter().position(|k| *k == self.form.kind).unwrap_or(0);
        if let Some(i) = segmented(ui, &["SSH", "Telnet", "Serial"], cur) {
            self.form.kind = kinds[i].into();
            if i == 2 {
                self.refresh_ports();
            }
        }
        ui.add_space(8.0);

        let mut connect_now = false;
        if self.form.is_network() {
            field(ui, t!("Gerätename (optional)", "Device name (optional)"), &mut self.form.name, t!("z.B. Core-Switch", "e.g. core switch"));
            label(ui, t!("Host / IP *", "Host / IP *"));
            let hint = if self.form.is_telnet() { "192.168.1.1 oder host:2323" } else { t!("192.168.1.50 oder host:2222", "192.168.1.50 or host:2222") };
            let r = ui.add(TextEdit::singleline(&mut self.form.ip).id(egui::Id::new("quick_ip")).hint_text(hint).desired_width(f32::INFINITY));
            if (r.lost_focus() || r.has_focus()) && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                connect_now = true;
            }
            ui.add_space(6.0);
        }
        if self.form.is_ssh() {
            field(ui, t!("Benutzername *", "User name *"), &mut self.form.user, "root");
            label(ui, t!("Anmeldung", "Login"));
            if let Some(i) = segmented(ui, &[t!("Passwort", "Password"), "Key", "Agent"], auth_index(&self.form.auth_method)) {
                self.form.auth_method = AUTH_IDS[i].into();
                if i == 1 && self.form.key_name.is_empty() {
                    self.form.key_name = self.keys.first().map(|k| k.name.clone()).unwrap_or_default();
                }
            }
            ui.add_space(6.0);
            match self.form.auth_method.as_str() {
                "key" => {
                    ui.horizontal(|ui| {
                        let label = if self.keys.is_empty() { t!("Keine Keys gespeichert", "No keys saved").to_string() } else { self.form.key_name.clone() };
                        egui::ComboBox::from_id_salt("key").selected_text(label).width(fill_width(ui, 1) - 8.0).show_ui(ui, |ui| {
                            for k in &self.keys {
                                ui.selectable_value(&mut self.form.key_name, k.name.clone(), &k.name);
                            }
                        });
                        if icon_button(ui, Icon::Key, t!("SSH-Keys verwalten / erzeugen", "Manage / create SSH keys")).clicked() {
                            self.open_keys();
                        }
                    });
                }
                "agent" => {
                    ui.label(
                        RichText::new(t!(
                            "Nutzt die Schlüssel aus Pageant, dem Windows-OpenSSH-Agent, 1Password oder KeePassXC.",
                            "Uses the keys from Pageant, the Windows OpenSSH agent, 1Password or KeePassXC."
                        ))
                        .size(11.5)
                        .color(MUTED),
                    );
                }
                _ => {
                    ui.horizontal(|ui| {
                        let r = ui.add(TextEdit::singleline(&mut self.form.pass).password(true).hint_text(t!("Passwort", "Password")).desired_width(fill_width(ui, 1)));
                        if (r.lost_focus() || r.has_focus()) && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                            connect_now = true;
                        }
                        if ui
                            .add_enabled_ui(!self.form.pass.is_empty(), |ui| icon_button(ui, Icon::Copy, ""))
                            .inner
                            .on_hover_text(tf!(
                                "Passwort kopieren (wird nach {} s aus der Zwischenablage gelöscht)",
                                "Copy password (cleared from the clipboard after {} s)",
                                self.prefs.clipboard_clear
                            ))
                            .clicked()
                        {
                            crate::clipboard::copy_secret(self.form.pass.clone(), self.prefs.clipboard_clear);
                            self.notify(t!("Passwort kopiert", "Password copied"));
                        }
                    });
                }
            }
            ui.add_space(8.0);
        } else if self.form.is_serial() {
            field(ui, t!("Gerätename (optional)", "Device name (optional)"), &mut self.form.name, t!("z.B. Konsole Rack 3", "e.g. console rack 3"));
            label(ui, t!("COM-Port *", "COM port *"));
            ui.horizontal(|ui| {
                let label = if self.ports.is_empty() { t!("Keine Ports gefunden", "No ports found").to_string() } else { self.form.port.clone() };
                egui::ComboBox::from_id_salt("port").selected_text(label).width(fill_width(ui, 1) - 8.0).show_ui(ui, |ui| {
                    for p in &self.ports {
                        ui.selectable_value(&mut self.form.port, p.clone(), p);
                    }
                });
                if icon_button(ui, Icon::Refresh, t!("Ports aktualisieren", "Refresh ports")).clicked() {
                    self.refresh_ports();
                }
            });
            ui.add_space(6.0);
            label(ui, t!("Baudrate", "Baud rate"));
            ui.horizontal(|ui| {
                let custom = !data::default_bauds().iter().any(|d| d.label == self.baud_label);
                let w = fill_width(ui, if custom { 2 } else { 1 }) - 8.0;
                let before = self.baud_label.clone();
                egui::ComboBox::from_id_salt("baud").selected_text(self.baud_label.clone()).width(w).show_ui(ui, |ui| {
                    for b in &self.bauds {
                        ui.selectable_value(&mut self.baud_label, b.label.clone(), &b.label);
                    }
                });
                if before != self.baud_label {
                    if let Some(b) = self.bauds.iter().find(|b| b.label == self.baud_label) {
                        self.form.baud = b.rate;
                    }
                }
                if icon_button(ui, Icon::Plus, t!("Eigenes Baudrate-Profil", "Custom baud rate profile")).clicked() {
                    self.dialog = Some(Dialog::AddBaud { name: String::new(), rate: String::new() });
                }
                if custom && icon_button(ui, Icon::Trash, t!("Eigenes Profil löschen", "Delete custom profile")).clicked() {
                    let label = self.baud_label.clone();
                    self.bauds.retain(|b| b.label != label);
                    self.baud_label = "115200".into();
                    self.form.baud = 115200;
                    self.save_settings();
                }
            });
            ui.add_space(10.0);
        } else {
            ui.horizontal(|ui| {
                icons::show(ui, Icon::Warning, 14.0, AMBER);
                ui.add(egui::Label::new(RichText::new(t!("Telnet ist unverschlüsselt – nur für alte Geräte im eigenen Netz verwenden.", "Telnet is unencrypted – only use it for old devices in your own network.")).size(11.5).color(AMBER)).wrap());
            });
            ui.add_space(8.0);
        }

        ui.horizontal(|ui| {
            let w = (ui.available_width() - 6.0) / 2.0;
            let can = !self.form.is_serial() || !self.form.port.is_empty();
            if ui
                .add_enabled(can, egui::Button::new(RichText::new(t!("Verbinden", "Connect")).strong().color(Color32::WHITE)).fill(ACCENT).min_size(vec2(w, 34.0)))
                .clicked()
            {
                connect_now = true;
            }
            if ui
                .add(egui::Button::new(RichText::new(t!("Speichern", "Save")).strong()).min_size(vec2(w, 34.0)))
                .on_hover_text(t!("Als Gerät speichern", "Save as device"))
                .clicked()
            {
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
        if self.form.is_serial() {
            self.form.kind = "ssh".into();
        }
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
                alert("NebulaSSH", t!("Bitte einen COM-Port wählen.", "Please choose a COM port."));
                return;
            }
        } else if h.ip.trim().is_empty() || (h.is_ssh() && h.user.trim().is_empty()) {
            alert("NebulaSSH", t!("Host und Benutzer werden benötigt.", "Host and user are required."));
            return;
        }
        if h.name.is_empty() {
            h.name = if h.is_serial() { h.port.clone() } else { h.ip.clone() };
        }
        match self.hosts.iter().position(|x| x.same_device(&h) || (x.name == h.name && x.kind == h.kind)) {
            Some(i) => {
                // Gruppe/Favorit/Statistik und erweiterte Einstellungen behalten
                let old = self.hosts[i].clone();
                h.group = old.group;
                h.favorite = old.favorite;
                h.notes = old.notes;
                h.last_used = old.last_used;
                h.use_count = old.use_count;
                h.jump = old.jump;
                h.forwards = old.forwards;
                h.agent_forward = old.agent_forward;
                h.x11 = old.x11;
                h.mac = old.mac;
                self.hosts[i] = h;
            }
            None => self.hosts.push(h),
        }
        match data::save_hosts(&self.hosts) {
            Ok(()) => self.notify(t!("Gerät gespeichert", "Device saved")),
            Err(e) => alert(t!("Speichern", "Save"), &e),
        }
    }

    pub fn new_host_dialog(&mut self) {
        self.dialog = Some(Dialog::EditHost {
            index: None,
            host: Host { user: self.prefs.default_user.clone(), ..Default::default() },
            new_group: String::new(),
            page: HostPage::General,
            new_fwd: Default::default(),
        });
    }

    fn device_list(&mut self, ui: &mut Ui) {
        ui.horizontal(|ui| {
            ui.label(RichText::new(t!("Geräte", "Devices")).size(15.0).family(egui::FontFamily::Name("ui-bold".into())).color(Color32::WHITE));
            egui::Frame::new().fill(FIELD).corner_radius(8).inner_margin(Margin::symmetric(7, 1)).show(ui, |ui| {
                ui.label(RichText::new(self.hosts.len().to_string()).size(11.0).color(MUTED));
            });
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 0.0;
                if icons::button(ui, Icon::Plus, t!("Neues Gerät anlegen", "Add device")).clicked() {
                    self.new_host_dialog();
                }
                let reach_on = self.prefs.check_reachability;
                if icons::toggle(ui, Icon::Activity, t!("Erreichbarkeit regelmäßig prüfen (Ping + Port)", "Check reachability regularly (ping + port)"), reach_on, GREEN).clicked() {
                    self.prefs.check_reachability = !reach_on;
                    if !reach_on {
                        let hosts: Vec<Host> = self.hosts.iter().filter(|h| h.is_network()).cloned().collect();
                        for h in &hosts {
                            self.check_reach(h, false);
                        }
                    } else {
                        self.reach.clear();
                    }
                    self.save_settings();
                }
            });
        });
        ui.add_space(4.0);
        if self.hosts.len() > 3 {
            let focused = ui.memory(|m| m.has_focus(egui::Id::new("device_search")));
            egui::Frame::new()
                .fill(FIELD)
                .stroke(Stroke::new(1.0, if focused { ACCENT_HI } else { Color32::from_rgb(0x2e, 0x2e, 0x42) }))
                .corner_radius(9)
                .inner_margin(Margin { left: 9, right: 6, top: 3, bottom: 3 })
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        icons::show(ui, Icon::Search, 14.0, MUTED);
                        ui.add(
                            TextEdit::singleline(&mut self.device_search)
                                .id(egui::Id::new("device_search"))
                                .frame(egui::Frame::NONE)
                                .hint_text(t!("Geräte durchsuchen", "Search devices"))
                                .desired_width(f32::INFINITY),
                        );
                    });
                });
            ui.add_space(10.0);
        }
        if self.hosts.is_empty() {
            ui.add_space(10.0);
            ui.vertical_centered(|ui| {
                badge_icon(ui, Icon::Server, 44.0, MUTED);
                ui.add_space(8.0);
                ui.label(RichText::new(t!("Noch keine Geräte", "No devices yet")).strong().color(Color32::from_gray(210)));
                ui.label(
                    RichText::new(t!(
                        "Verbindung oben eingeben und „Speichern“ klicken – oder vorhandene Sitzungen übernehmen.",
                        "Enter a connection above and click \"Save\" – or bring over existing sessions."
                    ))
                    .size(12.0)
                    .color(MUTED),
                );
                ui.add_space(8.0);
                if icons::text_button(ui, Icon::Import, t!("Aus PuTTY & Co. importieren", "Import from PuTTY & co."), FIELD, Color32::WHITE).clicked() {
                    self.open_import();
                }
            });
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
            group_header(ui, Icon::StarFill, t!("Favoriten", "Favorites"), favs.len(), AMBER);
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
                let title = if g.is_empty() { t!("Ohne Gruppe", "No group").to_string() } else { g.clone() };
                let id = egui::Id::new(("grp", &g));
                let mut open = ui.ctx().data_mut(|d| *d.get_temp_mut_or(id, true));
                let arrow = if open { Icon::ChevronDown } else { Icon::ChevronRight };
                if group_header(ui, arrow, &title, idx.len(), Color32::from_gray(175)).clicked() {
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
                DeviceAction::Edit(i) => self.edit_host(i),
                DeviceAction::Duplicate(i) => {
                    let mut h = self.hosts[i].clone();
                    h.name = tf!("{} (Kopie)", "{} (copy)", h.display_name());
                    h.last_used = 0;
                    h.use_count = 0;
                    self.hosts.insert(i + 1, h);
                    let _ = data::save_hosts(&self.hosts);
                }
                DeviceAction::Favorite(i) => {
                    self.hosts[i].favorite = !self.hosts[i].favorite;
                    let _ = data::save_hosts(&self.hosts);
                }
                DeviceAction::Check(i) => {
                    let h = self.hosts[i].clone();
                    self.notify(tf!("Prüfe {} …", "Checking {} …", h.display_name()));
                    self.check_reach(&h, true);
                }
                DeviceAction::Wake(i) => {
                    let h = self.hosts[i].clone();
                    self.wake(&h);
                }
                DeviceAction::Tunnels(i) => {
                    self.edit_host(i);
                    if let Some(Dialog::EditHost { page, .. }) = &mut self.dialog {
                        *page = HostPage::Tunnels;
                    }
                }
                DeviceAction::Delete(i) => {
                    let name = self.hosts[i].display_name();
                    if confirm(t!("Gerät löschen", "Delete device"), &tf!("„{name}“ aus der Liste löschen?", "Remove \"{name}\" from the list?"), MessageLevel::Warning) {
                        self.hosts.remove(i);
                        let _ = data::save_hosts(&self.hosts);
                    }
                }
            }
        }
    }

    pub fn edit_host(&mut self, i: usize) {
        self.dialog = Some(Dialog::EditHost {
            index: Some(i),
            host: self.hosts[i].clone(),
            new_group: String::new(),
            page: HostPage::General,
            new_fwd: Default::default(),
        });
    }

    fn device_row(&self, ui: &mut Ui, i: usize) -> Option<DeviceAction> {
        let h = &self.hosts[i];
        let mut act = None;
        let connected = self.tabs.iter().any(|t| t.status == Status::Connected && t.host.same_device(h));
        let reach = self.reach.get(&h.ip).and_then(|r| r.1.as_ref());
        let dot = if connected {
            Some(GREEN)
        } else if self.prefs.check_reachability || reach.is_some() {
            reach.map(|r| if r.online() { Color32::from_rgb(0x3e, 0x9e, 0x5a) } else { RED })
        } else {
            None
        };
        let hover_id = egui::Id::new(("dev_hover", i));
        let was_hovered = ui.ctx().data(|d| d.get_temp::<bool>(hover_id)).unwrap_or(false);
        let fill = if was_hovered {
            Color32::from_rgb(0x24, 0x23, 0x37)
        } else if connected {
            Color32::from_rgb(0x1d, 0x1c, 0x30)
        } else {
            Color32::TRANSPARENT
        };
        let frame = egui::Frame::new().fill(fill).corner_radius(10).inner_margin(Margin { left: 7, right: 6, top: 6, bottom: 6 });
        let resp = frame
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.horizontal(|ui| {
                    tile(ui, h, 36.0, dot);
                    ui.add_space(4.0);
                    // Rechts: Merkmale und Stern; der Name bekommt den Rest
                    let mut marks: Vec<(Icon, &str)> = vec![];
                    if h.is_ssh() {
                        match h.auth_method.as_str() {
                            "key" => marks.push((Icon::Key, t!("Anmeldung per SSH-Key", "Login with SSH key"))),
                            "agent" => marks.push((Icon::Agent, t!("Anmeldung über SSH-Agent", "Login via SSH agent"))),
                            _ => {}
                        }
                        if !h.jump.is_empty() {
                            marks.push((Icon::Jump, t!("Über Jump-Host", "Via jump host")));
                        }
                        if h.forwards.iter().any(|f| f.enabled) {
                            marks.push((Icon::Tunnel, t!("Mit Tunneln", "With tunnels")));
                        }
                    }
                    let show_star = h.favorite || was_hovered;
                    let right = marks.len() as f32 * 17.0 + if show_star { 26.0 } else { 0.0 };
                    ui.vertical(|ui| {
                        ui.set_width((ui.available_width() - right - 4.0).max(40.0));
                        ui.spacing_mut().item_spacing.y = 1.0;
                        ui.add(egui::Label::new(RichText::new(h.display_name()).strong().color(Color32::WHITE)).truncate());
                        ui.add(egui::Label::new(RichText::new(h.subtitle()).size(11.5).color(MUTED)).truncate());
                    });
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        ui.spacing_mut().item_spacing.x = 3.0;
                        if show_star {
                            let (icon, col) = if h.favorite { (Icon::StarFill, AMBER) } else { (Icon::Star, Color32::from_gray(120)) };
                            let (r, sr) = ui.allocate_exact_size(vec2(24.0, 24.0), egui::Sense::click());
                            icons::paint(ui, egui::Rect::from_center_size(r.center(), vec2(15.0, 15.0)), icon, if sr.hovered() { AMBER } else { col });
                            if sr.on_hover_text(t!("Favorit", "Favorite")).clicked() {
                                act = Some(DeviceAction::Favorite(i));
                            }
                        }
                        for (icon, tip) in marks {
                            icons::show(ui, icon, 14.0, Color32::from_gray(125)).on_hover_text(tip);
                        }
                    });
                });
            })
            .response
            .interact(egui::Sense::click());
        let now_hovered = ui.rect_contains_pointer(resp.rect);
        ui.ctx().data_mut(|d| d.insert_temp(hover_id, now_hovered));
        if was_hovered != now_hovered {
            ui.ctx().request_repaint();
        }
        if resp.hovered() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }
        let resp = resp.on_hover_ui(|ui| {
            ui.label(RichText::new(h.display_name()).strong());
            ui.label(h.subtitle());
            if !h.group.is_empty() {
                ui.label(tf!("Gruppe: {}", "Group: {}", h.group));
            }
            if !h.jump.is_empty() {
                ui.label(tf!("Über Jump-Host: {}", "Via jump host: {}", h.jump));
            }
            for f in h.forwards.iter().filter(|f| f.enabled) {
                ui.label(tf!("Tunnel {}", "Tunnel {}", f.label()));
            }
            if let Some(r) = reach {
                ui.label(r.summary());
            }
            ui.label(tf!("Zuletzt: {} · {} Verbindungen", "Last: {} · {} connections", data::ago(h.last_used), h.use_count));
            if !h.notes.is_empty() {
                ui.separator();
                ui.label(&h.notes);
            }
            ui.separator();
            ui.label(RichText::new(t!("Klick = verbinden · Rechtsklick = mehr", "Click = connect · right-click = more")).size(11.0).color(MUTED));
        });
        if resp.clicked() && act.is_none() {
            act = Some(DeviceAction::Connect(i));
        }
        resp.context_menu(|ui| {
            if icons::menu(ui, Icon::Play, t!("Verbinden", "Connect")).clicked() {
                act = Some(DeviceAction::Connect(i));
            }
            if icons::menu(ui, Icon::Pencil, t!("Bearbeiten…", "Edit…")).clicked() {
                act = Some(DeviceAction::Edit(i));
            }
            if h.is_ssh() && icons::menu(ui, Icon::Tunnel, t!("Tunnel…", "Tunnels…")).clicked() {
                act = Some(DeviceAction::Tunnels(i));
            }
            if icons::menu(ui, Icon::Import, t!("In Formular laden", "Load into form")).clicked() {
                act = Some(DeviceAction::Load(i));
            }
            if icons::menu(ui, Icon::Copy, t!("Duplizieren", "Duplicate")).clicked() {
                act = Some(DeviceAction::Duplicate(i));
            }
            if icons::menu(ui, if h.favorite { Icon::Star } else { Icon::StarFill }, if h.favorite { t!("Kein Favorit mehr", "Remove favorite") } else { t!("Als Favorit", "Add to favorites") }).clicked() {
                act = Some(DeviceAction::Favorite(i));
            }
            if h.is_network() {
                ui.separator();
                if icons::menu(ui, Icon::Activity, t!("Ping / Port prüfen", "Ping / check port")).clicked() {
                    act = Some(DeviceAction::Check(i));
                }
                if ui
                    .add_enabled(!h.mac.trim().is_empty(), egui::Button::image_and_text(icons::image(ui.ctx(), Icon::Power, 15.0, Color32::from_gray(220)), t!("Aufwecken (Wake-on-LAN)", "Wake up (Wake-on-LAN)")))
                    .on_disabled_hover_text(t!("MAC-Adresse unter Bearbeiten → Erweitert eintragen", "Enter the MAC address under Edit → Advanced"))
                    .clicked()
                {
                    act = Some(DeviceAction::Wake(i));
                }
            }
            ui.separator();
            if icons::menu_colored(ui, Icon::Trash, t!("Löschen", "Delete"), RED).clicked() {
                act = Some(DeviceAction::Delete(i));
            }
        });
        ui.add_space(2.0);
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
            Split(u64),
            Detach(u64, Option<egui::Pos2>),
            Tunnels(u64),
        }
        let mut act = None;
        let mut new_tab = false;
        let window = ui.ctx().content_rect();
        let outer = ui.input(|i| i.viewport().outer_rect);
        ui.horizontal_centered(|ui| {
            ui.spacing_mut().item_spacing.x = 0.0;
            ui.add_space(6.0);
            if icons::button(ui, Icon::Sidebar, t!("Seitenleiste ein/aus (Strg+Umschalt+B)", "Toggle sidebar (Ctrl+Shift+B)")).clicked() {
                self.prefs.sidebar_open = !self.prefs.sidebar_open;
                self.save_settings();
            }
            egui::ScrollArea::horizontal()
                .max_width(ui.available_width() - 270.0)
                .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden)
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 0.0;
                        let panes = self.visible_panes();
                        for t in &self.tabs {
                            let active = self.active == Some(t.id);
                            let shown = panes.contains(&t.id);
                            let accent = match t.host.kind.as_str() {
                                "serial" => AMBER,
                                "telnet" => CYAN,
                                _ => ACCENT_HI,
                            };
                            let fill = if active { BG } else if shown { Color32::from_rgb(0x14, 0x14, 0x1f) } else { Color32::TRANSPARENT };
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
                                        let (ic, gcol) = kind_icon(&t.host);
                                        let (r, _) = ui.allocate_exact_size(vec2(20.0, 20.0), egui::Sense::hover());
                                        icons::paint(ui, egui::Rect::from_center_size(r.center(), vec2(15.0, 15.0)), ic, if active { gcol } else { gcol.gamma_multiply(0.75) });
                                        let c = r.right_top() + vec2(-1.0, 2.0);
                                        ui.painter().circle_filled(c, 4.0, if active { BG } else { PANEL });
                                        ui.painter().circle_filled(c, 2.8, t.status_color());
                                        let mut title = t.title.clone();
                                        if t.detached.is_some() {
                                            title = format!("⧉ {title}");
                                        }
                                        let txt = RichText::new(title).size(13.0).color(if active { Color32::WHITE } else { Color32::from_gray(185) });
                                        let l = ui.add(egui::Label::new(txt).sense(egui::Sense::click()).truncate());
                                        if l.clicked() {
                                            act = Some(if t.detached.is_some() { A::Detach(t.id, None) } else { A::Switch(t.id) });
                                        }
                                        let (cr, cresp) = ui.allocate_exact_size(vec2(20.0, 20.0), egui::Sense::click());
                                        if cresp.hovered() {
                                            ui.painter().rect_filled(cr, 5.0, icons::HOVER_BG);
                                        }
                                        icons::paint(ui, egui::Rect::from_center_size(cr.center(), vec2(13.0, 13.0)), Icon::Close, if cresp.hovered() { Color32::WHITE } else { MUTED });
                                        if cresp.on_hover_text(t!("Tab schließen (Strg+Umschalt+W)", "Close tab (Ctrl+Shift+W)")).clicked() {
                                            act = Some(A::Close(t.id));
                                        }
                                    });
                                })
                                .response
                                .interact(egui::Sense::click_and_drag());
                            let r = resp.rect;
                            if active {
                                ui.painter().line_segment([r.left_top() + vec2(8.0, 1.0), r.right_top() + vec2(-8.0, 1.0)], Stroke::new(2.0, accent));
                            } else if resp.hovered() {
                                ui.painter().rect_stroke(r, CornerRadius { nw: 9, ne: 9, sw: 0, se: 0 }, Stroke::new(1.0, LINE), egui::StrokeKind::Inside);
                            }
                            if resp.dragged() {
                                ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
                            }
                            // Tab aus dem Fenster ziehen = eigenes Fenster
                            if resp.drag_stopped() {
                                if let Some(p) = ui.input(|i| i.pointer.latest_pos()) {
                                    if !window.expand(8.0).contains(p) || p.y > window.top() + 160.0 {
                                        let screen = outer.map(|o| o.min + p.to_vec2() - vec2(120.0, 20.0));
                                        act = Some(A::Detach(t.id, screen));
                                    }
                                }
                            }
                            if resp.clicked() {
                                act = Some(if t.detached.is_some() { A::Detach(t.id, None) } else { A::Switch(t.id) });
                            }
                            if resp.middle_clicked() {
                                act = Some(A::Close(t.id));
                            }
                            let id = t.id;
                            let in_split = self.split.contains(&id);
                            let is_ssh = t.is_ssh() && t.ssh.is_some();
                            let detached = t.detached.is_some();
                            resp.on_hover_text(format!(
                                "{}\n{}\n{}",
                                t.title,
                                t.host.subtitle(),
                                t!("Ziehen: in eigenes Fenster · Rechtsklick: mehr", "Drag out: own window · right-click: more")
                            ))
                            .context_menu(|ui| {
                                if icons::menu(ui, Icon::Refresh, t!("Neu verbinden", "Reconnect")).clicked() {
                                    act = Some(A::Reconnect(id));
                                }
                                if icons::menu(ui, Icon::Copy, t!("Duplizieren", "Duplicate")).clicked() {
                                    act = Some(A::Duplicate(id));
                                }
                                ui.separator();
                                let split_label = if in_split { t!("Aus geteilter Ansicht nehmen", "Remove from split view") } else { t!("In geteilte Ansicht", "Add to split view") };
                                if icons::menu(ui, Icon::Split, split_label).clicked() {
                                    act = Some(A::Split(id));
                                }
                                let win_label = if detached { t!("Zurück ins Hauptfenster", "Back to main window") } else { t!("In eigenem Fenster", "Open in own window") };
                                if icons::menu(ui, Icon::Window, win_label).clicked() {
                                    act = Some(A::Detach(id, None));
                                }
                                if is_ssh && icons::menu(ui, Icon::Tunnel, t!("Tunnel…", "Tunnels…")).clicked() {
                                    act = Some(A::Tunnels(id));
                                }
                                ui.separator();
                                if icons::menu(ui, Icon::Download, t!("Ausgabe speichern…", "Save output…")).clicked() {
                                    act = Some(A::SaveOutput(id));
                                }
                                if icons::menu(ui, Icon::Eraser, t!("Terminal leeren", "Clear terminal")).clicked() {
                                    act = Some(A::Clear(id));
                                }
                                ui.separator();
                                if icons::menu(ui, Icon::Close, t!("Andere Tabs schließen", "Close other tabs")).clicked() {
                                    act = Some(A::CloseOthers(id));
                                }
                                if icons::menu_colored(ui, Icon::Close, t!("Schließen", "Close"), RED).clicked() {
                                    act = Some(A::Close(id));
                                }
                            });
                        }
                        ui.add_space(4.0);
                        if icons::button(ui, Icon::Plus, t!("Neue Verbindung (Strg+Umschalt+T)", "New connection (Ctrl+Shift+T)")).clicked() {
                            new_tab = true;
                        }
                    });
                });

            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                ui.add_space(8.0);
                let is_ssh = self.active.is_some_and(|a| self.tabs.iter().any(|t| t.id == a && t.is_ssh()));
                if is_ssh {
                    let fill = if self.show_files { ACCENT } else { Color32::from_rgb(0x2e, 0x2e, 0x45) };
                    if icons::text_button(ui, Icon::Folder, t!("Dateien", "Files"), fill, Color32::WHITE).on_hover_text(t!("SFTP-Datei-Explorer (Strg+Umschalt+E)", "SFTP file explorer (Ctrl+Shift+E)")).clicked() {
                        self.toggle_files();
                    }
                }
                if self.tabs.len() > 1 {
                    let split_on = self.visible_panes().len() > 1;
                    let b = egui::Button::new(icons::image(ui.ctx(), Icon::Split, 15.0, Color32::WHITE)).fill(if split_on { ACCENT } else { Color32::from_rgb(0x2e, 0x2e, 0x45) }).min_size(vec2(34.0, 30.0));
                    let r = ui.add(b).on_hover_text(t!("Geteilte Ansicht (Strg+Umschalt+S)", "Split view (Ctrl+Shift+S)"));
                    egui::Popup::menu(&r).show(|ui| {
                        ui.set_min_width(240.0);
                        ui.label(RichText::new(t!("Nebeneinander anzeigen (max. 4)", "Show side by side (max. 4)")).color(MUTED).size(12.0));
                        let ids: Vec<(u64, String)> = self.tabs.iter().filter(|t| t.detached.is_none()).map(|t| (t.id, t.title.clone())).collect();
                        for (id, title) in ids {
                            let mut on = self.split.contains(&id) || (self.split.is_empty() && self.active == Some(id));
                            if ui.checkbox(&mut on, title).changed() {
                                self.toggle_split(id);
                            }
                        }
                        ui.separator();
                        if ui.button(t!("Alle nebeneinander", "All side by side")).clicked() {
                            self.split = self.tabs.iter().filter(|t| t.detached.is_none()).map(|t| t.id).take(MAX_SPLIT).collect();
                            if !self.active.is_some_and(|a| self.split.contains(&a)) {
                                self.active = self.split.first().copied();
                            }
                        }
                        if ui.add_enabled(!self.split.is_empty(), egui::Button::new(t!("Geteilte Ansicht beenden", "End split view"))).clicked() {
                            self.split.clear();
                        }
                    });
                    let (fill, fg) = if self.broadcast { (AMBER, Color32::BLACK) } else { (Color32::from_rgb(0x2e, 0x2e, 0x45), Color32::WHITE) };
                    if icons::text_button(ui, Icon::Broadcast, t!("Alle", "All"), fill, fg).on_hover_text(t!("Broadcast: Eingaben gleichzeitig an alle verbundenen Tabs senden", "Broadcast: send input to all connected tabs at once")).clicked() {
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
            Some(A::Split(id)) => self.toggle_split(id),
            Some(A::Detach(id, pos)) => {
                if self.tabs.iter().any(|t| t.id == id && t.detached.is_some()) {
                    self.attach(id);
                } else {
                    self.detach(id, pos);
                }
            }
            Some(A::Tunnels(id)) => self.open_tunnels(id),
            Some(A::Clear(id)) => {
                if let Some(t) = self.tab(id) {
                    t.term.clear();
                }
            }
            Some(A::SaveOutput(id)) => {
                if let Some(t) = self.tabs.iter().find(|t| t.id == id) {
                    let name = format!("{}_{}.txt", t.title.replace(['/', '\\', ':', '@', ' '], "_"), chrono::Local::now().format("%Y-%m-%d_%H-%M"));
                    if let Some(p) = rfd::FileDialog::new().set_title(t!("Terminal-Ausgabe speichern", "Save terminal output")).set_file_name(&name).save_file() {
                        match std::fs::write(&p, t.term.all_text()) {
                            Ok(()) => self.notify(t!("Ausgabe gespeichert", "Output saved")),
                            Err(e) => alert(t!("Speichern", "Save"), &e.to_string()),
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
        let mut open_lib = false;
        let mut open_tunnels = None;
        ui.horizontal_centered(|ui| {
            if ui
                .add(
                    egui::Button::image_and_text(icons::image(ui.ctx(), Icon::Bolt, 14.0, AMBER), RichText::new("Snippets").size(13.0).color(AMBER))
                        .fill(PANEL2)
                        .corner_radius(14)
                        .min_size(vec2(0.0, 26.0)),
                )
                .on_hover_text(t!("Befehlsbibliothek (Strg+Umschalt+P)", "Command library (Ctrl+Shift+P)"))
                .clicked()
            {
                open_lib = true;
            }
            let right_w = 470.0;
            let w = (ui.available_width() - right_w).max(80.0);
            egui::ScrollArea::horizontal()
                .max_width(w)
                .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden)
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        for s in self.snippets.iter().filter(|s| s.pinned) {
                            let r = ui
                                .add(egui::Button::new(RichText::new(&s.name).size(13.0)).fill(PANEL2).corner_radius(14).min_size(vec2(0.0, 26.0)))
                                .on_hover_text(s.cmd.trim_end());
                            if r.clicked() {
                                run = Some(s.clone());
                            }
                        }
                    });
                });

            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                self.update_badge(ui);
                if let Some(t) = self.active.and_then(|id| self.tabs.iter().find(|t| t.id == id)) {
                    let (icon, txt, col) = match t.status {
                        Status::Connected if t.is_ssh() => (Icon::Lock, t!("SSH verschlüsselt", "SSH encrypted"), GREEN),
                        Status::Connected if t.host.is_telnet() => (Icon::Warning, t!("Telnet unverschlüsselt", "Telnet unencrypted"), AMBER),
                        Status::Connected => (Icon::Plug, t!("Serial verbunden", "Serial connected"), AMBER),
                        Status::Connecting => (Icon::Clock, t!("verbinde …", "connecting …"), AMBER),
                        Status::Closed => (Icon::Close, t!("getrennt", "disconnected"), RED),
                    };
                    ui.label(RichText::new(format!("{}×{}", t.term.size.cols, t.term.size.rows)).size(12.0).color(MUTED));
                    if t.log.is_some() {
                        ui.label(RichText::new("● REC").size(12.0).color(RED)).on_hover_text(t!("Sitzung wird protokolliert", "Session is being logged"));
                    }
                    if let Some(at) = t.connected_at.filter(|_| t.status == Status::Connected) {
                        let s = at.elapsed().as_secs();
                        ui.label(RichText::new(format!("{:02}:{:02}:{:02}", s / 3600, (s / 60) % 60, s % 60)).size(12.0).color(MUTED));
                        ui.ctx().request_repaint_after(std::time::Duration::from_secs(1));
                    }
                    chip(ui, Some(icon), txt, col);
                    if let Some(s) = &t.ssh {
                        let tun = s.tunnels();
                        if !tun.is_empty() {
                            let bad = tun.iter().any(|x| x.error.is_some());
                            let txt = tf!("{} Tunnel", "{} tunnels", tun.len());
                            if chip(ui, Some(Icon::Tunnel), &txt, if bad { RED } else { CYAN })
                                .on_hover_text(tun.iter().map(|x| format!("{}{}", x.fwd.label(), x.error.as_ref().map(|e| format!(" – {e}")).unwrap_or_default())).collect::<Vec<_>>().join("\n"))
                                .clicked()
                            {
                                open_tunnels = Some(t.id);
                            }
                        }
                    }
                    if let Some(st) = t.stats.value.as_ref().filter(|_| self.prefs.server_stats && t.status == Status::Connected) {
                        server_stats(ui, st);
                    }
                }
            });
        });
        if let Some(s) = run {
            self.run_snippet(s);
        }
        if open_lib {
            self.open_snippets();
        }
        if let Some(id) = open_tunnels {
            self.open_tunnels(id);
        }
    }
}

/// Kompakte Server-Anzeige: CPU, RAM, Festplatte (rechts nach links)
fn server_stats(ui: &mut Ui, s: &nebula_core::stats::Stats) {
    let color = |p: f32| if p >= 90.0 { RED } else if p >= 70.0 { AMBER } else { Color32::from_gray(190) };
    let bar = |ui: &mut Ui, label: &str, p: f32| {
        // Die Statusleiste ist rechtsbündig (rechts nach links) – daher umgekehrte Reihenfolge
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            ui.label(RichText::new(format!("{p:.0}%")).size(11.5).color(color(p)));
            let (r, _) = ui.allocate_exact_size(vec2(34.0, 6.0), egui::Sense::hover());
            ui.painter().rect_filled(r, 3.0, Color32::from_rgb(0x2a, 0x2a, 0x3c));
            let mut f = r;
            f.set_width(r.width() * (p / 100.0).clamp(0.02, 1.0));
            ui.painter().rect_filled(f, 3.0, color(p));
            ui.label(RichText::new(label).size(11.5).color(MUTED));
        });
    };
    let resp = ui
        .horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 10.0;
            bar(ui, "Disk", s.disk_pct());
            bar(ui, "RAM", s.mem_pct());
            bar(ui, "CPU", s.cpu);
        })
        .response;
    resp.on_hover_text(tf!(
        "CPU {:.0}% ({} Kerne, Last {:.2})\nRAM {} / {}\nFestplatte / {} / {}\nLaufzeit {}",
        "CPU {:.0}% ({} cores, load {:.2})\nRAM {} / {}\nDisk / {} / {}\nUptime {}",
        s.cpu,
        s.cores,
        s.load1,
        human_size(s.mem_used),
        human_size(s.mem_total),
        human_size(s.disk_used),
        human_size(s.disk_total),
        nebula_core::stats::uptime_text(s.uptime_secs)
    ));
}

/// Zwischenüberschrift für Gruppen in der Geräteliste (klickbar).
fn group_header(ui: &mut Ui, icon: Icon, title: &str, count: usize, color: Color32) -> egui::Response {
    ui.add_space(4.0);
    let rect = ui
        .horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            icons::show(ui, icon, 13.0, color);
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
