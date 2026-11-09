//! Natives Terminal-Widget: alacritty_terminal (VT-Emulation) + egui (Darstellung).

use crate::theme::Theme;
use alacritty_terminal::event::{Event as TermEvent, EventListener};
use alacritty_terminal::grid::{Dimensions, Scroll};
use alacritty_terminal::index::{Boundary, Column, Direction, Line, Point, Side};
use alacritty_terminal::selection::{Selection, SelectionType};
use alacritty_terminal::term::cell::Flags;
use alacritty_terminal::term::color::Colors;
use alacritty_terminal::term::search::RegexSearch;
use alacritty_terminal::term::{Config, Term, TermMode};
use alacritty_terminal::vte::ansi::{
    Color, CursorShape, CursorStyle, NamedColor, Processor, Rgb,
};
use eframe::egui::{
    self, pos2, vec2, Align2, Color32, CursorIcon, EventFilter, FontId, Key, Modifiers, Rect,
    Sense, Stroke, StrokeKind, Ui,
};
use std::sync::mpsc;
use std::sync::{Arc, Mutex};

const PAD: f32 = 8.0;

/// Leitet Antworten des Terminals (z.B. auf Geräte-Abfragen) und den Fenstertitel weiter.
#[derive(Clone)]
pub struct Listener(mpsc::Sender<TermEvent>);

impl EventListener for Listener {
    fn send_event(&self, event: TermEvent) {
        if matches!(event, TermEvent::PtyWrite(_) | TermEvent::Title(_)) {
            let _ = self.0.send(event);
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Size {
    pub cols: usize,
    pub rows: usize,
}

impl Dimensions for Size {
    fn total_lines(&self) -> usize {
        self.rows
    }
    fn screen_lines(&self) -> usize {
        self.rows
    }
    fn columns(&self) -> usize {
        self.cols
    }
}

/// Terminalzustand; wird vom Netzwerk-Thread befüllt und vom UI-Thread gezeichnet.
pub struct TermCore {
    pub term: Term<Listener>,
    parser: Processor,
}

impl TermCore {
    pub fn feed(&mut self, bytes: &[u8]) {
        self.parser.advance(&mut self.term, bytes);
    }
}

/// Darstellungs- und Verhaltensoptionen.
pub struct View<'a> {
    pub font: FontId,
    pub theme: &'a Theme,
    pub copy_on_select: bool,
    pub right_click_paste: bool,
}

pub struct Terminal {
    pub core: Arc<Mutex<TermCore>>,
    events: mpsc::Receiver<TermEvent>,
    pub size: Size,
    cell: egui::Vec2,
    selecting: bool,
    scroll_accum: f32,
    /// Vom Server gesetzter Fenstertitel (OSC 0/2)
    pub title: Option<String>,
}

pub enum Output {
    /// Bytes, die an die Verbindung gehen sollen (Eingaben des Benutzers).
    Input(Vec<u8>),
    /// Automatische Antworten des Terminals (nicht broadcasten).
    Reply(Vec<u8>),
    /// Neue Terminalgröße.
    Resize(Size),
}

pub fn cursor_style(name: &str) -> CursorStyle {
    let shape = match name {
        "beam" => CursorShape::Beam,
        "underline" => CursorShape::Underline,
        _ => CursorShape::Block,
    };
    CursorStyle {
        shape,
        blinking: false,
    }
}

impl Terminal {
    pub fn new(scrollback: usize, cursor: &str, size: Option<Size>) -> Self {
        let (tx, rx) = mpsc::channel();
        let size = size.unwrap_or(Size { cols: 100, rows: 30 });
        let config = Config {
            scrolling_history: scrollback,
            default_cursor_style: cursor_style(cursor),
            ..Default::default()
        };
        let term = Term::new(config, &size, Listener(tx));
        Self {
            core: Arc::new(Mutex::new(TermCore {
                term,
                parser: Processor::new(),
            })),
            events: rx,
            size,
            cell: vec2(8.0, 17.0),
            selecting: false,
            scroll_accum: 0.0,
            title: None,
        }
    }

    pub fn apply_prefs(&self, scrollback: usize, cursor: &str) {
        let config = Config {
            scrolling_history: scrollback,
            default_cursor_style: cursor_style(cursor),
            ..Default::default()
        };
        self.core.lock().unwrap().term.set_options(config);
    }

    /// Text lokal anzeigen (Statusmeldungen).
    pub fn print(&self, text: &str) {
        let s = text.replace('\n', "\r\n");
        self.core.lock().unwrap().feed(s.as_bytes());
    }

    pub fn clear(&self) {
        // Bildschirm + Verlauf löschen
        self.core.lock().unwrap().feed(b"\x1b[H\x1b[2J\x1b[3J");
    }

    pub fn selection_text(&self) -> Option<String> {
        self.core.lock().unwrap().term.selection_to_string()
    }

    /// Kompletter Verlauf als Text (für "Ausgabe speichern").
    pub fn all_text(&self) -> String {
        let core = self.core.lock().unwrap();
        let term = &core.term;
        let grid = term.grid();
        let top = Line(-(grid.history_size() as i32));
        let bottom = Line(grid.screen_lines() as i32 - 1);
        let last = Column(grid.columns() - 1);
        let mut out = String::new();
        let mut line = top;
        while line <= bottom {
            let s = term.bounds_to_string(Point::new(line, Column(0)), Point::new(line, last));
            out.push_str(s.trim_end_matches(['\n', ' ']));
            out.push('\n');
            line += 1;
        }
        out
    }

    /// Sucht den nächsten Treffer (rückwärts = nach oben) und markiert ihn.
    pub fn find(&mut self, needle: &str, backwards: bool) -> bool {
        if needle.is_empty() {
            return false;
        }
        let pattern: String = needle
            .chars()
            .flat_map(|c| {
                if "\\.+*?()|[]{}^$#&-~".contains(c) {
                    vec!['\\', c]
                } else {
                    vec![c]
                }
            })
            .collect();
        let Ok(mut regex) = RegexSearch::new(&format!("(?i){pattern}")) else {
            return false;
        };
        let mut core = self.core.lock().unwrap();
        let term = &mut core.term;
        let origin = match term.selection.as_ref().and_then(|s| s.to_range(term)) {
            Some(r) if backwards => r.start.sub(term, Boundary::None, 1),
            Some(r) => r.end.add(term, Boundary::None, 1),
            None => {
                let off = term.grid().display_offset() as i32;
                if backwards {
                    Point::new(Line(term.screen_lines() as i32 - 1 - off), Column(0))
                } else {
                    Point::new(Line(-off), Column(0))
                }
            }
        };
        let (dir, side) = if backwards {
            (Direction::Left, Side::Left)
        } else {
            (Direction::Right, Side::Right)
        };
        match term.search_next(&mut regex, origin, dir, side, None) {
            Some(m) => {
                let mut sel = Selection::new(SelectionType::Simple, *m.start(), Side::Left);
                sel.update(*m.end(), Side::Right);
                term.selection = Some(sel);
                term.scroll_to_point(*m.start());
                true
            }
            None => false,
        }
    }

    /// Zeichnet das Terminal in den verfügbaren Platz und verarbeitet Eingaben.
    pub fn show(&mut self, ui: &mut Ui, id: egui::Id, view: &View) -> Vec<Output> {
        let mut out = Vec::new();
        while let Ok(ev) = self.events.try_recv() {
            match ev {
                TermEvent::PtyWrite(s) => out.push(Output::Reply(s.into_bytes())),
                TermEvent::Title(t) => self.title = Some(t),
                _ => {}
            }
        }

        let rect = ui.available_rect_before_wrap();
        let resp = ui.interact(rect, id, Sense::click_and_drag());
        ui.painter().rect_filled(rect, 0.0, view.theme.bg);
        let inner = rect.shrink(PAD);

        // Zellgröße aus der Schrift bestimmen
        let (cw, ch) = ui.fonts_mut(|f| (f.glyph_width(&view.font, 'M'), f.row_height(&view.font)));
        self.cell = vec2(cw.max(1.0), ch.max(1.0));
        let cols = ((inner.width() / self.cell.x).floor() as usize).max(10);
        let rows = ((inner.height() / self.cell.y).floor() as usize).max(3);
        let new_size = Size { cols, rows };
        if new_size != self.size {
            self.size = new_size;
            self.core.lock().unwrap().term.resize(new_size);
            out.push(Output::Resize(new_size));
        }

        if resp.clicked() || resp.drag_started() || resp.secondary_clicked() {
            resp.request_focus();
        }
        let focused = resp.has_focus();
        if focused {
            ui.memory_mut(|m| {
                m.set_focus_lock_filter(
                    id,
                    EventFilter {
                        tab: true,
                        horizontal_arrows: true,
                        vertical_arrows: true,
                        escape: true,
                    },
                )
            });
        }
        if resp.hovered() {
            ui.ctx().set_cursor_icon(CursorIcon::Text);
        }

        self.handle_mouse(ui, &resp, inner, view, &mut out);
        if focused {
            self.handle_keys(ui, &mut out);
        }
        self.paint(ui, inner, view, focused);
        out
    }

    fn point_at(&self, inner: Rect, pos: egui::Pos2, display_offset: usize) -> (Point, Side) {
        let x = ((pos.x - inner.left()) / self.cell.x).max(0.0);
        let y = ((pos.y - inner.top()) / self.cell.y).max(0.0);
        let col = (x as usize).min(self.size.cols - 1);
        let row = (y as usize).min(self.size.rows - 1);
        let side = if x.fract() > 0.5 { Side::Right } else { Side::Left };
        let point = alacritty_terminal::term::viewport_to_point(
            display_offset,
            Point::new(row, Column(col)),
        );
        (point, side)
    }

    fn handle_mouse(
        &mut self,
        ui: &Ui,
        resp: &egui::Response,
        inner: Rect,
        view: &View,
        out: &mut Vec<Output>,
    ) {
        // Rechtsklick: Einfügen (wie bisher) – mit Markierung: kopieren
        if resp.secondary_clicked() {
            let sel = self.selection_text().filter(|t| !t.is_empty());
            if let Some(text) = sel {
                ui.ctx().copy_text(text);
                self.core.lock().unwrap().term.selection = None;
            } else if view.right_click_paste {
                if let Some(text) = crate::clipboard::get() {
                    let mode = *self.core.lock().unwrap().term.mode();
                    out.push(Output::Input(paste_bytes(&text, mode)));
                }
            }
            return;
        }

        let mut core = self.core.lock().unwrap();
        let term = &mut core.term;
        let offset = term.grid().display_offset();

        // Scrollen
        if resp.hovered() {
            let dy = ui.input(|i| i.smooth_scroll_delta.y);
            if dy != 0.0 {
                self.scroll_accum += dy / self.cell.y;
                let lines = self.scroll_accum.trunc() as i32;
                if lines != 0 {
                    self.scroll_accum -= lines as f32;
                    let mode = *term.mode();
                    if mode.contains(TermMode::ALT_SCREEN) && mode.contains(TermMode::ALTERNATE_SCROLL) {
                        // In less/vim/htop: Pfeiltasten statt Verlauf
                        let seq: &[u8] = if lines > 0 { b"\x1bOA" } else { b"\x1bOB" };
                        for _ in 0..lines.unsigned_abs() {
                            out.push(Output::Input(seq.to_vec()));
                        }
                    } else {
                        term.scroll_display(Scroll::Delta(lines));
                    }
                }
            }
        }

        // Markieren (Doppelklick = Wort, Dreifachklick = Zeile)
        if let Some(pos) = resp.interact_pointer_pos() {
            let (point, side) = self.point_at(inner, pos, offset);
            if resp.triple_clicked() {
                term.selection = Some(Selection::new(SelectionType::Lines, point, side));
                self.selecting = false;
            } else if resp.double_clicked() {
                term.selection = Some(Selection::new(SelectionType::Semantic, point, side));
                self.selecting = false;
            } else if resp.drag_started() {
                term.selection = Some(Selection::new(SelectionType::Simple, point, side));
                self.selecting = true;
            } else if resp.dragged() && self.selecting {
                if let Some(sel) = term.selection.as_mut() {
                    sel.update(point, side);
                }
                // Am Rand automatisch scrollen
                if pos.y < inner.top() {
                    term.scroll_display(Scroll::Delta(1));
                } else if pos.y > inner.bottom() {
                    term.scroll_display(Scroll::Delta(-1));
                }
            }
        }
        if resp.drag_stopped() || resp.double_clicked() || resp.triple_clicked() {
            self.selecting = false;
            if view.copy_on_select {
                if let Some(text) = term.selection_to_string().filter(|t| !t.is_empty()) {
                    ui.ctx().copy_text(text);
                }
            }
        } else if resp.clicked() {
            term.selection = None;
        }
    }

    fn handle_keys(&mut self, ui: &Ui, out: &mut Vec<Output>) {
        let events = ui.input(|i| i.events.clone());
        let mode = *self.core.lock().unwrap().term.mode();
        let app_cursor = mode.contains(TermMode::APP_CURSOR);
        let mut typed = false;
        for ev in events {
            match ev {
                egui::Event::Text(t) => {
                    out.push(Output::Input(t.into_bytes()));
                    typed = true;
                }
                egui::Event::Paste(t) => {
                    out.push(Output::Input(paste_bytes(&t, mode)));
                    typed = true;
                }
                egui::Event::Copy => {
                    // Strg+C: mit Markierung kopieren, sonst ^C senden
                    match self.selection_text().filter(|t| !t.is_empty()) {
                        Some(text) => {
                            ui.ctx().copy_text(text);
                            self.core.lock().unwrap().term.selection = None;
                        }
                        None => {
                            out.push(Output::Input(vec![0x03]));
                            typed = true;
                        }
                    }
                }
                egui::Event::Cut => {
                    out.push(Output::Input(vec![0x18]));
                    typed = true;
                }
                egui::Event::Key {
                    key,
                    pressed: true,
                    modifiers,
                    ..
                } => {
                    if modifiers.shift && matches!(key, Key::PageUp | Key::PageDown | Key::Home | Key::End) {
                        let s = match key {
                            Key::PageUp => Scroll::PageUp,
                            Key::PageDown => Scroll::PageDown,
                            Key::Home => Scroll::Top,
                            _ => Scroll::Bottom,
                        };
                        self.core.lock().unwrap().term.scroll_display(s);
                        continue;
                    }
                    if let Some(bytes) = key_bytes(key, modifiers, app_cursor) {
                        out.push(Output::Input(bytes));
                        typed = true;
                    }
                }
                _ => {}
            }
        }
        if typed {
            let mut core = self.core.lock().unwrap();
            core.term.scroll_display(Scroll::Bottom);
            core.term.selection = None;
        }
    }

    fn paint(&self, ui: &Ui, inner: Rect, view: &View, focused: bool) {
        let theme = view.theme;
        let font = &view.font;
        let painter = ui.painter_at(inner.expand(PAD));
        let core = self.core.lock().unwrap();
        let content = core.term.renderable_content();
        let colors = content.colors;
        let offset = content.display_offset;
        let selection = content.selection;
        let cursor = content.cursor;
        let show_cursor = content.mode.contains(TermMode::SHOW_CURSOR)
            && cursor.shape != CursorShape::Hidden;
        let (cw, ch) = (self.cell.x, self.cell.y);

        struct Run {
            x: f32,
            y: f32,
            text: String,
            color: Color32,
            bold: bool,
            underline: bool,
            strike: bool,
            cells: usize,
            mergeable: bool,
        }
        let mut runs: Vec<Run> = Vec::with_capacity(self.size.rows * 4);
        let mut bg_rects: Vec<(Rect, Color32)> = Vec::new();

        for indexed in content.display_iter {
            let cell = indexed.cell;
            if cell.flags.contains(Flags::WIDE_CHAR_SPACER) {
                continue;
            }
            let point = indexed.point;
            let vline = (point.line.0 + offset as i32) as f32;
            let x = inner.left() + point.column.0 as f32 * cw;
            let y = inner.top() + vline * ch;
            let width_cells = if cell.flags.contains(Flags::WIDE_CHAR) { 2 } else { 1 };

            let mut fg = resolve(cell.fg, colors, cell.flags.contains(Flags::BOLD), true, theme);
            let mut bg = resolve(cell.bg, colors, false, false, theme);
            if cell.flags.contains(Flags::DIM) {
                fg = fg.gamma_multiply(0.66);
            }
            if cell.flags.contains(Flags::INVERSE) {
                std::mem::swap(&mut fg, &mut bg);
            }
            let selected =
                selection.is_some_and(|s| s.contains_cell(&indexed, cursor.point, cursor.shape));
            if selected {
                bg = theme.selection;
                fg = if theme.id == "light" { Color32::BLACK } else { Color32::WHITE };
            }
            if bg != theme.bg {
                let r = Rect::from_min_size(pos2(x, y), vec2(cw * width_cells as f32, ch));
                match bg_rects.last_mut() {
                    Some((last, c))
                        if *c == bg
                            && (last.right() - r.left()).abs() < 0.5
                            && last.top() == r.top() =>
                    {
                        last.max.x = r.right();
                    }
                    _ => bg_rects.push((r, bg)),
                }
            }
            if cell.c == ' ' || cell.c == '\t' || cell.flags.contains(Flags::HIDDEN) {
                continue;
            }
            let is_bold = cell.flags.contains(Flags::BOLD);
            let underline = cell.flags.intersects(Flags::ALL_UNDERLINES);
            let strike = cell.flags.contains(Flags::STRIKEOUT);
            // Nur ASCII wird zu Läufen zusammengefasst; andere Zeichen (Symbole, Emojis)
            // stehen einzeln an ihrer Spalte, damit nichts verrutscht.
            let simple = cell.c.is_ascii() && width_cells == 1;
            if simple {
                if let Some(last) = runs.last_mut() {
                    let contiguous =
                        last.y == y && (last.x + last.cells as f32 * cw - x).abs() < 0.5;
                    if last.mergeable
                        && contiguous
                        && last.color == fg
                        && last.bold == is_bold
                        && last.underline == underline
                        && last.strike == strike
                    {
                        last.text.push(cell.c);
                        last.cells += 1;
                        continue;
                    }
                }
            }
            let mut text = String::new();
            text.push(cell.c);
            if let Some(zw) = cell.zerowidth() {
                text.extend(zw);
            }
            runs.push(Run {
                x,
                y,
                text,
                color: fg,
                bold: is_bold,
                underline,
                strike,
                cells: width_cells,
                mergeable: simple,
            });
        }

        for (r, c) in bg_rects {
            painter.rect_filled(r, 0.0, c);
        }
        for run in &runs {
            painter.text(pos2(run.x, run.y), Align2::LEFT_TOP, &run.text, font.clone(), run.color);
            if run.bold {
                // Fett: gleiche Schrift leicht versetzt (identische Zeichenbreite)
                painter.text(pos2(run.x + 0.6, run.y), Align2::LEFT_TOP, &run.text, font.clone(), run.color);
            }
            let n = run.cells as f32;
            if run.underline {
                let yy = run.y + ch - 2.0;
                painter.line_segment([pos2(run.x, yy), pos2(run.x + n * cw, yy)], Stroke::new(1.0, run.color));
            }
            if run.strike {
                let yy = run.y + ch * 0.55;
                painter.line_segment([pos2(run.x, yy), pos2(run.x + n * cw, yy)], Stroke::new(1.0, run.color));
            }
        }

        // Cursor
        if show_cursor {
            let vline = cursor.point.line.0 + offset as i32;
            if vline >= 0 && (vline as usize) < self.size.rows {
                let x = inner.left() + cursor.point.column.0 as f32 * cw;
                let y = inner.top() + vline as f32 * ch;
                let r = Rect::from_min_size(pos2(x, y), vec2(cw, ch));
                let cc = theme.cursor;
                if !focused {
                    painter.rect_stroke(r, 0.0, Stroke::new(1.0, cc), StrokeKind::Inside);
                } else {
                    match cursor.shape {
                        CursorShape::Beam => {
                            painter.rect_filled(Rect::from_min_size(r.min, vec2(2.0, ch)), 0.0, cc);
                        }
                        CursorShape::Underline => {
                            painter.rect_filled(
                                Rect::from_min_size(pos2(x, y + ch - 2.0), vec2(cw, 2.0)),
                                0.0,
                                cc,
                            );
                        }
                        CursorShape::HollowBlock => {
                            painter.rect_stroke(r, 0.0, Stroke::new(1.0, cc), StrokeKind::Inside);
                        }
                        _ => {
                            painter.rect_filled(r, 0.0, cc);
                            let cell = &core.term.grid()[cursor.point];
                            if cell.c != ' ' {
                                painter.text(r.min, Align2::LEFT_TOP, cell.c, font.clone(), theme.bg);
                            }
                        }
                    }
                }
            }
        }

        // Hinweis, wenn im Verlauf gescrollt wird
        if offset > 0 {
            let total = core.term.grid().history_size();
            let label = format!("↑ {offset} / {total} Zeilen  ·  Umschalt+Ende = unten");
            let galley = painter.layout_no_wrap(label, FontId::proportional(12.0), Color32::WHITE);
            let p = pos2(inner.right() - 4.0, inner.top() + 2.0);
            let bg = Rect::from_min_size(
                p - vec2(galley.size().x + 12.0, 0.0),
                galley.size() + vec2(12.0, 6.0),
            );
            painter.rect_filled(bg, 4.0, Color32::from_rgba_unmultiplied(40, 30, 70, 230));
            painter.galley(bg.min + vec2(6.0, 3.0), galley, Color32::WHITE);
        }
    }
}

pub fn paste_bytes(text: &str, mode: TermMode) -> Vec<u8> {
    let t = text.replace("\r\n", "\r").replace('\n', "\r");
    if mode.contains(TermMode::BRACKETED_PASTE) {
        format!("\x1b[200~{}\x1b[201~", t.replace('\x1b', "")).into_bytes()
    } else {
        t.into_bytes()
    }
}

fn key_bytes(key: Key, m: Modifiers, app_cursor: bool) -> Option<Vec<u8>> {
    // Strg + Buchstabe -> Steuerzeichen
    if m.ctrl && !m.alt {
        let name = key.name();
        if name.len() == 1 {
            let c = name.as_bytes()[0];
            if c.is_ascii_alphabetic() {
                return Some(vec![c.to_ascii_lowercase() - b'a' + 1]);
            }
        }
        match key {
            Key::OpenBracket => return Some(vec![0x1b]),
            Key::Backslash => return Some(vec![0x1c]),
            Key::CloseBracket => return Some(vec![0x1d]),
            Key::Space => return Some(vec![0x00]),
            _ => {}
        }
    }
    // Modifikator-Parameter für CSI-Sequenzen (xterm): 1 + shift + 2*alt + 4*ctrl
    let modp = 1 + m.shift as u8 + 2 * m.alt as u8 + 4 * m.ctrl as u8;
    let csi = |final_: char| -> Vec<u8> {
        if modp > 1 {
            format!("\x1b[1;{modp}{final_}").into_bytes()
        } else if app_cursor {
            format!("\x1bO{final_}").into_bytes()
        } else {
            format!("\x1b[{final_}").into_bytes()
        }
    };
    let tilde = |n: u8| -> Vec<u8> {
        if modp > 1 {
            format!("\x1b[{n};{modp}~").into_bytes()
        } else {
            format!("\x1b[{n}~").into_bytes()
        }
    };
    let bytes = match key {
        Key::Enter => b"\r".to_vec(),
        Key::Tab if m.shift => b"\x1b[Z".to_vec(),
        Key::Tab => b"\t".to_vec(),
        Key::Backspace if m.ctrl => vec![0x08],
        Key::Backspace => vec![0x7f],
        Key::Escape => vec![0x1b],
        Key::ArrowUp => csi('A'),
        Key::ArrowDown => csi('B'),
        Key::ArrowRight => csi('C'),
        Key::ArrowLeft => csi('D'),
        Key::Home => csi('H'),
        Key::End => csi('F'),
        Key::Insert => tilde(2),
        Key::Delete => tilde(3),
        Key::PageUp => tilde(5),
        Key::PageDown => tilde(6),
        Key::F1 => b"\x1bOP".to_vec(),
        Key::F2 => b"\x1bOQ".to_vec(),
        Key::F3 => b"\x1bOR".to_vec(),
        Key::F4 => b"\x1bOS".to_vec(),
        Key::F5 => tilde(15),
        Key::F6 => tilde(17),
        Key::F7 => tilde(18),
        Key::F8 => tilde(19),
        Key::F9 => tilde(20),
        Key::F10 => tilde(21),
        Key::F11 => tilde(23),
        Key::F12 => tilde(24),
        _ => return None,
    };
    if m.alt && !m.ctrl && bytes.len() == 1 {
        // Alt als Meta-Präfix
        return Some([&[0x1b], bytes.as_slice()].concat());
    }
    Some(bytes)
}

fn indexed(i: u8, theme: &Theme) -> Color32 {
    match i {
        0..=15 => theme.ansi[i as usize],
        16..=231 => {
            let i = i - 16;
            let step = |v: u8| if v == 0 { 0 } else { 55 + v * 40 };
            Color32::from_rgb(step(i / 36), step((i / 6) % 6), step(i % 6))
        }
        _ => {
            let v = 8 + (i - 232) * 10;
            Color32::from_rgb(v, v, v)
        }
    }
}

fn rgb(c: Rgb) -> Color32 {
    Color32::from_rgb(c.r, c.g, c.b)
}

fn resolve(color: Color, colors: &Colors, bold: bool, is_fg: bool, theme: &Theme) -> Color32 {
    match color {
        Color::Spec(c) => rgb(c),
        Color::Indexed(i) => {
            let i = if bold && i < 8 { i + 8 } else { i };
            colors[i as usize].map(rgb).unwrap_or_else(|| indexed(i, theme))
        }
        Color::Named(n) => {
            if let Some(c) = colors[n] {
                return rgb(c);
            }
            let idx = n as usize;
            match n {
                NamedColor::Foreground | NamedColor::BrightForeground => {
                    if is_fg {
                        theme.fg
                    } else {
                        theme.bg
                    }
                }
                NamedColor::Background => theme.bg,
                NamedColor::Cursor => theme.cursor,
                NamedColor::DimForeground => theme.fg.gamma_multiply(0.66),
                _ if idx < 16 => {
                    let i = if bold && idx < 8 { idx + 8 } else { idx };
                    theme.ansi[i]
                }
                _ => {
                    // Dim-Farben (DimBlack..DimWhite)
                    let base = idx.saturating_sub(NamedColor::DimBlack as usize).min(7);
                    theme.ansi[base].gamma_multiply(0.66)
                }
            }
        }
    }
}
