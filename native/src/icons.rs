//! Einheitliches Linien-Icon-Set (24er-Raster, 1,8er Strich), als SVG gezeichnet und je
//! Pixelgröße einmal in eine Textur gerastert. Die Icons sind weiß und werden beim Zeichnen eingefärbt.

use eframe::egui::{self, pos2, vec2, Color32, Rect, Response, RichText, Sense, TextureHandle, TextureId, Ui};
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Icon {
    Settings,
    Key,
    Import,
    Export,
    Lock,
    Plus,
    Activity,
    Search,
    Star,
    StarFill,
    Terminal,
    Plug,
    Server,
    Globe,
    ChevronDown,
    ChevronRight,
    ChevronLeft,
    Bolt,
    Copy,
    Refresh,
    Trash,
    Shield,
    Clock,
    Archive,
    Info,
    Sidebar,
    Close,
    Split,
    Broadcast,
    Folder,
    File,
    Pencil,
    Play,
    Power,
    Tunnel,
    Download,
    Window,
    Home,
    ArrowUp,
    ArrowUpCircle,
    Check,
    CheckCircle,
    Eraser,
    Jump,
    Agent,
    History,
    Palette,
    Keyboard,
    Warning,
    Code,
    Image,
    Back,
}

fn gear() -> String {
    // 8 Zähne, Fuß bei r=7,2, Spitze bei r=9,6
    let mut d = String::new();
    for k in 0..8 {
        let a = k as f32 * 45.0;
        for (off, r) in [(-17.0, 7.0), (-9.0, 9.6), (9.0, 9.6), (17.0, 7.0)] {
            let t = (a + off as f32).to_radians();
            let cmd = if d.is_empty() { 'M' } else { 'L' };
            d.push_str(&format!("{cmd}{:.2} {:.2} ", 12.0 + r * t.cos(), 12.0 + r * t.sin()));
        }
    }
    format!(r#"<path d="{d}Z"/><circle cx="12" cy="12" r="3"/>"#)
}

fn body(icon: Icon) -> String {
    let s = match icon {
        Icon::Settings => return gear(),
        Icon::Key => r#"<circle cx="7.5" cy="15.5" r="4.5"/><path d="M10.7 12.3 20 3M15.5 7.5l3 3M18.5 4.5l2 2"/>"#,
        Icon::Import => r#"<path d="M12 3v11M7.5 9.5 12 14l4.5-4.5M4 15v3a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2v-3"/>"#,
        Icon::Export => r#"<path d="M12 14V3M7.5 7.5 12 3l4.5 4.5M4 15v3a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2v-3"/>"#,
        Icon::Lock => r#"<rect x="5" y="11" width="14" height="10" rx="2.5"/><path d="M8 11V7.5a4 4 0 0 1 8 0V11"/>"#,
        Icon::Plus => r#"<path d="M12 5v14M5 12h14"/>"#,
        Icon::Activity => r#"<path d="M3 12h4l3-7 4 14 3-7h4"/>"#,
        Icon::Search => r#"<circle cx="11" cy="11" r="6.5"/><path d="m16 16 4.5 4.5"/>"#,
        Icon::Star => r#"<path d="M12 3.2 14.7 8.7l6 .9-4.35 4.25 1.03 6L12 17l-5.38 2.85 1.03-6L3.3 9.6l6-.9z"/>"#,
        Icon::StarFill => r#"<path fill="white" d="M12 3.2 14.7 8.7l6 .9-4.35 4.25 1.03 6L12 17l-5.38 2.85 1.03-6L3.3 9.6l6-.9z"/>"#,
        Icon::Terminal => r#"<rect x="3" y="4" width="18" height="16" rx="3"/><path d="m7.5 9.5 3 2.5-3 2.5M12.5 15h4"/>"#,
        Icon::Plug => r#"<path d="M9 3v4M15 3v4M6 7h12v3a6 6 0 0 1-12 0zM12 16v5"/>"#,
        Icon::Server => r#"<rect x="3" y="4" width="18" height="7" rx="2"/><rect x="3" y="13" width="18" height="7" rx="2"/><path d="M7 7.5h.01M7 16.5h.01M11 7.5h6M11 16.5h6"/>"#,
        Icon::Globe => r#"<circle cx="12" cy="12" r="9"/><path d="M3 12h18M12 3a14 14 0 0 1 0 18M12 3a14 14 0 0 0 0 18"/>"#,
        Icon::ChevronDown => r#"<path d="m6 9 6 6 6-6"/>"#,
        Icon::ChevronRight => r#"<path d="m9 6 6 6-6 6"/>"#,
        Icon::ChevronLeft => r#"<path d="m15 6-6 6 6 6"/>"#,
        Icon::Bolt => r#"<path d="M13 2.5 4.5 13.5h7l-1 8 8.5-11h-7z"/>"#,
        Icon::Copy => r#"<rect x="9" y="9" width="12" height="12" rx="2.5"/><path d="M5 15h-.5A1.5 1.5 0 0 1 3 13.5v-9A1.5 1.5 0 0 1 4.5 3h9A1.5 1.5 0 0 1 15 4.5V5"/>"#,
        Icon::Refresh => r#"<path d="M20 12a8 8 0 1 1-8-8c2.2 0 4.3.9 5.7 2.3L20 8.5M20 4v4.5h-4.5"/>"#,
        Icon::Trash => r#"<path d="M4 7h16M10 11v6M14 11v6M6 7l1 12a2 2 0 0 0 2 2h6a2 2 0 0 0 2-2l1-12M9 7V4h6v3"/>"#,
        Icon::Shield => r#"<path d="M12 3 19 6v5c0 4.5-3 8-7 10-4-2-7-5.5-7-10V6z"/><path d="m9 12 2 2 4-4"/>"#,
        Icon::Clock => r#"<circle cx="12" cy="12" r="9"/><path d="M12 7v5l3 2"/>"#,
        Icon::Archive => r#"<rect x="3" y="4" width="18" height="5" rx="1.5"/><path d="M5 9v9a2 2 0 0 0 2 2h10a2 2 0 0 0 2-2V9M10 13h4"/>"#,
        Icon::Info => r#"<circle cx="12" cy="12" r="9"/><path d="M12 11v5M12 7.5h.01"/>"#,
        Icon::Sidebar => r#"<rect x="3" y="4" width="18" height="16" rx="3"/><path d="M9 4v16"/>"#,
        Icon::Close => r#"<path d="M6.5 6.5l11 11M17.5 6.5l-11 11"/>"#,
        Icon::Split => r#"<rect x="3" y="4" width="18" height="16" rx="3"/><path d="M12 4v16"/>"#,
        Icon::Broadcast => r#"<circle cx="12" cy="12" r="2"/><path d="M16.2 7.8a6 6 0 0 1 0 8.4M7.8 16.2a6 6 0 0 1 0-8.4M19 5a10 10 0 0 1 0 14M5 19A10 10 0 0 1 5 5"/>"#,
        Icon::Folder => r#"<path d="M3 7a2 2 0 0 1 2-2h4l2 2h8a2 2 0 0 1 2 2v8a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z"/>"#,
        Icon::File => r#"<path d="M14 3H7a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h10a2 2 0 0 0 2-2V8zM14 3v5h5"/>"#,
        Icon::Pencil => r#"<path d="M4 20h4L19 9a2.83 2.83 0 0 0-4-4L4 16zM13.5 6.5l4 4"/>"#,
        Icon::Play => r#"<path d="M7 4.5v15l12-7.5z"/>"#,
        Icon::Power => r#"<path d="M12 3v9M6.3 6.8a8 8 0 1 0 11.4 0"/>"#,
        Icon::Tunnel => r#"<path d="M4 8h16M16 4l4 4-4 4M20 16H4M8 12l-4 4 4 4"/>"#,
        Icon::Download => r#"<path d="M12 3v12M7 10l5 5 5-5M4 20h16"/>"#,
        Icon::Window => r#"<path d="M14 4h6v6M20 4l-9 9M18 14v4a2 2 0 0 1-2 2H6a2 2 0 0 1-2-2V8a2 2 0 0 1 2-2h4"/>"#,
        Icon::Home => r#"<path d="m3 11 9-7 9 7M5 9.5V19a1 1 0 0 0 1 1h4v-6h4v6h4a1 1 0 0 0 1-1V9.5"/>"#,
        Icon::ArrowUp => r#"<path d="M12 19V5M6 11l6-6 6 6"/>"#,
        Icon::ArrowUpCircle => r#"<circle cx="12" cy="12" r="9"/><path d="M12 16V8M8.5 11.5 12 8l3.5 3.5"/>"#,
        Icon::Check => r#"<path d="m5 12.5 4.5 4.5L19 7.5"/>"#,
        Icon::CheckCircle => r#"<circle cx="12" cy="12" r="9"/><path d="m8 12.5 3 3 5-6"/>"#,
        Icon::Eraser => r#"<path d="M20 20H9l-4.6-4.6a2 2 0 0 1 0-2.8L13.6 3.4a2 2 0 0 1 2.8 0l4.2 4.2a2 2 0 0 1 0 2.8L11 20M8.5 9.5l6 6"/>"#,
        Icon::Jump => r#"<path d="M5 4v7a4 4 0 0 0 4 4h11M15 10l5 5-5 5"/>"#,
        Icon::Agent => r#"<rect x="3" y="5" width="18" height="14" rx="2.5"/><circle cx="9" cy="11" r="2"/><path d="M5.8 16c.6-1.4 1.8-2 3.2-2s2.6.6 3.2 2M14.5 10h3.5M14.5 13.5h2.5"/>"#,
        Icon::History => r#"<path d="M3.5 12a8.5 8.5 0 1 0 2.5-6L3.5 8.5M3.5 4v4.5H8M12 7.5V12l3 2"/>"#,
        Icon::Palette => r#"<path d="M12 3a9 9 0 0 0 0 18c1.1 0 1.8-.8 1.8-1.8 0-.5-.2-.9-.5-1.2-.3-.3-.5-.7-.5-1.2 0-1 .8-1.8 1.8-1.8H17a4 4 0 0 0 4-4c0-4.4-4-8-9-8z"/><path d="M7.5 11.5h.01M10 7.5h.01M14.5 7.5h.01"/>"#,
        Icon::Keyboard => r#"<rect x="2.5" y="6" width="19" height="12" rx="2.5"/><path d="M6.5 10h.01M10 10h.01M14 10h.01M17.5 10h.01M8 14h8"/>"#,
        Icon::Code => r#"<path d="M14 3H7a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h10a2 2 0 0 0 2-2V8zM14 3v5h5M10 12.5 8 14.5l2 2M14 12.5l2 2-2 2"/>"#,
        Icon::Image => r#"<rect x="3" y="4" width="18" height="16" rx="2.5"/><circle cx="9" cy="9.5" r="1.8"/><path d="m21 15.5-4.5-4.5L6 20"/>"#,
        Icon::Back => r#"<path d="M19 12H5M11 6l-6 6 6 6"/>"#,
        Icon::Warning => r#"<path d="M10.3 4.3 2.6 17.5A2 2 0 0 0 4.3 20.5h15.4a2 2 0 0 0 1.7-3L13.7 4.3a2 2 0 0 0-3.4 0zM12 9.5v4M12 17h.01"/>"#,
    };
    s.to_string()
}

fn rasterize(icon: Icon, px: u32) -> egui::ColorImage {
    // Kleine Größen bekommen einen etwas kräftigeren Strich, damit sie nicht verwaschen wirken.
    let stroke = if px <= 18 { 2.1 } else if px <= 26 { 1.9 } else { 1.75 };
    let svg = format!(
        r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" fill="none" stroke="white" stroke-width="{stroke}" stroke-linecap="round" stroke-linejoin="round">{}</svg>"#,
        body(icon)
    );
    let tree = resvg::usvg::Tree::from_str(&svg, &resvg::usvg::Options::default()).expect("Icon-SVG");
    let mut pm = resvg::tiny_skia::Pixmap::new(px, px).expect("Pixmap");
    let s = px as f32 / 24.0;
    resvg::render(&tree, resvg::tiny_skia::Transform::from_scale(s, s), &mut pm.as_mut());
    egui::ColorImage::from_rgba_premultiplied([px as usize, px as usize], pm.data())
}

static CACHE: OnceLock<Mutex<HashMap<(Icon, u32), TextureHandle>>> = OnceLock::new();

/// Textur für ein Icon in `size` Punkten – pixelgenau für die aktuelle Bildschirmskalierung.
pub fn texture(ctx: &egui::Context, icon: Icon, size: f32) -> TextureId {
    let px = (size * ctx.pixels_per_point()).round().max(8.0) as u32;
    let mut cache = CACHE.get_or_init(Default::default).lock().unwrap();
    cache
        .entry((icon, px))
        .or_insert_with(|| ctx.load_texture(format!("icon-{icon:?}-{px}"), rasterize(icon, px), egui::TextureOptions::LINEAR))
        .id()
}

pub fn paint(ui: &Ui, rect: Rect, icon: Icon, color: Color32) {
    let id = texture(ui.ctx(), icon, rect.width());
    ui.painter().image(id, rect, Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)), color);
}

pub fn image(ctx: &egui::Context, icon: Icon, size: f32, color: Color32) -> egui::Image<'static> {
    egui::Image::from_texture((texture(ctx, icon, size), vec2(size, size))).tint(color)
}

/// Icon als einfaches Element (ohne Interaktion).
pub fn show(ui: &mut Ui, icon: Icon, size: f32, color: Color32) -> Response {
    let (rect, resp) = ui.allocate_exact_size(vec2(size, size), Sense::hover());
    paint(ui, rect, icon, color);
    resp
}

pub const HOVER_BG: Color32 = Color32::from_rgb(0x2b, 0x2a, 0x42);

/// Quadratischer Symbolknopf ohne Rahmen; beim Überfahren hinterlegt.
pub fn button(ui: &mut Ui, icon: Icon, tip: &str) -> Response {
    toggle(ui, icon, tip, false, Color32::WHITE)
}

/// Symbolknopf, der eingeschaltet in `on_color` leuchtet.
pub fn toggle(ui: &mut Ui, icon: Icon, tip: &str, on: bool, on_color: Color32) -> Response {
    let size = 30.0;
    let (rect, resp) = ui.allocate_exact_size(vec2(size, size), Sense::click());
    let hovered = resp.hovered();
    if on {
        ui.painter().rect_filled(rect, 8.0, on_color.gamma_multiply(0.16));
    } else if hovered {
        ui.painter().rect_filled(rect, 8.0, HOVER_BG);
    }
    let col = if on {
        on_color
    } else if hovered {
        Color32::WHITE
    } else {
        Color32::from_rgb(0xa9, 0xa7, 0xc2)
    };
    paint(ui, Rect::from_center_size(rect.center(), vec2(17.0, 17.0)), icon, col);
    if hovered {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    resp.on_hover_text(tip)
}

/// Knopf mit Icon und Text.
pub fn text_button(ui: &mut Ui, icon: Icon, text: &str, fill: Color32, fg: Color32) -> Response {
    let img = image(ui.ctx(), icon, 15.0, fg);
    ui.add(egui::Button::image_and_text(img, RichText::new(text).color(fg).strong()).fill(fill).min_size(vec2(0.0, 30.0)))
}

/// Menüeintrag mit Icon (für Kontextmenüs).
pub fn menu(ui: &mut Ui, icon: Icon, text: &str) -> Response {
    menu_colored(ui, icon, text, ui.visuals().text_color())
}

pub fn menu_colored(ui: &mut Ui, icon: Icon, text: &str, color: Color32) -> Response {
    let img = image(ui.ctx(), icon, 15.0, color);
    ui.add(egui::Button::image_and_text(img, RichText::new(text).color(color)))
}
