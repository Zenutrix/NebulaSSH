//! Farbschemata für das Terminal.

use eframe::egui::Color32;

#[derive(Clone)]
pub struct Theme {
    pub id: &'static str,
    pub name: &'static str,
    pub bg: Color32,
    pub fg: Color32,
    pub cursor: Color32,
    pub selection: Color32,
    pub ansi: [Color32; 16],
}

const fn c(hex: u32) -> Color32 {
    Color32::from_rgb((hex >> 16) as u8, (hex >> 8) as u8, hex as u8)
}

pub const THEMES: &[Theme] = &[
    Theme {
        id: "nebula",
        name: "Nebula (Standard)",
        bg: c(0x0f0f17),
        fg: c(0xecebf3),
        cursor: c(0x7c22ff),
        selection: c(0x3d1f7a),
        ansi: [
            c(0x1e1e2d), c(0xff5c7c), c(0x7ee08a), c(0xffd46b), c(0x6c9eff), c(0xc38bff), c(0x5cd8e6), c(0xd8d7e3),
            c(0x6b6a80), c(0xff859d), c(0xa4f0ad), c(0xffe299), c(0x9bbdff), c(0xd9b6ff), c(0x8fe9f2), c(0xffffff),
        ],
    },
    Theme {
        id: "dracula",
        name: "Dracula",
        bg: c(0x282a36),
        fg: c(0xf8f8f2),
        cursor: c(0xf8f8f2),
        selection: c(0x44475a),
        ansi: [
            c(0x21222c), c(0xff5555), c(0x50fa7b), c(0xf1fa8c), c(0xbd93f9), c(0xff79c6), c(0x8be9fd), c(0xf8f8f2),
            c(0x6272a4), c(0xff6e6e), c(0x69ff94), c(0xffffa5), c(0xd6acff), c(0xff92df), c(0xa4ffff), c(0xffffff),
        ],
    },
    Theme {
        id: "solarized",
        name: "Solarized Dark",
        bg: c(0x002b36),
        fg: c(0x93a1a1),
        cursor: c(0x93a1a1),
        selection: c(0x274642),
        ansi: [
            c(0x073642), c(0xdc322f), c(0x859900), c(0xb58900), c(0x268bd2), c(0xd33682), c(0x2aa198), c(0xeee8d5),
            c(0x586e75), c(0xcb4b16), c(0x93a1a1), c(0x839496), c(0x657b83), c(0x6c71c4), c(0x93a1a1), c(0xfdf6e3),
        ],
    },
    Theme {
        id: "matrix",
        name: "Konsole Grün",
        bg: c(0x0a0f0a),
        fg: c(0x7dff8a),
        cursor: c(0x7dff8a),
        selection: c(0x1f4d25),
        ansi: [
            c(0x0a0f0a), c(0xff6b6b), c(0x5cff70), c(0xd8ff5c), c(0x4dd2a0), c(0x9cff9c), c(0x6bffd8), c(0xb8ffbf),
            c(0x2e5a33), c(0xff8f8f), c(0x8cff99), c(0xe6ff8c), c(0x80e6bf), c(0xbfffbf), c(0x99ffe6), c(0xe6ffe8),
        ],
    },
    Theme {
        id: "light",
        name: "Hell",
        bg: c(0xfbfbfd),
        fg: c(0x24242e),
        cursor: c(0x6200ee),
        selection: c(0xd9ccff),
        ansi: [
            c(0x24242e), c(0xc8183c), c(0x1e8a3a), c(0x9a6b00), c(0x2457c5), c(0x8a2bd8), c(0x0a7f8c), c(0xbdbdc8),
            c(0x6b6a80), c(0xe0325a), c(0x2aa64d), c(0xb88200), c(0x3b6fe0), c(0xa34ef0), c(0x139aa8), c(0x404050),
        ],
    },
];

pub fn by_id(id: &str) -> &'static Theme {
    THEMES.iter().find(|t| t.id == id).unwrap_or(&THEMES[0])
}
