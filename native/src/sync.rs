//! Ordner vergleichen und synchronisieren (lokal ⇄ Server).

use crate::app::*;
use crate::files::Job;
use crate::sidebar::{label, segmented};
use eframe::egui::{self, vec2, Align, Color32, Layout, RichText, ScrollArea, Ui};
use nebula_core::ssh::{self, join, TreeEntry};
use nebula_core::{t, tf};
use std::collections::BTreeMap;
use std::path::PathBuf;

/// Unterschied zwischen lokaler und Server-Datei
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Diff {
    Same,
    OnlyLocal,
    OnlyRemote,
    LocalNewer,
    RemoteNewer,
    /// Gleiche Zeit, andere Größe
    Differs,
}

#[derive(Clone, Debug)]
pub struct Row {
    pub rel: String,
    /// (Größe, Änderungszeit)
    pub local: Option<(u64, u64)>,
    pub remote: Option<(u64, u64)>,
    pub diff: Diff,
    pub selected: bool,
}

/// Toleranz für Änderungszeiten (FAT/Samba runden auf 2 s)
const SLACK: u64 = 2;

pub fn compare(local: &[TreeEntry], remote: &[TreeEntry]) -> Vec<Row> {
    let mut map: BTreeMap<String, Row> = BTreeMap::new();
    for e in local.iter().filter(|e| !e.is_dir) {
        map.insert(e.rel.clone(), Row { rel: e.rel.clone(), local: Some((e.size, e.mtime)), remote: None, diff: Diff::OnlyLocal, selected: false });
    }
    for e in remote.iter().filter(|e| !e.is_dir) {
        let r = map.entry(e.rel.clone()).or_insert(Row { rel: e.rel.clone(), local: None, remote: None, diff: Diff::OnlyRemote, selected: false });
        r.remote = Some((e.size, e.mtime));
    }
    for r in map.values_mut() {
        r.diff = match (r.local, r.remote) {
            (Some(_), None) => Diff::OnlyLocal,
            (None, Some(_)) => Diff::OnlyRemote,
            (Some((ls, lt)), Some((rs, rt))) => {
                if lt.abs_diff(rt) <= SLACK {
                    if ls == rs {
                        Diff::Same
                    } else {
                        Diff::Differs
                    }
                } else if lt > rt {
                    Diff::LocalNewer
                } else {
                    Diff::RemoteNewer
                }
            }
            (None, None) => Diff::Same,
        };
    }
    map.into_values().collect()
}

/// Welche Zeilen in Richtung `upload` standardmäßig übertragen werden
pub fn default_selected(d: Diff, upload: bool) -> bool {
    match d {
        Diff::OnlyLocal | Diff::LocalNewer => upload,
        Diff::OnlyRemote | Diff::RemoteNewer => !upload,
        Diff::Differs => true,
        Diff::Same => false,
    }
}

pub struct SyncState {
    pub tab: u64,
    pub remote: String,
    pub local: Option<PathBuf>,
    pub rows: Vec<Row>,
    pub busy: bool,
    pub error: Option<String>,
    pub upload: bool,
    pub hide_same: bool,
    pub compared: bool,
}

impl SyncState {
    pub fn on_compared(&mut self, res: Result<Vec<Row>, String>) {
        self.busy = false;
        self.compared = true;
        match res {
            Ok(mut rows) => {
                for r in &mut rows {
                    r.selected = default_selected(r.diff, self.upload);
                }
                self.rows = rows;
                self.error = None;
            }
            Err(e) => self.error = Some(e),
        }
    }
}

fn diff_text(d: Diff) -> (&'static str, Color32) {
    match d {
        Diff::Same => (t!("gleich", "same"), MUTED),
        Diff::OnlyLocal => (t!("nur lokal", "local only"), CYAN),
        Diff::OnlyRemote => (t!("nur Server", "server only"), LAVENDER),
        Diff::LocalNewer => (t!("lokal neuer", "local newer"), GREEN),
        Diff::RemoteNewer => (t!("Server neuer", "server newer"), AMBER),
        Diff::Differs => (t!("abweichend", "differs"), RED),
    }
}

impl App {
    pub fn open_sync(&mut self, tab: u64, remote: String) {
        self.dialog = Some(Dialog::Sync(SyncState {
            tab,
            remote,
            local: None,
            rows: Vec::new(),
            busy: false,
            error: None,
            upload: true,
            hide_same: true,
            compared: false,
        }));
    }

    fn start_compare(&mut self, s: &mut SyncState) {
        let (Some(local), Some(sess)) = (s.local.clone(), self.tabs.iter().find(|t| t.id == s.tab).and_then(|t| t.ssh.clone())) else {
            s.error = Some(t!("Keine SSH-Verbindung.", "No SSH connection.").into());
            return;
        };
        s.busy = true;
        s.error = None;
        let (tab, remote) = (s.tab, s.remote.clone());
        let tx = self.tx.clone();
        let ctx = self.ctx.clone();
        self.rt.spawn(async move {
            let res = async {
                let l = tokio::task::spawn_blocking(move || ssh::walk_local(&local, 50_000)).await.map_err(|e| e.to_string())??;
                let r = ssh::walk_remote(&sess, &remote, 50_000).await?;
                Ok::<_, String>(compare(&l, &r))
            }
            .await;
            let _ = tx.send(Msg::SyncCompared(tab, res));
            ctx.request_repaint();
        });
    }

    /// Liefert false, wenn der Dialog geschlossen werden soll.
    pub fn sync_dialog(&mut self, ui: &mut Ui, s: &mut SyncState) -> bool {
        let mut keep = true;
        let screen = ui.ctx().content_rect();
        ui.set_width((screen.width() * 0.75).clamp(560.0, 1000.0));
        ui.horizontal(|ui| {
            ui.label(RichText::new("⇅").size(20.0));
            ui.label(RichText::new(t!("Ordner vergleichen & synchronisieren", "Compare & sync folders")).size(19.0).family(egui::FontFamily::Name("ui-bold".into())).color(Color32::WHITE));
        });
        ui.add_space(10.0);
        egui::Grid::new("sync_paths").num_columns(2).spacing(vec2(10.0, 6.0)).show(ui, |ui| {
            label(ui, t!("Server", "Server"));
            ui.add(egui::Label::new(RichText::new(&s.remote).monospace()).truncate());
            ui.end_row();
            label(ui, t!("Lokal", "Local"));
            ui.horizontal(|ui| {
                let shown = s.local.as_ref().map(|p| p.display().to_string()).unwrap_or_else(|| t!("– bitte wählen –", "– please choose –").into());
                let w = (ui.available_width() - 150.0).max(120.0);
                ui.allocate_ui(vec2(w, 22.0), |ui| {
                    ui.add(egui::Label::new(RichText::new(&shown).monospace()).truncate()).on_hover_text(&shown);
                });
                if ui.button(t!("Ordner wählen…", "Choose folder…")).clicked() {
                    if let Some(d) = rfd::FileDialog::new().set_title(t!("Lokalen Ordner wählen", "Choose local folder")).pick_folder() {
                        s.local = Some(d);
                        s.rows.clear();
                        s.compared = false;
                    }
                }
            });
            ui.end_row();
        });
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            let before = s.upload;
            ui.allocate_ui(vec2(360.0, 30.0), |ui| {
                if let Some(i) = segmented(ui, &[t!("⬆ Lokal → Server", "⬆ Local → server"), t!("⬇ Server → Lokal", "⬇ Server → local")], if s.upload { 0 } else { 1 }) {
                    s.upload = i == 0;
                }
            });
            if before != s.upload {
                for r in &mut s.rows {
                    r.selected = default_selected(r.diff, s.upload);
                }
            }
            ui.checkbox(&mut s.hide_same, t!("Gleiche ausblenden", "Hide identical"));
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let can = s.local.is_some() && !s.busy;
                if ui.add_enabled(can, egui::Button::new(if s.compared { t!("⟳ Neu vergleichen", "⟳ Compare again") } else { t!("Vergleichen", "Compare") })).clicked() {
                    self.start_compare(s);
                }
            });
        });
        if let Some(e) = &s.error {
            ui.label(RichText::new(format!("⚠ {e}")).color(RED));
        }
        ui.add_space(6.0);
        if s.busy {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(t!("Vergleiche Ordner…", "Comparing folders…"));
            });
        } else if s.compared {
            let count = |d: Diff| s.rows.iter().filter(|r| r.diff == d).count();
            ui.label(
                RichText::new(tf!(
                    "{} Dateien · {} gleich · {} nur lokal · {} nur Server · {} lokal neuer · {} Server neuer · {} abweichend",
                    "{} files · {} same · {} local only · {} server only · {} local newer · {} server newer · {} differ",
                    s.rows.len(),
                    count(Diff::Same),
                    count(Diff::OnlyLocal),
                    count(Diff::OnlyRemote),
                    count(Diff::LocalNewer),
                    count(Diff::RemoteNewer),
                    count(Diff::Differs)
                ))
                .size(12.0)
                .color(MUTED),
            );
            let h = (screen.height() * 0.45).max(200.0);
            egui::Frame::new().fill(BG).corner_radius(8).inner_margin(6).show(ui, |ui| {
                ScrollArea::vertical().max_height(h).min_scrolled_height(h).auto_shrink([false, false]).show(ui, |ui| {
                    let upload = s.upload;
                    egui::Grid::new("sync_rows").num_columns(5).striped(true).spacing(vec2(12.0, 3.0)).show(ui, |ui| {
                        ui.label("");
                        ui.label(RichText::new(t!("Datei", "File")).color(MUTED).size(12.0));
                        ui.label(RichText::new(t!("Lokal", "Local")).color(MUTED).size(12.0));
                        ui.label(RichText::new(t!("Server", "Server")).color(MUTED).size(12.0));
                        ui.label(RichText::new(t!("Status", "Status")).color(MUTED).size(12.0));
                        ui.end_row();
                        for r in s.rows.iter_mut().filter(|r| !(s.hide_same && r.diff == Diff::Same)).take(5000) {
                            let possible = if upload { r.local.is_some() } else { r.remote.is_some() };
                            ui.add_enabled(possible, egui::Checkbox::without_text(&mut r.selected));
                            ui.label(RichText::new(&r.rel).monospace().size(12.5));
                            ui.label(RichText::new(r.local.map(|(s, _)| human_size(s)).unwrap_or_else(|| "–".into())).size(12.0).color(MUTED));
                            ui.label(RichText::new(r.remote.map(|(s, _)| human_size(s)).unwrap_or_else(|| "–".into())).size(12.0).color(MUTED));
                            let (txt, col) = diff_text(r.diff);
                            ui.label(RichText::new(txt).size(12.0).color(col));
                            ui.end_row();
                        }
                    });
                });
            });
            ui.horizontal(|ui| {
                if ui.small_button(t!("Alle", "All")).clicked() {
                    let up = s.upload;
                    for r in &mut s.rows {
                        r.selected = if up { r.local.is_some() } else { r.remote.is_some() };
                    }
                }
                if ui.small_button(t!("Keine", "None")).clicked() {
                    for r in &mut s.rows {
                        r.selected = false;
                    }
                }
                if ui.small_button(t!("Nur Änderungen", "Changes only")).clicked() {
                    let up = s.upload;
                    for r in &mut s.rows {
                        r.selected = default_selected(r.diff, up);
                    }
                }
            });
        } else {
            ui.label(RichText::new(t!(
                "Wähle einen lokalen Ordner und klicke „Vergleichen“. Danach werden neue und geänderte Dateien angezeigt; nichts wird gelöscht.",
                "Choose a local folder and click \"Compare\". New and changed files are listed afterwards; nothing is deleted."
            )).color(MUTED));
        }
        ui.add_space(10.0);
        let selected: Vec<Row> = s.rows.iter().filter(|r| r.selected && if s.upload { r.local.is_some() } else { r.remote.is_some() }).cloned().collect();
        let total: u64 = selected.iter().map(|r| if s.upload { r.local.map(|x| x.0) } else { r.remote.map(|x| x.0) }.unwrap_or(0)).sum();
        ui.horizontal(|ui| {
            if ui.add(egui::Button::new(RichText::new(t!("Schließen", "Close")).strong()).min_size(vec2(130.0, 34.0))).clicked() {
                keep = false;
            }
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let text = if s.upload {
                    tf!("⬆ {} Dateien hochladen ({})", "⬆ Upload {} files ({})", selected.len(), human_size(total))
                } else {
                    tf!("⬇ {} Dateien herunterladen ({})", "⬇ Download {} files ({})", selected.len(), human_size(total))
                };
                let b = egui::Button::new(RichText::new(text).strong().color(Color32::WHITE)).fill(ACCENT).min_size(vec2(220.0, 34.0));
                if ui.add_enabled(!selected.is_empty() && !s.busy, b).clicked() {
                    let local = s.local.clone().unwrap_or_default();
                    for r in &selected {
                        let lp = local.join(r.rel.replace('/', "\\"));
                        let rp = join(&s.remote, &r.rel);
                        let job = if s.upload { Job::UploadTo { local: lp, remote: rp } } else { Job::Download { remote: rp, local: lp } };
                        let i = self.transfers.len();
                        self.queue(s.tab, job);
                        if !s.upload {
                            self.transfers[i].total = r.remote.map(|x| x.0).unwrap_or(0);
                        }
                    }
                    self.notify(tf!("{} Übertragungen eingereiht", "{} transfers queued", selected.len()));
                    if !self.show_files {
                        self.toggle_files();
                    }
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

#[cfg(test)]
mod tests {
    use super::*;

    fn e(rel: &str, size: u64, mtime: u64) -> TreeEntry {
        TreeEntry { rel: rel.into(), is_dir: false, size, mtime }
    }

    #[test]
    fn compares_trees() {
        let local = vec![e("a.txt", 1, 100), e("b.txt", 2, 200), e("c.txt", 3, 300), e("d.txt", 4, 400), TreeEntry { rel: "sub".into(), is_dir: true, size: 0, mtime: 0 }];
        let remote = vec![e("a.txt", 1, 101), e("b.txt", 2, 150), e("c.txt", 3, 500), e("d.txt", 5, 400), e("x.txt", 9, 1)];
        let rows = compare(&local, &remote);
        let d: Vec<(&str, Diff)> = rows.iter().map(|r| (r.rel.as_str(), r.diff)).collect();
        assert_eq!(
            d,
            vec![("a.txt", Diff::Same), ("b.txt", Diff::LocalNewer), ("c.txt", Diff::RemoteNewer), ("d.txt", Diff::Differs), ("x.txt", Diff::OnlyRemote)]
        );
        assert!(default_selected(Diff::LocalNewer, true));
        assert!(!default_selected(Diff::LocalNewer, false));
        assert!(!default_selected(Diff::Same, true));
    }
}
