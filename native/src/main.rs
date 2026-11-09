// Kein Konsolenfenster im Release-Build
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod clipboard;
mod data;
mod dialogs;
mod files;
mod sidebar;
mod terminal;
mod theme;
mod updater;

use eframe::egui;
use std::sync::Arc;

fn load_font(families: &mut egui::FontDefinitions, name: &str, files: &[&str]) -> bool {
    let dir = std::env::var("WINDIR").unwrap_or_else(|_| "C:\\Windows".into());
    for f in files {
        if let Ok(bytes) = std::fs::read(format!("{dir}\\Fonts\\{f}")) {
            families
                .font_data
                .insert(name.to_owned(), Arc::new(egui::FontData::from_owned(bytes)));
            return true;
        }
    }
    false
}

fn setup_fonts(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    let mono = load_font(&mut fonts, "mono", &["CascadiaMono.ttf", "consola.ttf"]);
    let mono_b = load_font(&mut fonts, "mono-b", &["consolab.ttf", "CascadiaMono.ttf"]);
    let ui = load_font(&mut fonts, "ui", &["segoeui.ttf"]);
    let ui_b = load_font(&mut fonts, "ui-b", &["segoeuib.ttf", "seguisb.ttf"]);
    let sym = load_font(&mut fonts, "sym", &["seguisym.ttf"]);

    let fam = &mut fonts.families;
    let defaults_mono = fam.get(&egui::FontFamily::Monospace).cloned().unwrap_or_default();
    let defaults_prop = fam.get(&egui::FontFamily::Proportional).cloned().unwrap_or_default();

    let mut m = Vec::new();
    if mono {
        m.push("mono".to_string());
    }
    if sym {
        m.push("sym".to_string());
    }
    m.extend(defaults_mono.clone());
    fam.insert(egui::FontFamily::Monospace, m.clone());

    let mut mb = Vec::new();
    if mono_b {
        mb.push("mono-b".to_string());
    }
    mb.extend(m);
    fam.insert(egui::FontFamily::Name("mono-bold".into()), mb);

    let mut p = Vec::new();
    if ui {
        p.push("ui".to_string());
    }
    if sym {
        p.push("sym".to_string());
    }
    p.extend(defaults_prop.clone());
    fam.insert(egui::FontFamily::Proportional, p.clone());

    let mut pb = Vec::new();
    if ui_b {
        pb.push("ui-b".to_string());
    }
    pb.extend(p);
    fam.insert(egui::FontFamily::Name("ui-bold".into()), pb);

    ctx.set_fonts(fonts);
}

fn setup_style(ctx: &egui::Context) {
    use egui::{Color32, CornerRadius, Stroke};
    ctx.set_visuals(egui::Visuals::dark());
    ctx.global_style_mut(|style| {
        let v = &mut style.visuals;
        v.panel_fill = Color32::from_rgb(0x16, 0x16, 0x21);
        v.window_fill = Color32::from_rgb(0x1a, 0x1a, 0x26);
        v.extreme_bg_color = Color32::from_rgb(0x23, 0x23, 0x33);
        v.faint_bg_color = Color32::from_rgb(0x1e, 0x1e, 0x2d);
        v.selection.bg_fill = Color32::from_rgb(0x62, 0x00, 0xee);
        v.selection.stroke = Stroke::new(1.0, Color32::WHITE);
        v.hyperlink_color = Color32::from_rgb(0xb9, 0xa4, 0xff);
        let r = CornerRadius::same(7);
        let w = &mut v.widgets;
        w.noninteractive.bg_stroke = Stroke::new(1.0, Color32::from_rgb(0x2e, 0x2e, 0x3e));
        w.noninteractive.fg_stroke = Stroke::new(1.0, Color32::from_rgb(0xdd, 0xdd, 0xe6));
        w.inactive.bg_fill = Color32::from_rgb(0x2e, 0x2e, 0x45);
        w.inactive.weak_bg_fill = Color32::from_rgb(0x2e, 0x2e, 0x45);
        w.inactive.bg_stroke = Stroke::new(1.0, Color32::from_rgb(0x3a, 0x3a, 0x4e));
        w.inactive.fg_stroke = Stroke::new(1.0, Color32::from_rgb(0xe8, 0xe8, 0xf0));
        w.hovered.bg_fill = Color32::from_rgb(0x3a, 0x3a, 0x5e);
        w.hovered.weak_bg_fill = Color32::from_rgb(0x3a, 0x3a, 0x5e);
        w.hovered.bg_stroke = Stroke::new(1.0, Color32::from_rgb(0x7c, 0x22, 0xff));
        w.active.bg_fill = Color32::from_rgb(0x62, 0x00, 0xee);
        w.active.weak_bg_fill = Color32::from_rgb(0x62, 0x00, 0xee);
        w.open.weak_bg_fill = Color32::from_rgb(0x2e, 0x2e, 0x45);
        for s in [&mut w.noninteractive, &mut w.inactive, &mut w.hovered, &mut w.active, &mut w.open] {
            s.corner_radius = r;
        }
        style.spacing.item_spacing = egui::vec2(6.0, 6.0);
        style.spacing.scroll = egui::style::ScrollStyle::thin();
        v.window_corner_radius = CornerRadius::same(12);
        v.menu_corner_radius = CornerRadius::same(10);
        v.popup_shadow = egui::Shadow { offset: [0, 8], blur: 24, spread: 0, color: Color32::from_black_alpha(150) };
        style.spacing.button_padding = egui::vec2(10.0, 5.0);
        style.spacing.interact_size.y = 28.0;
        style.text_styles.insert(egui::TextStyle::Body, egui::FontId::proportional(14.0));
        style.text_styles.insert(egui::TextStyle::Button, egui::FontId::proportional(14.0));
        style.text_styles.insert(egui::TextStyle::Monospace, egui::FontId::monospace(13.5));
        style.text_styles.insert(
            egui::TextStyle::Heading,
            egui::FontId::new(19.0, egui::FontFamily::Name("ui-bold".into())),
        );
    });
}

fn icon() -> egui::IconData {
    let img = image::load_from_memory(include_bytes!("../assets/128x128.png"))
        .expect("Icon")
        .into_rgba8();
    let (w, h) = img.dimensions();
    egui::IconData {
        rgba: img.into_raw(),
        width: w,
        height: h,
    }
}

fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("NebulaSSH")
            .with_inner_size([1240.0, 800.0])
            .with_min_inner_size([860.0, 520.0])
            .with_icon(Arc::new(icon()))
            .with_drag_and_drop(true),
        renderer: eframe::Renderer::Glow,
        ..Default::default()
    };
    eframe::run_native(
        "NebulaSSH",
        options,
        Box::new(|cc| {
            setup_fonts(&cc.egui_ctx);
            setup_style(&cc.egui_ctx);
            Ok(Box::new(app::App::new(cc)))
        }),
    )
}
