//! SFTP-Datei-Explorer: Liste mit Spalten, Navigation, Drag & Drop und Übertragungs-Warteschlange
//! (pausieren, fortsetzen, abbrechen, wiederholen).

use crate::app::*;
use crate::sidebar::icon_button;
use eframe::egui::{self, pos2, vec2, Align, Align2, Color32, FontId, Layout, RichText, Sense, Stroke, TextEdit, Ui};
use nebula_core::ssh::{self, join, Ctl, FileInfo};
use nebula_core::{t, tf};
use rfd::MessageLevel;
use std::path::PathBuf;
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Instant;

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
    Sync,
    Sort(SortBy),
    Close,
}

/// Was übertragen wird
#[derive(Clone)]
pub enum Job {
    /// Datei oder Ordner in einen Server-Ordner hochladen
    Upload { local: PathBuf, remote_dir: String },
    /// Eine Datei an genau diesen Server-Pfad (Synchronisieren)
    UploadTo { local: PathBuf, remote: String },
    /// Datei herunterladen
    Download { remote: String, local: PathBuf },
    /// Ganzen Ordner herunterladen (nach local_parent/<name>)
    DownloadDir { remote: String, local_parent: PathBuf },
}

impl Job {
    pub fn is_upload(&self) -> bool {
        matches!(self, Job::Upload { .. } | Job::UploadTo { .. })
    }
    fn can_resume(&self) -> bool {
        matches!(self, Job::UploadTo { .. } | Job::Download { .. }) || matches!(self, Job::Upload { local, .. } if local.is_file())
    }
}

#[derive(Clone, PartialEq)]
pub enum TransferState {
    Queued,
    Running,
    Done(Instant),
    Failed(String),
    Cancelled,
}

pub struct Transfer {
    pub id: u64,
    pub tab: u64,
    pub label: String,
    pub total: u64,
    pub ctl: Arc<Ctl>,
    pub state: TransferState,
    pub job: Job,
    pub resume: bool,
    pub task: Option<tokio::task::AbortHandle>,
}

impl Transfer {
    pub fn is_active(&self) -> bool {
        matches!(self.state, TransferState::Queued | TransferState::Running)
    }
    fn paused(&self) -> bool {
        self.ctl.paused.load(Ordering::Relaxed)
    }
}

fn parent(path: &str) -> String {
    let mut parts: Vec<&str> = path.split('/').filter(|p| !p.is_empty()).collect();
    parts.pop();
    format!("/{}", parts.join("/"))
}

fn fmt_time(ts: Option<u32>) -> String {
    use chrono::TimeZone;
    match ts.and_then(|t| chrono::Local.timestamp_opt(t as i64, 0).single()) {
        Some(d) => d.format(if nebula_core::i18n::english() { "%Y-%m-%d %H:%M" } else { "%d.%m.%Y %H:%M" }).to_string(),
        None => String::new(),
    }
}

fn file_icon(f: &FileInfo) -> Icon {
    if f.name == ".." {
        return Icon::ArrowUp;
    }
    if f.is_dir {
        return Icon::Folder;
    }
    let ext = f.name.rsplit('.').next().unwrap_or("").to_lowercase();
    match ext.as_str() {
        "sh" | "bash" | "py" | "pl" | "rb" | "js" | "ts" | "rs" | "go" | "c" | "h" | "cpp" | "php" => Icon::Code,
        "conf" | "cfg" | "ini" | "yml" | "yaml" | "toml" | "json" | "xml" | "env" => Icon::Settings,
        "zip" | "gz" | "tgz" | "tar" | "xz" | "bz2" | "7z" | "deb" | "rpm" => Icon::Archive,
        "png" | "jpg" | "jpeg" | "gif" | "svg" | "webp" | "ico" => Icon::Image,
        "key" | "pem" | "crt" | "pub" => Icon::Key,
        _ => Icon::File,
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
                t.sftp.path_edit = t.sftp.path.clone();
                t.sftp.error = Some(e);
            }
        }
    }

    pub fn sftp_op<F, Fut>(&mut self, id: u64, what: String, f: F)
    where
        F: FnOnce(Arc<ssh::SshSession>) -> Fut + Send + 'static,
        Fut: std::future::Future<Output = Result<(), String>> + Send,
    {
        let tx = self.tx.clone();
        let ctx = self.ctx.clone();
        let Some(s) = self.tab(id).and_then(|t| t.ssh.clone()) else {
            alert(t!("Dateien", "Files"), t!("Keine aktive SSH-Verbindung.", "No active SSH connection."));
            return;
        };
        self.rt.spawn(async move {
            let res = f(s).await;
            let _ = tx.send(Msg::SftpDone(id, res, what));
            ctx.request_repaint();
        });
    }

    // ------------------------------------------------------------------
    // Übertragungen (Warteschlange)
    // ------------------------------------------------------------------

    /// Stellt eine Übertragung in die Warteschlange.
    pub fn queue(&mut self, tab: u64, job: Job) {
        let (label, total) = match &job {
            Job::Upload { local, .. } => (
                local.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default(),
                ssh::local_size(&local.to_string_lossy()),
            ),
            Job::UploadTo { local, remote } => (remote.rsplit('/').next().unwrap_or("").to_string(), ssh::local_size(&local.to_string_lossy())),
            Job::Download { remote, .. } => (remote.rsplit('/').next().unwrap_or("").to_string(), 0),
            Job::DownloadDir { remote, .. } => (format!("{}/", remote.trim_end_matches('/').rsplit('/').next().unwrap_or("")), 0),
        };
        let id = self.transfers.iter().map(|t| t.id).max().unwrap_or(0) + 1;
        self.transfers.push(Transfer {
            id,
            tab,
            label,
            total,
            ctl: Arc::new(Ctl::default()),
            state: TransferState::Queued,
            job,
            resume: false,
            task: None,
        });
    }

    /// Startet wartende Übertragungen (begrenzt auf `parallel_transfers`).
    pub fn pump_transfers(&mut self) {
        let max = self.prefs.parallel_transfers.clamp(1, 8);
        loop {
            let running = self.transfers.iter().filter(|t| t.state == TransferState::Running).count();
            if running >= max {
                return;
            }
            let Some(i) = self.transfers.iter().position(|t| t.state == TransferState::Queued) else { return };
            let tab = self.transfers[i].tab;
            let Some(s) = self.tabs.iter().find(|t| t.id == tab).and_then(|t| t.ssh.clone()) else {
                self.transfers[i].state = TransferState::Failed(t!("Keine SSH-Verbindung", "No SSH connection").into());
                continue;
            };
            let tr = &mut self.transfers[i];
            tr.state = TransferState::Running;
            let (tid, job, ctl, resume) = (tr.id, tr.job.clone(), tr.ctl.clone(), tr.resume);
            let tx = self.tx.clone();
            let ctx = self.ctx.clone();
            let handle = self.rt.spawn(async move {
                let p = Some(ctl);
                let res = match job {
                    Job::Upload { local, remote_dir } => {
                        if resume && local.is_file() {
                            let name = local.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
                            ssh::upload_to(&s, &local.to_string_lossy(), &join(&remote_dir, &name), p, true).await
                        } else {
                            ssh::upload_with(&s, &remote_dir, &local.to_string_lossy(), p).await
                        }
                    }
                    Job::UploadTo { local, remote } => {
                        let _ = ssh::mkdir_all(&s, &parent(&remote)).await;
                        ssh::upload_to(&s, &local.to_string_lossy(), &remote, p, resume).await
                    }
                    Job::Download { remote, local } => {
                        if let Some(d) = local.parent() {
                            let _ = std::fs::create_dir_all(d);
                        }
                        ssh::download_with(&s, &remote, &local.to_string_lossy(), p, resume).await
                    }
                    Job::DownloadDir { remote, local_parent } => {
                        let size = ssh::remote_size(&s, &remote).await;
                        let _ = tx.send(Msg::TransferSize(tid, size));
                        ssh::download_tree(&s, &remote, &local_parent, p).await
                    }
                };
                let _ = tx.send(Msg::TransferDone(tid, tab, res));
                ctx.request_repaint();
            });
            self.transfers[i].task = Some(handle.abort_handle());
        }
    }

    pub fn on_transfer_done(&mut self, tid: u64, tab: u64, res: Result<(), String>) {
        let ok = res.is_ok();
        if let Some(t) = self.transfers.iter_mut().find(|t| t.id == tid) {
            t.task = None;
            t.state = match res {
                Ok(()) => {
                    if t.total == 0 {
                        t.total = t.ctl.done.load(Ordering::Relaxed);
                    }
                    TransferState::Done(Instant::now())
                }
                Err(e) if e == ssh::CANCELLED => TransferState::Cancelled,
                Err(e) => TransferState::Failed(e),
            };
        }
        if ok && !self.transfers.iter().any(|t| t.tab == tab && t.is_active()) {
            self.reload_dir(tab);
        }
    }

    fn transfer_action(&mut self, id: u64, action: &str) {
        let Some(t) = self.transfers.iter_mut().find(|t| t.id == id) else { return };
        match action {
            "pause" => {
                let p = !t.paused();
                t.ctl.paused.store(p, Ordering::Relaxed);
            }
            "cancel" => {
                t.ctl.cancel.store(true, Ordering::Relaxed);
                t.ctl.paused.store(false, Ordering::Relaxed);
                if t.state == TransferState::Queued {
                    t.state = TransferState::Cancelled;
                }
            }
            "retry" => {
                // Fortsetzen, wo es aufgehört hat (sofern möglich)
                t.resume = t.job.can_resume();
                t.ctl = Arc::new(Ctl::default());
                t.state = TransferState::Queued;
            }
            "remove" => {
                let id = t.id;
                self.transfers.retain(|x| x.id != id);
            }
            _ => {}
        }
    }

    pub fn open_editor(&mut self, id: u64, path: String, new_file: bool) {
        let tx = self.tx.clone();
        let ctx = self.ctx.clone();
        let Some(s) = self.tab(id).and_then(|t| t.ssh.clone()) else { return };
        self.dialog = Some(Dialog::Editor { tab: id, path: path.clone(), content: String::new(), original: String::new(), loading: !new_file, saving: false });
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
        let target = self.active.filter(|id| self.tabs.iter().any(|t| t.id == *id && t.ssh.is_some()));
        if hovering {
            let rect = ui.ctx().content_rect();
            let painter = ui.ctx().layer_painter(egui::LayerId::new(egui::Order::Foreground, egui::Id::new("dnd")));
            painter.rect_filled(rect, 0.0, Color32::from_rgba_unmultiplied(20, 10, 40, 200));
            painter.rect_stroke(rect.shrink(16.0), 16.0, Stroke::new(3.0, if target.is_some() { ACCENT_HI } else { RED }), egui::StrokeKind::Inside);
            let text = match target {
                Some(id) => {
                    let dir = self.tabs.iter().find(|t| t.id == id).map(|t| t.sftp.path.clone()).unwrap_or_default();
                    let dir = if dir.is_empty() { t!("Home-Verzeichnis", "home directory").to_string() } else { dir };
                    tf!("⬆  Loslassen zum Hochladen nach\n{dir}", "⬆  Drop to upload to\n{dir}")
                }
                None => t!("Zum Hochladen zuerst eine SSH-Verbindung öffnen", "Open an SSH connection first to upload").into(),
            };
            painter.text(rect.center(), Align2::CENTER_CENTER, text, FontId::proportional(22.0), Color32::WHITE);
        }
        if dropped.is_empty() {
            return;
        }
        let Some(id) = target else {
            self.notify_err(t!("Kein SSH-Tab aktiv – Upload nicht möglich", "No SSH tab active – cannot upload"));
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
                self.queue(id, Job::Upload { local: p.to_path_buf(), remote_dir: dir.clone() });
            }
        }
    }

    pub fn files_panel(&mut self, ui: &mut Ui, id: u64) {
        let mut act: Option<A> = None;
        let transfers_h = self.transfers_height();
        let Some(t) = self.tabs.iter_mut().find(|t| t.id == id) else { return };
        let connected = t.ssh.is_some();

        ui.add_space(4.0);
        ui.horizontal(|ui| {
            icons::show(ui, Icon::Folder, 17.0, AMBER);
            ui.label(RichText::new(t!("Dateien", "Files")).strong().size(15.0).color(Color32::WHITE));
            ui.label(RichText::new(t.host.subtitle()).size(12.0).color(MUTED));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if icons::button(ui, Icon::Close, t!("Schließen", "Close")).clicked() {
                    act = Some(A::Close);
                }
            });
        });
        ui.add_space(4.0);
        if !connected {
            ui.add_space(20.0);
            ui.vertical_centered(|ui| {
                ui.label(RichText::new(t!("Keine Verbindung – Dateien sind erst nach dem Verbinden verfügbar.", "Not connected – files are available after connecting.")).color(MUTED));
            });
            if let Some(A::Close) = act {
                self.show_files = false;
            }
            return;
        }

        // Navigation
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            if ui
                .add_enabled_ui(!t.sftp.history.is_empty(), |ui| icon_button(ui, Icon::Back, ""))
                .inner
                .on_hover_text(t!("Zurück", "Back"))
                .clicked()
            {
                act = Some(A::Back);
            }
            if icon_button(ui, Icon::ArrowUp, t!("Übergeordneter Ordner", "Parent folder")).clicked() {
                act = Some(A::Up);
            }
            if icon_button(ui, Icon::Home, t!("Home-Verzeichnis", "Home directory")).clicked() {
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
                                let crumb = |ui: &mut Ui, text: &str| ui.add(egui::Button::new(RichText::new(text).size(13.0).color(Color32::from_gray(225))).frame_when_inactive(false));
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
            if icon_button(ui, Icon::Pencil, t!("Pfad eingeben", "Enter path")).clicked() {
                t.sftp.editing_path = !t.sftp.editing_path;
                t.sftp.path_edit = t.sftp.path.clone();
            }
            if icon_button(ui, Icon::Refresh, t!("Aktualisieren", "Refresh")).clicked() {
                act = Some(A::Reload);
            }
        });
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            if icons::menu(ui, Icon::Export, t!("Hochladen", "Upload")).on_hover_text(t!("Dateien hochladen – oder einfach ins Fenster ziehen", "Upload files – or just drag them into the window")).clicked() {
                act = Some(A::Upload);
            }
            if icons::menu(ui, Icon::Export, t!("Ordner", "Folder")).on_hover_text(t!("Ganzen Ordner hochladen", "Upload a whole folder")).clicked() {
                act = Some(A::UploadDir);
            }
            if icons::menu(ui, Icon::Folder, t!("Neuer Ordner", "New folder")).on_hover_text(t!("Neuer Ordner", "New folder")).clicked() {
                act = Some(A::MkDir);
            }
            if icons::menu(ui, Icon::File, t!("Neue Datei", "New file")).clicked() {
                act = Some(A::NewFile);
            }
            if icons::menu(ui, Icon::Tunnel, t!("Vergleichen", "Compare")).on_hover_text(t!("Diesen Ordner mit einem lokalen Ordner vergleichen und synchronisieren", "Compare and sync this folder with a local folder")).clicked() {
                act = Some(A::Sync);
            }
        });
        ui.horizontal(|ui| {
            ui.checkbox(&mut t.sftp.show_hidden, t!("Versteckte", "Hidden")).on_hover_text(t!("Dateien mit . am Anfang anzeigen", "Show files starting with ."));
            ui.add(TextEdit::singleline(&mut t.sftp.filter).hint_text(t!("Filter", "Filter")).desired_width(f32::INFINITY));
        });
        if let Some(e) = &t.sftp.error {
            ui.label(RichText::new(format!("⚠ {e}")).color(RED).size(12.0));
        }
        ui.add_space(4.0);

        // Spaltenköpfe
        let w = ui.available_width();
        let cols = [w * 0.50, w * 0.16, w * 0.34];
        let header = ui.allocate_exact_size(vec2(w, 22.0), Sense::hover()).0;
        let heads = [(t!("Name", "Name"), SortBy::Name), (t!("Größe", "Size"), SortBy::Size), (t!("Geändert", "Modified"), SortBy::Date)];
        let mut x = header.left();
        for (i, (label, by)) in heads.iter().enumerate() {
            let r = egui::Rect::from_min_size(pos2(x, header.top()), vec2(cols[i], 22.0));
            let resp = ui.interact(r, ui.scope_id().with(("hdr", i)), Sense::click());
            let arrow = if t.sftp.sort == *by { if t.sftp.asc { " ▲" } else { " ▼" } } else { "" };
            ui.painter().text(r.left_center() + vec2(6.0, 0.0), Align2::LEFT_CENTER, format!("{label}{arrow}"), FontId::proportional(12.0), if resp.hovered() { Color32::WHITE } else { MUTED });
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
                let icon_rect = egui::Rect::from_center_size(rect.left_center() + vec2(16.0, 0.0), vec2(15.0, 15.0));
                icons::paint(ui, icon_rect, file_icon(f), icon_col);
                clip.text(rect.left_center() + vec2(32.0, 0.0), Align2::LEFT_CENTER, name, FontId::proportional(13.5), name_col);
                if !f.is_dir {
                    ui.painter().text(pos2(rect.left() + cols[0] + 6.0, rect.center().y), Align2::LEFT_CENTER, human_size(f.size), FontId::proportional(12.0), MUTED);
                }
                ui.painter().text(pos2(rect.left() + cols[0] + cols[1] + 6.0, rect.center().y), Align2::LEFT_CENTER, fmt_time(f.modified), FontId::proportional(12.0), MUTED);
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
                            if icons::menu(ui, Icon::Folder, t!("Öffnen", "Open")).clicked() {
                                act = Some(A::Open((*f).clone()));
                            }
                            if icons::menu(ui, Icon::Terminal, t!("Im Terminal öffnen (cd)", "Open in terminal (cd)")).clicked() {
                                act = Some(A::Cd(full.clone()));
                            }
                            if icons::menu(ui, Icon::Download, t!("Ordner herunterladen…", "Download folder…")).clicked() {
                                act = Some(A::Download((*f).clone()));
                            }
                        } else {
                            if icons::menu(ui, Icon::Pencil, t!("Bearbeiten", "Edit")).clicked() {
                                act = Some(A::Edit((*f).clone()));
                            }
                            if icons::menu(ui, Icon::Download, t!("Herunterladen…", "Download…")).clicked() {
                                act = Some(A::Download((*f).clone()));
                            }
                        }
                        ui.separator();
                        if icons::menu(ui, Icon::Pencil, t!("Umbenennen…", "Rename…")).clicked() {
                            act = Some(A::Rename(f.name.clone()));
                        }
                        if icons::menu(ui, Icon::Lock, t!("Rechte ändern…", "Change permissions…")).clicked() {
                            act = Some(A::Chmod((*f).clone()));
                        }
                        if icons::menu(ui, Icon::Copy, t!("Pfad kopieren", "Copy path")).clicked() {
                            act = Some(A::CopyPath(full.clone()));
                        }
                        ui.separator();
                        if icons::menu_colored(ui, Icon::Trash, t!("Löschen…", "Delete…"), RED).clicked() {
                            act = Some(A::Delete((*f).clone()));
                        }
                    });
                }
            }
        });
        if loading {
            ui.label(RichText::new(t!("Lade…", "Loading…")).color(MUTED).size(12.0));
        } else {
            ui.add(
                egui::Label::new(
                    RichText::new(tf!(
                        "{count} Einträge · Doppelklick öffnet · Rechtsklick für mehr · Dateien hierher ziehen = Upload",
                        "{count} entries · double-click opens · right-click for more · drop files here = upload"
                    ))
                    .color(Color32::from_gray(105))
                    .size(11.0),
                )
                .truncate(),
            );
        }
        if let Some(n) = new_sel {
            t.sftp.selected = Some(n);
        }

        self.transfers_ui(ui);
        self.apply_files_action(id, act, &path);
    }

    fn transfers_height(&self) -> f32 {
        let n = self.visible_transfers().len();
        if n == 0 {
            20.0
        } else {
            36.0 + n.min(5) as f32 * 42.0
        }
    }

    fn visible_transfers(&self) -> Vec<usize> {
        (0..self.transfers.len())
            .filter(|&i| !matches!(self.transfers[i].state, TransferState::Done(at) if at.elapsed().as_secs() > 5))
            .collect()
    }

    fn transfers_ui(&mut self, ui: &mut Ui) {
        // Fertige Übertragungen nach kurzer Zeit ausblenden
        self.transfers.retain(|t| !matches!(t.state, TransferState::Done(at) if at.elapsed().as_secs() > 5));
        if self.transfers.is_empty() {
            return;
        }
        ui.separator();
        let queued = self.transfers.iter().filter(|t| t.state == TransferState::Queued).count();
        let mut action: Option<(u64, &str)> = None;
        let mut clear = false;
        ui.horizontal(|ui| {
            ui.label(RichText::new(t!("Übertragungen", "Transfers")).size(12.0).color(MUTED).strong());
            if queued > 0 {
                ui.label(RichText::new(tf!("{queued} wartend", "{queued} queued")).size(11.5).color(MUTED));
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                if self.transfers.iter().any(|t| !t.is_active()) && ui.small_button(t!("Liste leeren", "Clear list")).clicked() {
                    clear = true;
                }
            });
        });
        egui::ScrollArea::vertical().id_salt("transfers").max_height(5.0 * 42.0).show(ui, |ui| {
            for t in self.transfers.iter().rev() {
                let done = t.ctl.done.load(Ordering::Relaxed);
                let frac = if t.total > 0 { (done as f32 / t.total as f32).min(1.0) } else if matches!(t.state, TransferState::Done(_)) { 1.0 } else { 0.0 };
                let paused = t.paused();
                ui.horizontal(|ui| {
                    ui.label(RichText::new(if t.job.is_upload() { "⬆" } else { "⬇" }).color(LAVENDER));
                    ui.add(egui::Label::new(RichText::new(&t.label).size(12.5)).truncate());
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        ui.spacing_mut().item_spacing.x = 3.0;
                        match &t.state {
                            TransferState::Running | TransferState::Queued => {
                                if ui.small_button("✖").on_hover_text(t!("Abbrechen", "Cancel")).clicked() {
                                    action = Some((t.id, "cancel"));
                                }
                                if t.state == TransferState::Running
                                    && ui.small_button(if paused { "▶" } else { "⏸" }).on_hover_text(if paused { t!("Fortsetzen", "Resume") } else { t!("Pausieren", "Pause") }).clicked()
                                {
                                    action = Some((t.id, "pause"));
                                }
                                let txt = if t.state == TransferState::Queued {
                                    t!("wartet", "queued").to_string()
                                } else if paused {
                                    t!("pausiert", "paused").to_string()
                                } else if t.total > 0 {
                                    format!("{} / {}", human_size(done), human_size(t.total))
                                } else {
                                    human_size(done)
                                };
                                ui.label(RichText::new(txt).size(11.5).color(MUTED));
                            }
                            TransferState::Done(_) => {
                                ui.label(RichText::new(t!("✔ fertig", "✔ done")).size(11.5).color(GREEN));
                            }
                            TransferState::Cancelled | TransferState::Failed(_) => {
                                if ui.small_button("🗑").on_hover_text(t!("Entfernen", "Remove")).clicked() {
                                    action = Some((t.id, "remove"));
                                }
                                let tip = if t.job.can_resume() { t!("Fortsetzen ab der abgebrochenen Stelle", "Resume where it stopped") } else { t!("Erneut versuchen", "Retry") };
                                if ui.small_button("⟳").on_hover_text(tip).clicked() {
                                    action = Some((t.id, "retry"));
                                }
                                match &t.state {
                                    TransferState::Failed(e) => {
                                        ui.label(RichText::new(t!("Fehler", "Error")).size(11.5).color(RED)).on_hover_text(e);
                                    }
                                    _ => {
                                        ui.label(RichText::new(t!("abgebrochen", "cancelled")).size(11.5).color(AMBER));
                                    }
                                }
                            }
                        }
                    });
                });
                let color = match t.state {
                    TransferState::Failed(_) => RED,
                    TransferState::Cancelled => AMBER,
                    TransferState::Done(_) => GREEN,
                    _ if paused => AMBER,
                    _ => ACCENT_HI,
                };
                ui.add(egui::ProgressBar::new(frac).desired_height(5.0).fill(color));
            }
        });
        if let Some((id, a)) = action {
            self.transfer_action(id, a);
        }
        if clear {
            self.transfers.retain(|t| t.is_active());
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
                self.notify(t!("Pfad kopiert", "Path copied"));
            }
            A::Download(f) => {
                if f.is_dir {
                    if let Some(dir) = rfd::FileDialog::new().set_title(t!("Zielordner wählen", "Choose target folder")).pick_folder() {
                        self.queue(id, Job::DownloadDir { remote: join(path, &f.name), local_parent: dir });
                    }
                } else if let Some(local) = rfd::FileDialog::new().set_title(t!("Speicherort auswählen", "Choose where to save")).set_file_name(&f.name).save_file() {
                    let i = self.transfers.len();
                    self.queue(id, Job::Download { remote: join(path, &f.name), local });
                    self.transfers[i].total = f.size;
                }
            }
            A::Upload => {
                if let Some(files) = rfd::FileDialog::new().set_title(t!("Dateien hochladen", "Upload files")).pick_files() {
                    for p in files {
                        self.queue(id, Job::Upload { local: p, remote_dir: path.to_string() });
                    }
                }
            }
            A::UploadDir => {
                if let Some(dir) = rfd::FileDialog::new().set_title(t!("Ordner hochladen", "Upload folder")).pick_folder() {
                    self.queue(id, Job::Upload { local: dir, remote_dir: path.to_string() });
                }
            }
            A::Sync => self.open_sync(id, path.to_string()),
            A::Delete(f) => {
                let q = if f.is_dir {
                    tf!(
                        "Den Ordner „{}“ mit ALLEM Inhalt löschen?\n\nDas kann nicht rückgängig gemacht werden.",
                        "Delete the folder \"{}\" with ALL its contents?\n\nThis cannot be undone.",
                        f.name
                    )
                } else {
                    tf!("Die Datei „{}“ wirklich löschen?", "Really delete the file \"{}\"?", f.name)
                };
                if confirm(t!("Löschen", "Delete"), &q, MessageLevel::Warning) {
                    let p = join(path, &f.name);
                    self.sftp_op(id, t!("Löschen", "Delete").into(), move |s| async move { ssh::delete_recursive(&s, &p).await });
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
            alert("Editor", t!("Die Datei ist größer als 5 MB. Bitte herunterladen und lokal bearbeiten.", "The file is larger than 5 MB. Please download and edit it locally."));
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
