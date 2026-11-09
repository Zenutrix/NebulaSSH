//! cargo run -p brand  → erzeugt alle Grafiken in branding/out
use image::{codecs::ico::{IcoEncoder, IcoFrame}, ExtendedColorType, ImageBuffer, Rgba, RgbImage};
use std::path::Path;

fn render(svg: &str, w: u32, h: u32, opt: &resvg::usvg::Options) -> ImageBuffer<Rgba<u8>, Vec<u8>> {
    let tree = resvg::usvg::Tree::from_str(svg, opt).expect("SVG");
    let mut pix = resvg::tiny_skia::Pixmap::new(w, h).unwrap();
    let s = tree.size();
    let tf = resvg::tiny_skia::Transform::from_scale(w as f32 / s.width(), h as f32 / s.height());
    resvg::render(&tree, tf, &mut pix.as_mut());
    // tiny-skia liefert vormultipliziertes Alpha
    let mut img = ImageBuffer::from_raw(w, h, pix.take()).unwrap();
    for p in img.pixels_mut() {
        let Rgba([r, g, b, a]) = *p;
        if a > 0 && a < 255 {
            let f = 255.0 / a as f32;
            *p = Rgba([(r as f32 * f).min(255.0) as u8, (g as f32 * f).min(255.0) as u8, (b as f32 * f).min(255.0) as u8, a]);
        }
    }
    img
}

fn flatten(img: &ImageBuffer<Rgba<u8>, Vec<u8>>, bg: [u8; 3]) -> RgbImage {
    RgbImage::from_fn(img.width(), img.height(), |x, y| {
        let Rgba([r, g, b, a]) = *img.get_pixel(x, y);
        let a = a as f32 / 255.0;
        let m = |c: u8, k: u8| (c as f32 * a + k as f32 * (1.0 - a)) as u8;
        image::Rgb([m(r, bg[0]), m(g, bg[1]), m(b, bg[2])])
    })
}

fn main() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../branding");
    let out = root.join("out");
    std::fs::create_dir_all(&out).unwrap();
    let mut opt = resvg::usvg::Options::default();
    opt.fontdb_mut().load_system_fonts();
    let logo = std::fs::read_to_string(root.join("logo.svg")).unwrap();

    for s in [16, 24, 32, 48, 64, 128, 256, 512] {
        render(&logo, s, s, &opt).save(out.join(format!("icon-{s}.png"))).unwrap();
    }
    let frames: Vec<IcoFrame> = [16, 24, 32, 48, 64, 128, 256]
        .iter()
        .map(|&s| {
            let img = render(&logo, s, s, &opt);
            IcoFrame::as_png(img.as_raw(), s, s, ExtendedColorType::Rgba8).unwrap()
        })
        .collect();
    IcoEncoder::new(std::fs::File::create(out.join("icon.ico")).unwrap()).encode_images(&frames).unwrap();

    // Installer-Grafiken (NSIS Modern UI): Willkommensseite 164x314, Kopfzeile 150x57
    for (name, w, h) in [("installer-welcome", 164, 314), ("installer-header", 150, 57)] {
        let svg = std::fs::read_to_string(root.join(format!("{name}.svg"))).unwrap();
        let img = render(&svg, w, h, &opt);
        flatten(&img, [255, 255, 255]).save(out.join(format!("{name}.bmp"))).unwrap();
        img.save(out.join(format!("{name}.png"))).unwrap();
    }
    println!("Grafiken in {}", out.display());
}
