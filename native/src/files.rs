//! SFTP-Datei-Explorer: Liste mit Spalten, Navigation, Drag & Drop, Übertragungen.

use crate::app::*;
use crate::sidebar::icon_button;
use eframe::egui::{self, pos2, vec2, Align, Align2, Color32, FontId, Layout, RichText, Sense, Stroke, TextEdit, Ui};
use nebula_core::ssh::{self, join, FileInfo};
use rfd::MessageLevel;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

enum A {
    Open(FileInfo),
    Edit(FileInfo),
    Download(FileInfo),
    Delete(FileInfo),
    Rename(String),
    Chmod(FileInfo),
    CopyPath(String),
    Cd(String),
    Go(String),
    Back,
    Up,
    Home,
    Reload,
    Upload,
    UploadDir,
    MkDir,
    NewFile,
    Sort(SortBy),
    Close,
}

fn parent(path: &str) -> String {
    let mut parts: Vec<&str> = path.split('/').filter(|p| !p.is_empty()).collect();
    parts.pop();
    format!("/{}", parts.join("/"))
}

fn fmt_time(ts: Option<u32>) -> String {
    use chrono::TimeZone;
    match ts.and_then(|t| chrono::Local.timestamp_opt(t as i64, 0).single()) {
        Some(d) => d.format("%d.%m.%Y %H:%M").to_string(),
        None => String::new(),
    }
}

fn file_icon(f: &FileInfo) -> &'static str {
    if f.name == ".." {
        return "⮤";
    }
    if f.is_dir {
        return "📁";
    }
    let ext = f.name.rsplit('.').next().unwrap_or("").to_lowercase();
    match ext.as_str() {
        "sh" | "bash" | "py" | "pl" | "rb" | "js" | "ts" | "rs" | "go" | "c" | "h" | "cpp" | "php" => "📜",
        "conf" | "cfg" | "ini" | "yml" | "yaml" | "toml" | "json" | "xml" | "env" => "⚙",
        "log" | "txt" | "md" => "📄",
        "zip" | "gz" | "tgz" | "tar" | "xz" | "bz2" | "7z" | "deb" | "rpm" => "📦",
        "png" | "jpg" | "jpeg" | "gif" | "svg" | "webp" | "ico" => "🖼",
        "key" | "pem" | "crt" | "pub" => "🔐",
        _ => "📄",
    }
}

impl App {
    /// Erster Aufruf: Home-Verzeichnis ermitteln und anzeigen.
    pub fn sftp_init(&mut self, id: u64) {
        let tx = self.tx.clone();
        let ctx = self.ctx.clone();
        let Some(t) = self.tab(id) else { return };
        let Some(s) = t.ssh.clone() else { return };
        t.sftp.initialized = true;
        t.sftp.loading = true;
        self.rt.spawn(async move {
            let res = ssh::home(&s).await;
            let _ = tx.send(Msg::SftpHome(id, res));
            ctx.request_repaint();
        });
    }

    pub fn load_dir(&mut self, id: u64, path: String, push_history: bool) {
        let tx = self.tx.clone();
        let ctx = self.ctx.clone();
        let Some(t) = self.tab(id) else { return };
        let Some(s) = t.ssh.clone() else { return };
        if push_history && !t.sftp.path.is_empty() && t.sftp.path != path {
            let p = t.sftp.path.clone();
            t.sftp.history.push(p);
        }
        t.sftp.loading = true;
        t.sftp.error = None;
        let path = if path.trim().is_empty() { "/".to_string() } else { path.trim().to_string() };
        self.rt.spawn(async move {
            let res = ssh::list_directory(&s, &path).await;
            let _ = tx.send(Msg::SftpList(id, path, res));
            ctx.request_repaint();
        });
    }

    pub fn reload_dir(&mut self, id: u64) {
        let path = self.tabs.iter().find(|t| t.id == id).map(|t| t.sftp.path.clone());
        if let Some(p) = path.filter(|p| !p.is_empty()) {
            self.load_dir(id, p, false);
        }
    }

    pub fn on_list(&mut self, id: u64, path: String, res: Result<Vec<FileInfo>, String>) {
        let Some(t) = self.tab(id) else { return };
        t.sftp.loading = false;
        match res {
            Ok(files) => {
                t.sftp.path_edit = path.clone();
                t.sftp.path = path;
                t.sftp.files = files;
                t.sftp.selected = None;
            }
            Err(e) => {
                // Pfad zurücksetzen, Fehler anzeigen
                t.sftp.path_edit = t.sftp.path.clone();
                t.sftp.error = Some(e);
            }
        }
    }

    pub fn sftp_op<F, Fut>(&mut self, id: u64, what: &'static str, f: F)
    where
        F: FnOnce(Arc<ssh::SshSession>) -> Fut + Send + 'static,
        Fut: std::future::Future<Output = Result<(), String>> + Send,
    {
        let tx = self.tx.clone();
        let ctx = self.ctx.clone();
        let Some(s) = self.tab(id).and_then(|t| t.ssh.clone()) else {
            alert("Dateien", "Keine aktive SSH-Verbindung.");
            return;
        };
        self.rt.spawn(async move {
            let res = f(s).await;
            let _ = tx.send(Msg::SftpDone(id, res, what));
            ctx.request_repaint();
        });
    }

    /// Startet einen Upload (Datei oder Ordner) mit Fortschrittsanzeige.
    pub fn start_upload(&mut self, id: u64, local: std::path::PathBuf, remote_dir: String) {
        let Some(s) = self.tab(id).and_then(|t| t.ssh.clone()) else { return };
        let local_s = local.to_string_lossy().to_string();
        let name = local.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
        let done = Arc::new(AtomicU64::new(0));
        let tid = self.next_transfer_id();
        self.transfers.push(Transfer {
            id: tid,
            label: name,
            upload: true,
            total: ssh::local_size(&local_s),
            done: done.clone(),
            state: TransferState::Running,
        });
        let tx = self.tx.clone();
        let ctx = self.ctx.clone();
        self.rt.spawn(async move {
            let res = ssh::upload_with(&s, &remote_dir, &local_s, Some(done)).await;
            let _ = tx.send(Msg::TransferDone(tid, id, res));
            ctx.request_repaint();
        });
    }

    fn start_download(&mut self, id: u64, remote: String, size: u64, local: std::path::PathBuf) {
        let Some(s) = self.tab(id).and_then(|t| t.ssh.clone()) else { return };
        let name = remote.rsplit('/').next().unwrap_or("").to_string();
        let done = Arc::new(AtomicU64::new(0));
        let tid = self.next_transfer_id();
        self.transfers.push(Transfer {
            id: tid,
            label: name,
            upload: false,
            total: size,
            done: done.clone(),
            state: TransferState::Running,
        });
        let tx = self.tx.clone();
        let ctx = self.ctx.clone();
        let local_s = local.to_string_lossy().to_string();
        self.rt.spawn(async move {
            let res = ssh::download_with(&s, &remote, &local_s, Some(done)).await;
            let _ = tx.send(Msg::TransferDone(tid, id, res));
            ctx.request_repaint();
        });
    }

    pub fn open_editor(&mut self, id: u64, path: String, new_file: bool) {
        let tx = self.tx.clone();
        let ctx = self.ctx.clone();
        let Some(s) = self.tab(id).and_then(|t| t.ssh.clone()) else { return };
        self.dialog = Some(Dialog::Editor {
            tab: id,
            path: path.clone(),
            content: String::new(),
            original: String::new(),
            loading: !new_file,
            saving: false,
        });
        if !new_file {
            self.rt.spawn(async move {
                let res = ssh::read_file(&s, &path).await;
                let _ = tx.send(Msg::FileLoaded(id, path, res));
                ctx.request_repaint();
            });
        }
    }

    /// Dateien aus dem Windows-Explorer ins Fenster gezogen.
    pub fn drop_files(&mut self, ui: &mut Ui) {
        let (hovering, dropped) = ui.input(|i| (!i.raw.hovered_files.is_empty(), i.raw.dropped_files.clone()));
        let target = self
            .active
            .filter(|id| self.tabs.iter().any(|t| t.id == *id && t.ssh.is_some()));
        if hovering {
            let rect = ui.ctx().content_rect();
            let painter = ui.ctx().layer_painter(egui::LayerId::new(egui::Order::Foreground, egui::Id::new("dnd")));
            painter.rect_filled(rect, 0.0, Color32::from_rgba_unmultiplied(20, 10, 40, 200));
            painter.rect_stroke(rect.shrink(16.0), 16.0, Stroke::new(3.0, if target.is_some() { ACCENT_HI } else { RED }), egui::StrokeKind::Inside);
            let text = match target {
                Some(id) => {
                    let dir = self.tabs.iter().find(|t| t.id == id).map(|t| t.sftp.path.clone()).unwrap_or_default();
                    format!("⬆  Loslassen zum Hochladen nach\n{}", if dir.is_empty() { "Home-Verzeichnis".into() } else { dir })
                }
                None => "Zum Hochladen zuerst eine SSH-Verbindung öffnen".into(),
            };
            painter.text(rect.center(), Align2::CENTER_CENTER, text, FontId::proportional(22.0), Color32::WHITE);
        }
        if dropped.is_empty() {
            return;
        }
        let Some(id) = target else {
            self.notify_err("Kein SSH-Tab aktiv – Upload nicht möglich");
            return;
        };
        if !self.show_files {
            self.toggle_files();
        }
        let dir = self.tabs.iter().find(|t| t.id == id).map(|t| t.sftp.path.clone()).unwrap_or_default();
        let dir = if dir.is_empty() { ".".to_string() } else { dir };
        for f in dropped {
            let p = f.path();
            if !p.as_os_str().is_empty() {
                self.start_upload(id, p.to_path_buf(), dir.clone());
            }
        }
    }

    pub fn files_panel(&mut self, ui: &mut Ui, id: u64) {
        let mut act: Option<A> = None;
        let transfers_h = self.transfers_height();
        let Some(t) = self.tabs.iter_mut().find(|t| t.id == id) else { return };
        let connected = t.ssh.is_some();

        // Kopf
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            ui.label(RichText::new("📂 Dateien").strong().size(15.0).color(Color32::WHITE));
            ui.label(RichText::new(t.host.subtitle()).size(12.0).color(MUTED));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if ui.add(egui::Button::new(RichText::new("✖").color(MUTED)).frame(false)).on_hover_text("Schließen").clicked() {
                    act = Some(A::Close);
                }
            });
        });
        ui.add_space(4.0);
        if !connected {
            ui.add_space(20.0);
            ui.vertical_centered(|ui| {
                ui.label(RichText::new("Keine Verbindung – Dateien sind erst nach dem Verbinden verfügbar.").color(MUTED));
            });
            if let Some(A::Close) = act {
                self.show_files = false;
            }
            return;
        }

        // Navigation
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            if ui.add_enabled_ui(!t.sftp.history.is_empty(), |ui| ui.add_sized(vec2(32.0, 28.0), egui::Button::new("⬅"))).inner.on_hover_text("Zurück").clicked() {
                act = Some(A::Back);
            }
            if icon_button(ui, "⬆", "Übergeordneter Ordner").clicked() {
                act = Some(A::Up);
            }
            if icon_button(ui, "🏠", "Home-Verzeichnis").clicked() {
                act = Some(A::Home);
            }
            let w = crate::sidebar::fill_width(ui, 2);
            if t.sftp.editing_path {
                let r = ui.add(TextEdit::singleline(&mut t.sftp.path_edit).desired_width(w).font(egui::TextStyle::Monospace));
                if !r.has_focus() && !r.lost_focus() {
                    r.request_focus();
                }
                if (r.lost_focus() || r.has_focus()) && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                    act = Some(A::Go(t.sftp.path_edit.clone()));
                    t.sftp.editing_path = false;
                } else if r.lost_focus() || ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                    t.sftp.editing_path = false;
                }
            } else {
                // Klickbare Pfad-Teile ("Brotkrumen")
                let path = t.sftp.path.clone();
                egui::Frame::new().fill(FIELD).corner_radius(6).inner_margin(egui::Margin::symmetric(4, 0)).show(ui, |ui| {
                    ui.set_width(w - 8.0);
                    ui.set_height(28.0);
                    egui::ScrollArea::horizontal()
                        .id_salt("crumbs")
                        .max_width(w - 8.0)
                        .stick_to_right(true)
                        .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden)
                        .show(ui, |ui| {
                            ui.horizontal_centered(|ui| {
                                ui.spacing_mut().item_spacing.x = 0.0;
                                let crumb = |ui: &mut Ui, text: &str| {
                                    ui.add(egui::Button::new(RichText::new(text).size(13.0).color(Color32::from_gray(225))).frame_when_inactive(false))
                                };
                                if crumb(ui, "/").clicked() {
                                    act = Some(A::Go("/".into()));
                                }
                                let mut acc = String::new();
                                for part in path.split('/').filter(|p| !p.is_empty()) {
                                    acc.push('/');
                                    acc.push_str(part);
                                    ui.label(RichText::new(" › ").color(Color32::from_gray(90)));
                                    if crumb(ui, part).clicked() {
                                        act = Some(A::Go(acc.clone()));
                                    }
                                }
                            });
                        });
                });
            }
            if icon_button(ui, "✎", "Pfad eingeben").clicked() {
                t.sftp.editing_path = !t.sftp.editing_path;
                t.sftp.path_edit = t.sftp.path.clone();
            }
            if icon_button(ui, "⟳", "Aktualisieren").clicked() {
                act = Some(A::Reload);
            }
        });
        // Aktionen
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            if ui.button("⬆ Hochladen").on_hover_text("Dateien hochladen – oder einfach ins Fenster ziehen").clicked() {
                act = Some(A::Upload);
            }
            if ui.button("⬆ Ordner").on_hover_text("Ganzen Ordner hochladen").clicked() {
                act = Some(A::UploadDir);
            }
            if ui.button("+ Ordner").on_hover_text("Neuer Ordner").clicked() {
                act = Some(A::MkDir);
            }
            if ui.button("+ Datei").clicked() {
                act = Some(A::NewFile);
            }
        });
        ui.horizontal(|ui| {
            ui.checkbox(&mut t.sftp.show_hidden, "Versteckte").on_hover_text("Dateien mit . am Anfang anzeigen");
            ui.add(TextEdit::singleline(&mut t.sftp.filter).hint_text("🔍 Filter").desired_width(ui.available_width()));
        });
        if let Some(e) = &t.sftp.error {
            ui.label(RichText::new(format!("⚠ {e}")).color(RED).size(12.0));
        }
        ui.add_space(4.0);

        // Spaltenköpfe
        let w = ui.available_width();
        let cols = [w * 0.50, w * 0.16, w * 0.34];
        let header = ui.allocate_exact_size(vec2(w, 22.0), Sense::hover()).0;
        let heads = [("Name", SortBy::Name), ("Größe", SortBy::Size), ("Geändert", SortBy::Date)];
        let mut x = header.left();
        for (i, (label, by)) in heads.iter().enumerate() {
            let r = egui::Rect::from_min_size(pos2(x, header.top()), vec2(cols[i], 22.0));
            let resp = ui.interact(r, ui.scope_id().with(("hdr", i)), Sense::click());
            let arrow = if t.sftp.sort == *by { if t.sftp.asc { " ▲" } else { " ▼" } } else { "" };
            ui.painter().text(
                r.left_center() + vec2(6.0, 0.0),
                Align2::LEFT_CENTER,
                format!("{label}{arrow}"),
                FontId::proportional(12.0),
                if resp.hovered() { Color32::WHITE } else { MUTED },
            );
            if resp.clicked() {
                act = Some(A::Sort(*by));
            }
            x += cols[i];
        }
        ui.painter().line_segment([header.left_bottom(), header.right_bottom()], Stroke::new(1.0, LINE));

        // Liste
        let filter = t.sftp.filter.to_lowercase();
        let mut files: Vec<&FileInfo> = t
            .sftp
            .files
            .iter()
            .filter(|f| f.name == ".." || t.sftp.show_hidden || !f.name.starts_with('.'))
            .filter(|f| filter.is_empty() || f.name == ".." || f.name.to_lowercase().contains(&filter))
            .collect();
        let (sort, asc) = (t.sftp.sort, t.sftp.asc);
        files.sort_by(|a, b| {
            let top = (a.name != "..").cmp(&(b.name != "..")).then(b.is_dir.cmp(&a.is_dir));
            let ord = match sort {
                SortBy::Name => a.name.to_lowercase().cmp(&b.name.to_lowercase()),
                SortBy::Size => a.size.cmp(&b.size),
                SortBy::Date => a.modified.cmp(&b.modified),
            };
            top.then(if asc { ord } else { ord.reverse() })
        });
        let list_h = ui.available_height() - transfers_h;
        let selected = t.sftp.selected.clone();
        let mut new_sel = None;
        let loading = t.sftp.loading;
        let path = t.sftp.path.clone();
        let count = files.iter().filter(|f| f.name != "..").count();
        egui::ScrollArea::vertical().max_height(list_h).auto_shrink([false, false]).show_rows(ui, 26.0, files.len(), |ui, range| {
            // Innerhalb der ScrollArea ist wegen der Scrollbar weniger Platz als außerhalb
            let w = ui.available_width();
            for f in &files[range] {
                let (rect, resp) = ui.allocate_exact_size(vec2(w, 26.0), Sense::click());
                let is_sel = selected.as_deref() == Some(f.name.as_str());
                if is_sel {
                    ui.painter().rect_filled(rect, 4.0, Color32::from_rgb(0x2c, 0x1d, 0x55));
                } else if resp.hovered() {
                    ui.painter().rect_filled(rect, 4.0, PANEL2);
                }
                let name_col = if f.is_dir { Color32::from_rgb(0xd9, 0xcc, 0xff) } else { Color32::from_gray(225) };
                let name = if f.is_link { format!("{} ↪", f.name) } else { f.name.clone() };
                let icon_col = if f.name == ".." {
                    MUTED
                } else if f.is_dir {
                    Color32::from_rgb(0xff, 0xc8, 0x6b)
                } else {
                    Color32::from_gray(150)
                };
                let clip = ui.painter().with_clip_rect(egui::Rect::from_min_size(rect.min, vec2(cols[0] - 6.0, 26.0)));
                clip.text(rect.left_center() + vec2(9.0, 0.0), Align2::LEFT_CENTER, file_icon(f), FontId::proportional(14.0), icon_col);
                clip.text(rect.left_center() + vec2(32.0, 0.0), Align2::LEFT_CENTER, name, FontId::proportional(13.5), name_col);
                if !f.is_dir {
                    ui.painter().text(
                        pos2(rect.left() + cols[0] + 6.0, rect.center().y),
                        Align2::LEFT_CENTER,
                        human_size(f.size),
                        FontId::proportional(12.0),
                        MUTED,
                    );
                }
                ui.painter().text(
                    pos2(rect.left() + cols[0] + cols[1] + 6.0, rect.center().y),
                    Align2::LEFT_CENTER,
                    fmt_time(f.modified),
                    FontId::proportional(12.0),
                    MUTED,
                );
                let resp = if f.perms.is_empty() { resp } else { resp.on_hover_text(format!("{}\n{}  ·  {}", f.name, f.perms, human_size(f.size))) };
                if resp.clicked() {
                    new_sel = Some(f.name.clone());
                }
                if resp.double_clicked() {
                    act = Some(A::Open((*f).clone()));
                }
                if f.name != ".." {
                    let full = join(&path, &f.name);
                    resp.context_menu(|ui| {
                        if f.is_dir {
                            if ui.button("📂  Öffnen").clicked() {
                                act = Some(A::Open((*f).clone()));
                            }
                            if ui.button("⌨  Im Terminal öffnen (cd)").clicked() {
                                act = Some(A::Cd(full.clone()));
                            }
                        } else {
                            if ui.button("📝  Bearbeiten").clicked() {
                                act = Some(A::Edit((*f).clone()));
                            }
                            if ui.button("⬇  Herunterladen…").clicked() {
                                act = Some(A::Download((*f).clone()));
                            }
                        }
                        ui.separator();
                        if ui.button("✏  Umbenennen…").clicked() {
                            act = Some(A::Rename(f.name.clone()));
                        }
                        if ui.button("🔐  Rechte ändern…").clicked() {
                            act = Some(A::Chmod((*f).clone()));
                        }
                        if ui.button("📋  Pfad kopieren").clicked() {
                            act = Some(A::CopyPath(full.clone()));
                        }
                        ui.separator();
                        if ui.button(RichText::new("🗑  Löschen…").color(RED)).clicked() {
                            act = Some(A::Delete((*f).clone()));
                        }
                    });
                }
            }
        });
        if loading {
            ui.label(RichText::new("Lade…").color(MUTED).size(12.0));
        } else {
            ui.add(egui::Label::new(RichText::new(format!("{count} Einträge · Doppelklick öffnet · Rechtsklick für mehr · Dateien hierher ziehen = Upload")).color(Color32::from_gray(105)).size(11.0)).truncate());
        }
        if let Some(n) = new_sel {
            t.sftp.selected = Some(n);
        }

        self.transfers_ui(ui);
        self.apply_files_action(id, act, &path);
    }

    fn next_transfer_id(&mut self) -> u64 {
        let id = self.transfers.iter().map(|t| t.id).max().unwrap_or(0) + 1;
        id
    }

    fn transfers_height(&self) -> f32 {
        if self.transfers.is_empty() {
            20.0
        } else {
            32.0 + self.transfers.len().min(4) as f32 * 40.0
        }
    }

    fn transfers_ui(&mut self, ui: &mut Ui) {
        // Fertige Übertragungen nach kurzer Zeit ausblenden
        self.transfers.retain(|t| !matches!(t.state, TransferState::Done(at) if at.elapsed().as_secs() > 4));
        if self.transfers.is_empty() {
            return;
        }
        ui.separator();
        ui.label(RichText::new("Übertragungen").size(12.0).color(MUTED).strong());
        let mut remove = None;
        for t in self.transfers.iter().rev().take(4) {
            let done = t.done.load(Ordering::Relaxed);
            let frac = if t.total > 0 { (done as f32 / t.total as f32).min(1.0) } else { 1.0 };
            ui.horizontal(|ui| {
                let arrow = if t.upload { "⬆" } else { "⬇" };
                ui.label(RichText::new(arrow).color(LAVENDER));
                ui.add(egui::Label::new(RichText::new(&t.label).size(12.5)).truncate());
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    match &t.state {
                        TransferState::Running => {
                            ui.label(RichText::new(format!("{} / {}", human_size(done), human_size(t.total))).size(11.5).color(MUTED));
                        }
                        TransferState::Done(_) => {
                            ui.label(RichText::new("✔ fertig").size(11.5).color(GREEN));
                        }
                        TransferState::Failed(e) => {
                            if ui.small_button("✖").clicked() {
                                remove = Some(t.id);
                            }
                            ui.label(RichText::new("Fehler").size(11.5).color(RED)).on_hover_text(e);
                        }
                    }
                });
            });
            let color = match t.state {
                TransferState::Failed(_) => RED,
                TransferState::Done(_) => GREEN,
                TransferState::Running => ACCENT_HI,
            };
            ui.add(egui::ProgressBar::new(frac).desired_height(5.0).fill(color));
        }
        if let Some(id) = remove {
            self.transfers.retain(|t| t.id != id);
        }
    }

    fn apply_files_action(&mut self, id: u64, act: Option<A>, path: &str) {
        let Some(act) = act else { return };
        match act {
            A::Close => {
                self.show_files = false;
                self.focus_term = true;
            }
            A::Reload => self.reload_dir(id),
            A::Go(p) => self.load_dir(id, p, true),
            A::Up => self.load_dir(id, parent(path), true),
            A::Home => {
                if let Some(t) = self.tab(id) {
                    t.sftp.history.push(path.to_string());
                }
                self.sftp_init(id);
            }
            A::Back => {
                let prev = self.tab(id).and_then(|t| t.sftp.history.pop());
                if let Some(p) = prev {
                    self.load_dir(id, p, false);
                }
            }
            A::Sort(by) => {
                if let Some(t) = self.tab(id) {
                    if t.sftp.sort == by {
                        t.sftp.asc = !t.sftp.asc;
                    } else {
                        t.sftp.sort = by;
                        t.sftp.asc = true;
                    }
                }
            }
            A::Open(f) => {
                if f.is_dir {
                    let new = if f.name == ".." { parent(path) } else { join(path, &f.name) };
                    self.load_dir(id, new, true);
                } else {
                    self.edit_file(id, path, &f);
                }
            }
            A::Edit(f) => self.edit_file(id, path, &f),
            A::Cd(dir) => {
                if let Some(t) = self.tab(id) {
                    let quoted = format!("cd '{}'\n", dir.replace('\'', "'\\''"));
                    t.send(nebula_core::Cmd::Data(quoted.into_bytes()));
                }
                self.focus_term = true;
            }
            A::CopyPath(p) => {
                self.ctx.copy_text(p);
                self.notify("Pfad kopiert");
            }
            A::Download(f) => {
                if let Some(local) = rfd::FileDialog::new().set_title("Speicherort auswählen").set_file_name(&f.name).save_file() {
                    self.start_download(id, join(path, &f.name), f.size, local);
                }
            }
            A::Upload => {
                if let Some(files) = rfd::FileDialog::new().set_title("Dateien hochladen").pick_files() {
                    for p in files {
                        self.start_upload(id, p, path.to_string());
                    }
                }
            }
            A::UploadDir => {
                if let Some(dir) = rfd::FileDialog::new().set_title("Ordner hochladen").pick_folder() {
                    self.start_upload(id, dir, path.to_string());
                }
            }
            A::Delete(f) => {
                let q = if f.is_dir {
                    format!("Den Ordner „{}“ mit ALLEM Inhalt löschen?\n\nDas kann nicht rückgängig gemacht werden.", f.name)
                } else {
                    format!("Die Datei „{}“ wirklich löschen?", f.name)
                };
                if confirm("Löschen", &q, MessageLevel::Warning) {
                    let p = join(path, &f.name);
                    self.sftp_op(id, "Löschen", move |s| async move { ssh::delete_recursive(&s, &p).await });
                }
            }
            A::Rename(name) => {
                self.dialog = Some(Dialog::Rename { tab: id, old: name.clone(), new: name });
            }
            A::Chmod(f) => {
                let mode = mode_from_perms(&f.perms);
                self.dialog = Some(Dialog::Chmod { tab: id, path: join(path, &f.name), name: f.name.clone(), mode });
            }
            A::MkDir => self.dialog = Some(Dialog::MkDir { tab: id, name: String::new() }),
            A::NewFile => self.dialog = Some(Dialog::NewFile { tab: id, name: String::new() }),
        }
    }

    fn edit_file(&mut self, id: u64, dir: &str, f: &FileInfo) {
        if f.size > 5 * 1024 * 1024 {
            alert("Editor", "Die Datei ist größer als 5 MB. Bitte herunterladen und lokal bearbeiten.");
            return;
        }
        self.open_editor(id, join(dir, &f.name), false);
    }
}

/// "-rwxr-xr--" -> "754"
fn mode_from_perms(p: &str) -> String {
    if p.len() < 10 {
        return "644".into();
    }
    let b = p.as_bytes();
    let mut out = String::new();
    for g in 0..3 {
        let o = 1 + g * 3;
        let v = (b[o] == b'r') as u8 * 4 + (b[o + 1] == b'w') as u8 * 2 + (b[o + 2] == b'x') as u8;
        out.push((b'0' + v) as char);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn perms_to_mode() {
        assert_eq!(mode_from_perms("-rwxr-xr--"), "754");
        assert_eq!(mode_from_perms("drwx------"), "700");
        assert_eq!(parent("/etc/ssh"), "/etc");
        assert_eq!(parent("/etc"), "/");
    }
}
