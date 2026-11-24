//! Низькорівнева графіка: заокруглені прямокутники, текст, іконки, розмиття.

use ab_glyph::{point, Font, FontArc, PxScale, ScaleFont};
use resvg::tiny_skia::{Paint, Path, PathBuilder, Pixmap, PixmapMut, Transform};
use std::process::Command;

pub type Rgba = [u8; 4];

pub fn paint(c: Rgba) -> Paint<'static> {
    let mut p = Paint::default();
    p.set_color_rgba8(c[0], c[1], c[2], c[3]);
    p.anti_alias = true;
    p
}

pub fn rrect(x: f32, y: f32, w: f32, h: f32, r: f32) -> Path {
    let r = r.min(w / 2.0).min(h / 2.0).max(0.0);
    let k = 0.5523 * r;
    let mut pb = PathBuilder::new();
    pb.move_to(x + r, y);
    pb.line_to(x + w - r, y);
    pb.cubic_to(x + w - r + k, y, x + w, y + r - k, x + w, y + r);
    pb.line_to(x + w, y + h - r);
    pb.cubic_to(x + w, y + h - r + k, x + w - r + k, y + h, x + w - r, y + h);
    pb.line_to(x + r, y + h);
    pb.cubic_to(x + r - k, y + h, x, y + h - r + k, x, y + h - r);
    pb.line_to(x, y + r);
    pb.cubic_to(x, y + r - k, x + r - k, y, x + r, y);
    pb.close();
    pb.finish().unwrap()
}

pub fn circle(cx: f32, cy: f32, r: f32) -> Path {
    PathBuilder::from_circle(cx, cy, r.max(0.1)).unwrap()
}

/// Змішування одного пікселя (premultiplied RGBA) з покриттям `a` (0..1).
#[inline]
pub fn blend_px(data: &mut [u8], idx: usize, c: Rgba, a: f32) {
    let a = (a * c[3] as f32 / 255.0).clamp(0.0, 1.0);
    if a <= 0.0 {
        return;
    }
    let inv = 1.0 - a;
    for ch in 0..3 {
        data[idx + ch] = (c[ch] as f32 * a + data[idx + ch] as f32 * inv + 0.5) as u8;
    }
    data[idx + 3] = (255.0 * a + data[idx + 3] as f32 * inv + 0.5) as u8;
}

// ---------- шрифти ----------

pub struct Fonts {
    pub regular: FontArc,
    pub medium: FontArc,
}

fn fc_file(pattern: &str) -> Option<Vec<u8>> {
    let out = Command::new("fc-match").args(["-f", "%{file}", pattern]).output().ok()?;
    let path = String::from_utf8(out.stdout).ok()?;
    std::fs::read(path.trim()).ok()
}

impl Fonts {
    pub fn load() -> Fonts {
        let load = |pats: &[&str]| -> FontArc {
            for p in pats {
                if let Some(bytes) = fc_file(p) {
                    if let Ok(f) = FontArc::try_from_vec(bytes) {
                        return f;
                    }
                }
            }
            panic!("не знайдено жодного шрифту (fc-match)");
        };
        Fonts {
            regular: load(&["Fira Sans:style=Regular", "sans-serif"]),
            medium: load(&["Fira Sans:style=Medium", "Fira Sans:weight=medium", "sans-serif:bold"]),
        }
    }
}

pub fn text_width(font: &FontArc, size: f32, s: &str) -> f32 {
    let sf = font.as_scaled(PxScale::from(size));
    let mut w = 0.0;
    let mut prev = None;
    for ch in s.chars() {
        let id = sf.glyph_id(ch);
        if let Some(p) = prev {
            w += sf.kern(p, id);
        }
        w += sf.h_advance(id);
        prev = Some(id);
    }
    w
}

/// Малює рядок тексту; `y` — базова лінія. Повертає ширину.
pub fn draw_text(pm: &mut PixmapMut, font: &FontArc, size: f32, x: f32, y: f32, c: Rgba, s: &str) -> f32 {
    let (w, h) = (pm.width() as i32, pm.height() as i32);
    let data = pm.data_mut();
    let sf = font.as_scaled(PxScale::from(size));
    let mut caret = x;
    let mut prev = None;
    for ch in s.chars() {
        let id = sf.glyph_id(ch);
        if let Some(p) = prev {
            caret += sf.kern(p, id);
        }
        let g = id.with_scale_and_position(PxScale::from(size), point(caret, y));
        if let Some(og) = font.outline_glyph(g) {
            let b = og.px_bounds();
            og.draw(|gx, gy, cov| {
                let px = b.min.x as i32 + gx as i32;
                let py = b.min.y as i32 + gy as i32;
                if px >= 0 && py >= 0 && px < w && py < h {
                    blend_px(data, ((py * w + px) * 4) as usize, c, cov);
                }
            });
        }
        caret += sf.h_advance(id);
        prev = Some(id);
    }
    caret - x
}

// ---------- іконки (Lucide, stroke 1.75) ----------

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Icon {
    Move,
    Pen,
    Line,
    Arrow,
    Rect,
    Marker,
    Text,
    Pixel,
    Undo,
    Save,
    Copy,
    Close,
}

fn icon_svg(i: Icon) -> &'static str {
    match i {
        Icon::Move => r#"<path d="M5 9l-3 3 3 3M9 5l3-3 3 3M15 19l-3 3-3-3M19 9l3 3-3 3M2 12h20M12 2v20"/>"#,
        Icon::Pen => r#"<path d="M21.174 6.812a1 1 0 0 0-3.986-3.987L3.842 16.174a2 2 0 0 0-.5.83l-1.321 4.352a.5.5 0 0 0 .623.622l4.353-1.32a2 2 0 0 0 .83-.497z"/><path d="m15 5 4 4"/>"#,
        Icon::Line => r#"<path d="M5 19 19 5"/>"#,
        Icon::Arrow => r#"<path d="M7 17 17 7"/><path d="M7 7h10v10"/>"#,
        Icon::Rect => r#"<rect width="18" height="18" x="3" y="3" rx="2"/>"#,
        Icon::Marker => r#"<path d="m9 11-6 6v3h9l3-3"/><path d="m22 12-4.6 4.6a2 2 0 0 1-2.8 0l-5.2-5.2a2 2 0 0 1 0-2.8L14 4"/>"#,
        Icon::Text => r#"<path d="M12 4v16"/><path d="M4 7V5a1 1 0 0 1 1-1h14a1 1 0 0 1 1 1v2"/><path d="M9 20h6"/>"#,
        Icon::Pixel => r#"<rect width="18" height="18" x="3" y="3" rx="2"/><path d="M3 9h18M3 15h18M9 3v18M15 3v18"/>"#,
        Icon::Undo => r#"<path d="M9 14 4 9l5-5"/><path d="M4 9h10.5a5.5 5.5 0 0 1 5.5 5.5 5.5 5.5 0 0 1-5.5 5.5H11"/>"#,
        Icon::Save => r#"<path d="M21 15v4a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2v-4"/><path d="m7 10 5 5 5-5"/><path d="M12 15V3"/>"#,
        Icon::Copy => r#"<rect width="14" height="14" x="8" y="8" rx="2"/><path d="M4 16c-1.1 0-2-.9-2-2V4c0-1.1.9-2 2-2h10c1.1 0 2 .9 2 2"/>"#,
        Icon::Close => r#"<path d="M18 6 6 18"/><path d="m6 6 12 12"/>"#,
    }
}

pub fn render_icon(i: Icon, size: u32, c: Rgba) -> Pixmap {
    let svg = format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 24 24" width="{size}" height="{size}" fill="none" stroke="#{:02x}{:02x}{:02x}" stroke-width="1.75" stroke-linecap="round" stroke-linejoin="round">{}</svg>"##,
        c[0], c[1], c[2], icon_svg(i)
    );
    let tree = resvg::usvg::Tree::from_str(&svg, &resvg::usvg::Options::default()).expect("icon svg");
    let mut pm = Pixmap::new(size, size).unwrap();
    let s = size as f32 / 24.0;
    resvg::render(&tree, Transform::from_scale(s, s), &mut pm.as_mut());
    pm
}

// ---------- обробка зображень ----------

/// Зменшити в `f` разів і розмити — основа «матового скла».
pub fn blur_small(src: &Pixmap, f: u32) -> Pixmap {
    let (sw, sh) = (src.width(), src.height());
    let (w, h) = ((sw / f).max(1), (sh / f).max(1));
    let s = src.data();
    let mut buf = vec![[0f32; 3]; (w * h) as usize];
    for y in 0..h {
        for x in 0..w {
            let mut acc = [0f32; 3];
            let mut n = 0.0;
            for yy in (y * f)..((y + 1) * f).min(sh) {
                for xx in (x * f)..((x + 1) * f).min(sw) {
                    let i = ((yy * sw + xx) * 4) as usize;
                    acc[0] += s[i] as f32;
                    acc[1] += s[i + 1] as f32;
                    acc[2] += s[i + 2] as f32;
                    n += 1.0;
                }
            }
            buf[(y * w + x) as usize] = [acc[0] / n, acc[1] / n, acc[2] / n];
        }
    }
    let r = 3i32;
    for _ in 0..3 {
        let mut tmp = buf.clone();
        for y in 0..h as i32 {
            for x in 0..w as i32 {
                let mut a = [0f32; 3];
                for d in -r..=r {
                    let xx = (x + d).clamp(0, w as i32 - 1);
                    let p = buf[(y * w as i32 + xx) as usize];
                    a[0] += p[0];
                    a[1] += p[1];
                    a[2] += p[2];
                }
                let n = (2 * r + 1) as f32;
                tmp[(y * w as i32 + x) as usize] = [a[0] / n, a[1] / n, a[2] / n];
            }
        }
        for y in 0..h as i32 {
            for x in 0..w as i32 {
                let mut a = [0f32; 3];
                for d in -r..=r {
                    let yy = (y + d).clamp(0, h as i32 - 1);
                    let p = tmp[(yy * w as i32 + x) as usize];
                    a[0] += p[0];
                    a[1] += p[1];
                    a[2] += p[2];
                }
                let n = (2 * r + 1) as f32;
                buf[(y * w as i32 + x) as usize] = [a[0] / n, a[1] / n, a[2] / n];
            }
        }
    }
    let mut out = Pixmap::new(w, h).unwrap();
    for (i, px) in out.data_mut().chunks_exact_mut(4).enumerate() {
        let p = buf[i];
        px[0] = p[0] as u8;
        px[1] = p[1] as u8;
        px[2] = p[2] as u8;
        px[3] = 255;
    }
    out
}

pub fn dim(src: &Pixmap, k: f32) -> Pixmap {
    let mut out = src.clone();
    for px in out.data_mut().chunks_exact_mut(4) {
        px[0] = (px[0] as f32 * k) as u8;
        px[1] = (px[1] as f32 * k) as u8;
        px[2] = (px[2] as f32 * k) as u8;
    }
    out
}

pub fn read_png(path: &str) -> Pixmap {
    let dec = png::Decoder::new(std::io::BufReader::new(std::fs::File::open(path).expect("png")));
    let mut r = dec.read_info().unwrap();
    let mut buf = vec![0; r.output_buffer_size().unwrap()];
    let info = r.next_frame(&mut buf).unwrap();
    let (w, h) = (info.width, info.height);
    let mut pm = Pixmap::new(w, h).unwrap();
    let ch = info.color_type.samples();
    for (i, px) in pm.data_mut().chunks_exact_mut(4).enumerate() {
        let s = &buf[i * ch..];
        px[0] = s[0];
        px[1] = s[1];
        px[2] = s[2];
        px[3] = 255;
    }
    pm
}

pub fn encode_png(pm: &Pixmap) -> Vec<u8> {
    let mut out = Vec::new();
    {
        let mut enc = png::Encoder::new(&mut out, pm.width(), pm.height());
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        enc.set_compression(png::Compression::Fast);
        let mut w = enc.write_header().unwrap();
        w.write_image_data(pm.data()).unwrap();
    }
    out
}
