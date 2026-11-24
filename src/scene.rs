//! Стан оверлею (виділення, інструменти, фігури), розкладка панелі, рендер і експорт.
//! Усі координати — у пікселях буфера конкретного монітора.

use crate::gfx::{self, circle, paint, rrect, Fonts, Icon, Rgba};
use crate::theme::Theme;
use resvg::tiny_skia::{
    FillRule, FilterQuality, LineCap, LineJoin, Mask, Paint, PathBuilder, Pattern, Pixmap, PixmapMut,
    Rect, SpreadMode, Stroke, Transform,
};
use std::collections::HashMap;

pub const PALETTE: [Rgba; 8] = [
    [0xFF, 0x5A, 0x5F, 255],
    [0xFF, 0x9F, 0x43, 255],
    [0xFF, 0xD2, 0x3F, 255],
    [0x3D, 0xDC, 0x84, 255],
    [0x63, 0xD0, 0xDF, 255],
    [0x7C, 0x8C, 0xFF, 255],
    [0xFF, 0xFF, 0xFF, 255],
    [0x1B, 0x1D, 0x21, 255],
];
pub const WIDTHS: [f32; 3] = [2.0, 4.0, 7.0];
const TEXT_SIZES: [f32; 3] = [18.0, 26.0, 38.0];

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Tool {
    Move,
    Pen,
    Line,
    Arrow,
    Rect,
    Marker,
    Text,
    Pixel,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Item {
    Tool(Tool),
    Color,
    Undo,
    Save,
    Copy,
    Close,
    Sep,
}

pub const ITEMS: &[Item] = &[
    Item::Tool(Tool::Move),
    Item::Sep,
    Item::Tool(Tool::Pen),
    Item::Tool(Tool::Line),
    Item::Tool(Tool::Arrow),
    Item::Tool(Tool::Rect),
    Item::Tool(Tool::Marker),
    Item::Tool(Tool::Text),
    Item::Tool(Tool::Pixel),
    Item::Sep,
    Item::Color,
    Item::Undo,
    Item::Sep,
    Item::Save,
    Item::Copy,
    Item::Close,
];

fn item_icon(it: Item) -> Option<Icon> {
    Some(match it {
        Item::Tool(Tool::Move) => Icon::Move,
        Item::Tool(Tool::Pen) => Icon::Pen,
        Item::Tool(Tool::Line) => Icon::Line,
        Item::Tool(Tool::Arrow) => Icon::Arrow,
        Item::Tool(Tool::Rect) => Icon::Rect,
        Item::Tool(Tool::Marker) => Icon::Marker,
        Item::Tool(Tool::Text) => Icon::Text,
        Item::Tool(Tool::Pixel) => Icon::Pixel,
        Item::Undo => Icon::Undo,
        Item::Save => Icon::Save,
        Item::Copy => Icon::Copy,
        Item::Close => Icon::Close,
        _ => return None,
    })
}

/// Мова інтерфейсу: `JAMSHOT_LANG`, інакше `~/.config/jamshot/lang` (uk/en), інакше `LANG`.
pub fn ukrainian() -> bool {
    static UK: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *UK.get_or_init(|| {
        let home = std::env::var("HOME").unwrap_or_default();
        let lang = std::env::var("JAMSHOT_LANG")
            .ok()
            .or_else(|| std::fs::read_to_string(format!("{home}/.config/jamshot/lang")).ok())
            .or_else(|| std::env::var("LC_ALL").ok().filter(|v| !v.is_empty()))
            .or_else(|| std::env::var("LANG").ok())
            .unwrap_or_default();
        lang.trim().starts_with("uk")
    })
}

fn item_tip(it: Item) -> (&'static str, &'static str) {
    let uk = ukrainian();
    let t = |u: &'static str, e: &'static str| if uk { u } else { e };
    match it {
        Item::Tool(Tool::Move) => (t("Перемістити рамку", "Move selection"), "V"),
        Item::Tool(Tool::Pen) => (t("Олівець", "Pen"), "P"),
        Item::Tool(Tool::Line) => (t("Лінія", "Line"), "L"),
        Item::Tool(Tool::Arrow) => (t("Стрілка", "Arrow"), "A"),
        Item::Tool(Tool::Rect) => (t("Рамка", "Rectangle"), "R"),
        Item::Tool(Tool::Marker) => (t("Маркер", "Highlighter"), "M"),
        Item::Tool(Tool::Text) => (t("Текст", "Text"), "T"),
        Item::Tool(Tool::Pixel) => (t("Пікселі", "Pixelate"), "B"),
        Item::Color => (t("Колір і товщина", "Color & size"), "C"),
        Item::Undo => (t("Скасувати", "Undo"), "Ctrl Z"),
        Item::Save => (t("Зберегти", "Save"), "Ctrl S"),
        Item::Copy => (t("Копіювати", "Copy"), "Ctrl C"),
        Item::Close => (t("Закрити", "Close"), "Esc"),
        Item::Sep => ("", ""),
    }
}

pub type P = (f32, f32);

#[derive(Clone, Debug)]
pub enum Shape {
    Pen { pts: Vec<P>, c: Rgba, w: f32 },
    Marker { pts: Vec<P>, c: Rgba, w: f32 },
    Line { a: P, b: P, c: Rgba, w: f32 },
    Arrow { a: P, b: P, c: Rgba, w: f32 },
    Rect { a: P, b: P, c: Rgba, w: f32 },
    Text { pos: P, text: String, c: Rgba, size: f32 },
    Pixel { a: P, b: P, block: f32 },
}

#[derive(Clone, Copy, Debug)]
pub struct R {
    pub x0: f32,
    pub y0: f32,
    pub x1: f32,
    pub y1: f32,
}

impl R {
    pub fn norm(&self) -> R {
        R { x0: self.x0.min(self.x1), y0: self.y0.min(self.y1), x1: self.x0.max(self.x1), y1: self.y0.max(self.y1) }
    }
    pub fn w(&self) -> f32 {
        self.x1 - self.x0
    }
    pub fn h(&self) -> f32 {
        self.y1 - self.y0
    }
    pub fn contains(&self, p: P) -> bool {
        p.0 >= self.x0 && p.0 <= self.x1 && p.1 >= self.y0 && p.1 <= self.y1
    }
    fn handles(&self) -> [P; 8] {
        let (cx, cy) = ((self.x0 + self.x1) / 2.0, (self.y0 + self.y1) / 2.0);
        [
            (self.x0, self.y0),
            (cx, self.y0),
            (self.x1, self.y0),
            (self.x1, cy),
            (self.x1, self.y1),
            (cx, self.y1),
            (self.x0, self.y1),
            (self.x0, cy),
        ]
    }
}

#[derive(Clone, Copy, Debug)]
pub enum Drag {
    None,
    Selecting,
    Moving { dx: f32, dy: f32 },
    Resizing { h: usize },
    Drawing,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Action {
    None,
    Copy,
    Save,
    Exit,
    Redraw,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Cursor {
    Crosshair,
    Default,
    Move,
    Text,
    Resize(usize),
}

pub struct Screen {
    pub orig: Pixmap,
    pub dimmed: Pixmap,
    pub blur: Pixmap,
    /// Масштаб інтерфейсу (пікселі буфера на логічний піксель).
    pub k: f32,
}

impl Screen {
    pub fn new(orig: Pixmap, k: f32) -> Screen {
        let dimmed = gfx::dim(&orig, 0.6);
        let blur = gfx::blur_small(&orig, 8);
        Screen { orig, dimmed, blur, k }
    }
    pub fn w(&self) -> f32 {
        self.orig.width() as f32
    }
    pub fn h(&self) -> f32 {
        self.orig.height() as f32
    }
}

pub struct Layout {
    pub bar: R,
    pub items: Vec<(Item, f32, f32)>, // item, x, width
    pub btn: f32,
    pub pop: Option<R>,
    pub bar_below: bool,
}

pub struct Icons {
    cache: HashMap<(Icon, u8), Pixmap>,
}

impl Icons {
    pub fn new(theme: &Theme, size: u32) -> Icons {
        let mut cache = HashMap::new();
        let all = [
            Icon::Move, Icon::Pen, Icon::Line, Icon::Arrow, Icon::Rect, Icon::Marker, Icon::Text, Icon::Pixel,
            Icon::Undo, Icon::Save, Icon::Copy, Icon::Close,
        ];
        for i in all {
            cache.insert((i, 0), gfx::render_icon(i, size, theme.ink_dim()));
            cache.insert((i, 1), gfx::render_icon(i, size, theme.ink()));
            cache.insert((i, 2), gfx::render_icon(i, size, theme.on_accent()));
        }
        Icons { cache }
    }
    fn get(&self, i: Icon, v: u8) -> &Pixmap {
        &self.cache[&(i, v)]
    }
}

pub struct Scene {
    pub sel: Option<R>,
    pub sel_out: usize,
    pub tool: Tool,
    pub color: usize,
    pub width: usize,
    pub shapes: Vec<Shape>,
    pub drawing: Option<Shape>,
    pub text_edit: Option<(P, String)>,
    pub drag: Drag,
    pub popover: bool,
    pub hover: Option<usize>,
    pub tooltip: bool,
    pub cursor: P,
    pub cursor_out: usize,
}

impl Scene {
    pub fn new() -> Scene {
        Scene {
            sel: None,
            sel_out: 0,
            tool: Tool::Move,
            color: 0,
            width: 1,
            shapes: vec![],
            drawing: None,
            text_edit: None,
            drag: Drag::None,
            popover: false,
            hover: None,
            tooltip: false,
            cursor: (0.0, 0.0),
            cursor_out: 0,
        }
    }

    fn col(&self) -> Rgba {
        PALETTE[self.color]
    }

    fn text_size(&self, k: f32) -> f32 {
        TEXT_SIZES[self.width] * k
    }

    // ---------- розкладка ----------

    pub fn layout(&self, sw: f32, sh: f32, k: f32) -> Option<Layout> {
        let sel = self.sel?.norm();
        let (btn, gap, pad, sep) = (36.0 * k, 2.0 * k, 5.0 * k, 11.0 * k);
        let mut items = vec![];
        let mut x = pad;
        for (i, it) in ITEMS.iter().enumerate() {
            if i > 0 {
                x += gap;
            }
            let w = if *it == Item::Sep { sep } else { btn };
            items.push((*it, x, w));
            x += w;
        }
        let bw = x + pad;
        let bh = btn + pad * 2.0;
        let m = 8.0 * k;
        let off = 12.0 * k;
        let bx = (sel.x1 - bw).clamp(m, (sw - bw - m).max(m));
        let mut by = sel.y1 + off;
        let mut below = true;
        if by + bh > sh - m {
            by = sel.y0 - off - bh;
            below = false;
            if by < m {
                by = sel.y1 - bh - off;
            }
        }
        for it in items.iter_mut() {
            it.1 += bx;
        }
        let bar = R { x0: bx, y0: by, x1: bx + bw, y1: by + bh };
        let pop = if self.popover {
            let cx = items.iter().find(|i| i.0 == Item::Color).map(|i| i.1).unwrap_or(bx);
            let (pw, ph) = (236.0 * k, 90.0 * k);
            let px = (cx - 10.0 * k).clamp(m, sw - pw - m);
            let mut py = bar.y1 + 8.0 * k;
            if py + ph > sh - m {
                py = bar.y0 - 8.0 * k - ph;
            }
            Some(R { x0: px, y0: py, x1: px + pw, y1: py + ph })
        } else {
            None
        };
        Some(Layout { bar, items, btn, pop, bar_below: below })
    }

    fn pop_targets(pop: &R, k: f32) -> (Vec<R>, Vec<R>) {
        let mut sw = vec![];
        for i in 0..8 {
            let x = pop.x0 + 10.0 * k + i as f32 * 28.0 * k;
            sw.push(R { x0: x, y0: pop.y0 + 10.0 * k, x1: x + 20.0 * k, y1: pop.y0 + 30.0 * k });
        }
        let mut wd = vec![];
        let cw = (pop.w() - 20.0 * k - 12.0 * k) / 3.0;
        for i in 0..3 {
            let x = pop.x0 + 10.0 * k + i as f32 * (cw + 6.0 * k);
            wd.push(R { x0: x, y0: pop.y0 + 46.0 * k, x1: x + cw, y1: pop.y0 + 80.0 * k });
        }
        (sw, wd)
    }

    fn hit_handle(&self, p: P, k: f32) -> Option<usize> {
        let s = self.sel?.norm();
        s.handles().iter().position(|h| (h.0 - p.0).hypot(h.1 - p.1) <= 9.0 * k)
    }

    fn toolbar_visible(&self) -> bool {
        self.sel.is_some() && !matches!(self.drag, Drag::Selecting)
    }

    pub fn hit_toolbar(&self, o: usize, p: P, sw: f32, sh: f32, k: f32) -> Option<usize> {
        if o != self.sel_out || !self.toolbar_visible() {
            return None;
        }
        let l = self.layout(sw, sh, k)?;
        if !l.bar.contains(p) {
            return None;
        }
        l.items.iter().position(|(it, x, w)| *it != Item::Sep && p.0 >= *x && p.0 <= x + w)
    }

    pub fn cursor_kind(&self, o: usize, p: P, sw: f32, sh: f32, k: f32) -> Cursor {
        if o == self.sel_out && self.toolbar_visible() {
            if let Some(l) = self.layout(sw, sh, k) {
                if l.bar.contains(p) || l.pop.map_or(false, |r| r.contains(p)) {
                    return Cursor::Default;
                }
            }
        }
        match self.drag {
            Drag::Moving { .. } => return Cursor::Move,
            Drag::Resizing { h } => return Cursor::Resize(h),
            Drag::Selecting | Drag::Drawing => return Cursor::Crosshair,
            Drag::None => {}
        }
        if o == self.sel_out {
            if let Some(h) = self.hit_handle(p, k) {
                return Cursor::Resize(h);
            }
            if let Some(s) = self.sel {
                if s.norm().contains(p) {
                    return match self.tool {
                        Tool::Move => Cursor::Move,
                        Tool::Text => Cursor::Text,
                        _ => Cursor::Crosshair,
                    };
                }
            }
        }
        Cursor::Crosshair
    }

    // ---------- дії ----------

    pub fn commit_text(&mut self, k: f32) {
        if let Some((pos, text)) = self.text_edit.take() {
            if !text.trim().is_empty() {
                let size = self.text_size(k);
                self.shapes.push(Shape::Text { pos, text, c: self.col(), size });
            }
        }
    }

    pub fn undo(&mut self) {
        if self.text_edit.take().is_none() {
            self.shapes.pop();
        }
    }

    pub fn set_tool(&mut self, t: Tool, k: f32) {
        self.commit_text(k);
        self.tool = t;
    }

    pub fn press(&mut self, o: usize, p: P, sw: f32, sh: f32, k: f32) -> Action {
        self.cursor = p;
        self.cursor_out = o;
        // поповер кольору
        if self.popover && o == self.sel_out {
            if let Some(pop) = self.layout(sw, sh, k).and_then(|l| l.pop) {
                if pop.contains(p) {
                    let (sws, wds) = Self::pop_targets(&pop, k);
                    if let Some(i) = sws.iter().position(|r| r.contains(p)) {
                        self.color = i;
                    }
                    if let Some(i) = wds.iter().position(|r| r.contains(p)) {
                        self.width = i;
                    }
                    return Action::Redraw;
                }
            }
        }
        // панель
        if let Some(i) = self.hit_toolbar(o, p, sw, sh, k) {
            let it = ITEMS[i];
            if it != Item::Color {
                self.popover = false;
            }
            return match it {
                Item::Tool(t) => {
                    self.set_tool(t, k);
                    Action::Redraw
                }
                Item::Color => {
                    self.popover = !self.popover;
                    Action::Redraw
                }
                Item::Undo => {
                    self.undo();
                    Action::Redraw
                }
                Item::Save => Action::Save,
                Item::Copy => Action::Copy,
                Item::Close => Action::Exit,
                Item::Sep => Action::None,
            };
        }
        if self.popover {
            self.popover = false;
            return Action::Redraw;
        }
        self.commit_text(k);

        let same = self.sel.is_some() && o == self.sel_out;
        if same {
            if let Some(h) = self.hit_handle(p, k) {
                self.sel = self.sel.map(|s| s.norm());
                self.drag = Drag::Resizing { h };
                return Action::Redraw;
            }
            let s = self.sel.unwrap().norm();
            match self.tool {
                Tool::Move => {
                    if s.contains(p) {
                        self.drag = Drag::Moving { dx: p.0 - s.x0, dy: p.1 - s.y0 };
                        return Action::Redraw;
                    }
                }
                Tool::Text => {
                    self.text_edit = Some((p, String::new()));
                    return Action::Redraw;
                }
                t => {
                    let (c, w) = (self.col(), WIDTHS[self.width] * k);
                    self.drawing = Some(match t {
                        Tool::Pen => Shape::Pen { pts: vec![p], c, w },
                        Tool::Marker => Shape::Marker { pts: vec![p], c, w: w * 3.0 + 10.0 * k },
                        Tool::Line => Shape::Line { a: p, b: p, c, w },
                        Tool::Arrow => Shape::Arrow { a: p, b: p, c, w },
                        Tool::Rect => Shape::Rect { a: p, b: p, c, w },
                        _ => Shape::Pixel { a: p, b: p, block: (12.0 * k).round().max(4.0) },
                    });
                    self.drag = Drag::Drawing;
                    return Action::Redraw;
                }
            }
        }
        // нове виділення
        if !same {
            self.shapes.clear();
            self.tool = Tool::Move;
        }
        self.sel = Some(R { x0: p.0, y0: p.1, x1: p.0, y1: p.1 });
        self.sel_out = o;
        self.drag = Drag::Selecting;
        Action::Redraw
    }

    pub fn motion(&mut self, o: usize, p: P, sw: f32, sh: f32, k: f32) -> Action {
        self.cursor = p;
        self.cursor_out = o;
        let cl = |p: P| (p.0.clamp(0.0, sw), p.1.clamp(0.0, sh));
        match self.drag {
            Drag::Selecting if o == self.sel_out => {
                let q = cl(p);
                if let Some(s) = self.sel.as_mut() {
                    s.x1 = q.0;
                    s.y1 = q.1;
                }
                Action::Redraw
            }
            Drag::Moving { dx, dy } if o == self.sel_out => {
                if let Some(s) = self.sel.as_mut() {
                    let (w, h) = (s.w(), s.h());
                    s.x0 = (p.0 - dx).clamp(0.0, sw - w);
                    s.y0 = (p.1 - dy).clamp(0.0, sh - h);
                    s.x1 = s.x0 + w;
                    s.y1 = s.y0 + h;
                }
                Action::Redraw
            }
            Drag::Resizing { h } if o == self.sel_out => {
                let q = cl(p);
                if let Some(s) = self.sel.as_mut() {
                    if matches!(h, 0 | 6 | 7) {
                        s.x0 = q.0;
                    }
                    if matches!(h, 2 | 3 | 4) {
                        s.x1 = q.0;
                    }
                    if matches!(h, 0 | 1 | 2) {
                        s.y0 = q.1;
                    }
                    if matches!(h, 4 | 5 | 6) {
                        s.y1 = q.1;
                    }
                }
                Action::Redraw
            }
            Drag::Drawing if o == self.sel_out => {
                match self.drawing.as_mut() {
                    Some(Shape::Pen { pts, .. }) | Some(Shape::Marker { pts, .. }) => {
                        let l = *pts.last().unwrap();
                        if (l.0 - p.0).hypot(l.1 - p.1) >= 1.0 {
                            pts.push(p);
                        }
                    }
                    Some(Shape::Line { b, .. })
                    | Some(Shape::Arrow { b, .. })
                    | Some(Shape::Rect { b, .. })
                    | Some(Shape::Pixel { b, .. }) => *b = p,
                    _ => {}
                }
                Action::Redraw
            }
            _ => {
                let hv = self.hit_toolbar(o, p, sw, sh, k);
                if hv != self.hover {
                    self.hover = hv;
                    self.tooltip = false;
                    Action::Redraw
                } else {
                    Action::None
                }
            }
        }
    }

    pub fn release(&mut self, _o: usize, _p: P, k: f32) -> Action {
        let drag = std::mem::replace(&mut self.drag, Drag::None);
        match drag {
            Drag::Selecting => {
                if let Some(s) = self.sel {
                    let s = s.norm();
                    self.sel = if s.w() < 3.0 * k || s.h() < 3.0 * k { None } else { Some(s) };
                }
            }
            Drag::Moving { .. } | Drag::Resizing { .. } => self.sel = self.sel.map(|s| s.norm()),
            Drag::Drawing => {
                if let Some(sh) = self.drawing.take() {
                    let keep = match &sh {
                        Shape::Line { a, b, .. } | Shape::Arrow { a, b, .. } => (a.0 - b.0).hypot(a.1 - b.1) > 3.0,
                        Shape::Rect { a, b, .. } | Shape::Pixel { a, b, .. } => {
                            (a.0 - b.0).abs() > 2.0 && (a.1 - b.1).abs() > 2.0
                        }
                        _ => true,
                    };
                    if keep {
                        self.shapes.push(sh);
                    }
                }
            }
            Drag::None => return Action::None,
        }
        Action::Redraw
    }

    pub fn scroll(&mut self, dir: f64) -> Action {
        if self.sel.is_none() {
            return Action::None;
        }
        if dir < 0.0 && self.width < 2 {
            self.width += 1;
        } else if dir > 0.0 && self.width > 0 {
            self.width -= 1;
        } else {
            return Action::None;
        }
        Action::Redraw
    }

    // ---------- рендер ----------

    pub fn render(&self, o: usize, scr: &Screen, theme: &Theme, icons: &Icons, fonts: &Fonts, frame: &mut Pixmap) {
        frame.data_mut().copy_from_slice(scr.dimmed.data());
        let k = scr.k;
        let (sw, sh) = (scr.w(), scr.h());
        let Some(sel) = self.sel.filter(|_| o == self.sel_out).map(|s| s.norm()) else {
            return;
        };
        // яскрава область
        let (ix0, iy0) = (sel.x0.round().max(0.0) as usize, sel.y0.round().max(0.0) as usize);
        let (ix1, iy1) = (sel.x1.round().min(sw) as usize, sel.y1.round().min(sh) as usize);
        let fw = frame.width() as usize;
        if ix1 > ix0 && iy1 > iy0 {
            let src = scr.orig.data();
            let dst = frame.data_mut();
            for y in iy0..iy1 {
                let a = (y * fw + ix0) * 4;
                let b = (y * fw + ix1) * 4;
                dst[a..b].copy_from_slice(&src[a..b]);
            }
        }
        // фігури, обрізані виділенням
        let clip = (ix0 as i32, iy0 as i32, ix1 as i32, iy1 as i32);
        {
            let mut mask = Mask::new(frame.width(), frame.height()).unwrap();
            if let Some(r) = Rect::from_ltrb(ix0 as f32, iy0 as f32, ix1 as f32, iy1 as f32) {
                mask.fill_path(&PathBuilder::from_rect(r), FillRule::Winding, false, Transform::identity());
            }
            let mut pm = frame.as_mut();
            for s in self.shapes.iter().chain(self.drawing.iter()) {
                draw_shape(&mut pm, s, &scr.orig, (0.0, 0.0), Some(&mask), clip, fonts);
            }
            if let Some((pos, text)) = &self.text_edit {
                let size = self.text_size(k);
                let s = Shape::Text { pos: *pos, text: text.clone(), c: self.col(), size };
                draw_shape(&mut pm, &s, &scr.orig, (0.0, 0.0), Some(&mask), clip, fonts);
                // курсор тексту
                let last = text.split('\n').last().unwrap_or("");
                let lines = text.split('\n').count().max(1) as f32;
                let cx = pos.0 + gfx::text_width(&fonts.medium, size, last) + 2.0 * k;
                let cy = pos.1 + (lines - 1.0) * size * 1.2;
                if let Some(r) = Rect::from_xywh(cx, cy, (1.5 * k).max(1.0), size * 1.1) {
                    pm.fill_rect(r, &paint(theme.accent), Transform::identity(), None);
                }
            }
        }
        let mut pm = frame.as_mut();
        // рамка виділення
        let bw = 1.5 * k;
        let mut st = Stroke::default();
        st.width = bw + 2.0 * k;
        let rect = rrect(sel.x0, sel.y0, sel.w(), sel.h(), 0.0);
        pm.stroke_path(&rect, &paint([0, 0, 0, 70]), &st, Transform::identity(), None);
        st.width = bw;
        pm.stroke_path(&rect, &paint(theme.accent), &st, Transform::identity(), None);
        // ручки
        for h in sel.handles() {
            pm.fill_path(&circle(h.0, h.1 + 0.5 * k, 7.5 * k), &paint([0, 0, 0, 60]), FillRule::Winding, Transform::identity(), None);
            pm.fill_path(&circle(h.0, h.1, 6.5 * k), &paint(theme.accent), FillRule::Winding, Transform::identity(), None);
            pm.fill_path(&circle(h.0, h.1, 4.5 * k), &paint([255, 255, 255, 255]), FillRule::Winding, Transform::identity(), None);
        }
        // підпис розміру
        let label = format!("{} × {}", sel.w().round() as i32, sel.h().round() as i32);
        let fs = 13.0 * k;
        let tw = gfx::text_width(&fonts.medium, fs, &label);
        let (lw, lh) = (tw + 18.0 * k, 24.0 * k);
        let lx = sel.x0.clamp(8.0 * k, sw - lw - 8.0 * k);
        let mut ly = sel.y0 - lh - 8.0 * k;
        if ly < 8.0 * k {
            ly = sel.y0 + 8.0 * k;
        }
        glass(&mut pm, scr, theme, R { x0: lx, y0: ly, x1: lx + lw, y1: ly + lh }, 8.0 * k, k, false);
        gfx::draw_text(&mut pm, &fonts.medium, fs, lx + 9.0 * k, ly + 16.5 * k, theme.ink(), &label);

        if !self.toolbar_visible() {
            return;
        }
        let Some(l) = self.layout(sw, sh, k) else { return };
        glass(&mut pm, scr, theme, l.bar, 14.0 * k, k, true);
        let by = l.bar.y0 + 5.0 * k;
        for (i, (it, x, w)) in l.items.iter().enumerate() {
            if *it == Item::Sep {
                if let Some(r) = Rect::from_xywh(x + w / 2.0 - 0.5 * k, by + l.btn / 2.0 - 10.0 * k, k.max(1.0), 20.0 * k) {
                    pm.fill_rect(r, &paint(theme.edge()), Transform::identity(), None);
                }
                continue;
            }
            let active = matches!(it, Item::Tool(t) if *t == self.tool) || (*it == Item::Color && self.popover);
            let hovered = self.hover == Some(i);
            let btn = rrect(*x, by, *w, l.btn, 10.0 * k);
            let variant = if active && *it != Item::Color {
                pm.fill_path(&btn, &paint(theme.accent), FillRule::Winding, Transform::identity(), None);
                2
            } else {
                if hovered || active {
                    pm.fill_path(&btn, &paint(theme.hover()), FillRule::Winding, Transform::identity(), None);
                }
                if hovered || matches!(it, Item::Save | Item::Copy) { 1 } else { 0 }
            };
            let (cx, cy) = (x + w / 2.0, by + l.btn / 2.0);
            if *it == Item::Color {
                pm.fill_path(&circle(cx, cy, 10.0 * k), &paint([255, 255, 255, 220]), FillRule::Winding, Transform::identity(), None);
                pm.fill_path(&circle(cx, cy, 8.0 * k), &paint(self.col()), FillRule::Winding, Transform::identity(), None);
            } else if let Some(ic) = item_icon(*it) {
                let p = icons.get(ic, variant);
                let s = p.width() as f32;
                pm.draw_pixmap(
                    (cx - s / 2.0).round() as i32,
                    (cy - s / 2.0).round() as i32,
                    p.as_ref(),
                    &Default::default(),
                    Transform::identity(),
                    None,
                );
            }
        }
        // поповер
        if let Some(pop) = l.pop {
            glass(&mut pm, scr, theme, pop, 14.0 * k, k, true);
            let (sws, wds) = Self::pop_targets(&pop, k);
            for (i, r) in sws.iter().enumerate() {
                let (cx, cy) = ((r.x0 + r.x1) / 2.0, (r.y0 + r.y1) / 2.0);
                if i == self.color {
                    pm.fill_path(&circle(cx, cy, 14.0 * k), &paint([255, 255, 255, 255]), FillRule::Winding, Transform::identity(), None);
                    pm.fill_path(&circle(cx, cy, 12.0 * k), &paint([30, 32, 36, 255]), FillRule::Winding, Transform::identity(), None);
                }
                pm.fill_path(&circle(cx, cy, 10.0 * k), &paint(PALETTE[i]), FillRule::Winding, Transform::identity(), None);
                if i == 7 {
                    let mut s = Stroke::default();
                    s.width = k;
                    pm.stroke_path(&circle(cx, cy, 10.0 * k), &paint([255, 255, 255, 80]), &s, Transform::identity(), None);
                }
            }
            for (i, r) in wds.iter().enumerate() {
                let hv = r.contains(self.cursor) && self.cursor_out == o;
                if i == self.width || hv {
                    let c = if i == self.width { theme.selected_bg() } else { theme.hover() };
                    pm.fill_path(&rrect(r.x0, r.y0, r.w(), r.h(), 8.0 * k), &paint(c), FillRule::Winding, Transform::identity(), None);
                }
                let t = WIDTHS[i] * k;
                let (cx, cy) = ((r.x0 + r.x1) / 2.0, (r.y0 + r.y1) / 2.0);
                pm.fill_path(&rrect(cx - 12.0 * k, cy - t / 2.0, 24.0 * k, t, t / 2.0), &paint(theme.ink()), FillRule::Winding, Transform::identity(), None);
            }
        }
        // підказка
        if let (true, Some(i)) = (self.tooltip, self.hover) {
            if let Some((it, x, w)) = l.items.get(i) {
                let (name, key) = item_tip(*it);
                let fs = 12.5 * k;
                let gap = 8.0 * k;
                let tw1 = gfx::text_width(&fonts.regular, fs, name);
                let tw2 = gfx::text_width(&fonts.regular, fs, key);
                let (tw, th) = (tw1 + gap + tw2 + 18.0 * k, 26.0 * k);
                let tx = (x + w / 2.0 - tw / 2.0).clamp(4.0 * k, sw - tw - 4.0 * k);
                let ty = if l.bar_below && l.pop.is_none() || !l.bar_below {
                    if l.bar_below { l.bar.y1 + 8.0 * k } else { l.bar.y0 - th - 8.0 * k }
                } else {
                    l.bar.y0 - th - 8.0 * k
                };
                let ty = if ty + th > sh { l.bar.y0 - th - 8.0 * k } else { ty };
                pm.fill_path(&rrect(tx, ty, tw, th, 8.0 * k), &paint(theme.tip_bg()), FillRule::Winding, Transform::identity(), None);
                let bl = ty + 17.5 * k;
                gfx::draw_text(&mut pm, &fonts.regular, fs, tx + 9.0 * k, bl, theme.ink(), name);
                gfx::draw_text(&mut pm, &fonts.regular, fs, tx + 9.0 * k + tw1 + gap, bl, theme.ink_dim(), key);
            }
        }
    }

    /// Готове зображення: виділення (або весь монітор) з анотаціями.
    pub fn export(&self, scr: &Screen, fonts: &Fonts) -> Pixmap {
        let full = R { x0: 0.0, y0: 0.0, x1: scr.w(), y1: scr.h() };
        let s = self.sel.map(|s| s.norm()).unwrap_or(full);
        let x0 = s.x0.round().clamp(0.0, scr.w()) as i32;
        let y0 = s.y0.round().clamp(0.0, scr.h()) as i32;
        let x1 = s.x1.round().clamp(0.0, scr.w()) as i32;
        let y1 = s.y1.round().clamp(0.0, scr.h()) as i32;
        let (w, h) = ((x1 - x0).max(1) as u32, (y1 - y0).max(1) as u32);
        let mut out = Pixmap::new(w, h).unwrap();
        let fw = scr.orig.width() as usize;
        let src = scr.orig.data();
        let row = w as usize * 4;
        for y in 0..h as usize {
            let a = ((y0 as usize + y) * fw + x0 as usize) * 4;
            out.data_mut()[y * row..(y + 1) * row].copy_from_slice(&src[a..a + row]);
        }
        let mut pm = out.as_mut();
        for sh in &self.shapes {
            draw_shape(&mut pm, sh, &scr.orig, (x0 as f32, y0 as f32), None, (x0, y0, x1, y1), fonts);
        }
        out
    }
}

/// Панель зі «скла»: розмита копія екрана + тонування + тонка грань.
fn glass(pm: &mut PixmapMut, scr: &Screen, theme: &Theme, r: R, rad: f32, k: f32, shadow: bool) {
    if shadow {
        for i in 1..=8 {
            let f = i as f32 * 1.5 * k;
            let a = (16 - i * 2).max(1) as u8;
            let p = rrect(r.x0 - f, r.y0 - f + 6.0 * k, r.w() + 2.0 * f, r.h() + 2.0 * f, rad + f);
            pm.fill_path(&p, &paint([0, 0, 0, a]), FillRule::Winding, Transform::identity(), None);
        }
    }
    let path = rrect(r.x0, r.y0, r.w(), r.h(), rad);
    let f = scr.w() / scr.blur.width() as f32;
    let pat = Pattern::new(scr.blur.as_ref(), SpreadMode::Pad, FilterQuality::Bilinear, 1.0, Transform::from_scale(f, f));
    let mut p = Paint::default();
    p.shader = pat;
    p.anti_alias = true;
    pm.fill_path(&path, &p, FillRule::Winding, Transform::identity(), None);
    pm.fill_path(&path, &paint(theme.glass()), FillRule::Winding, Transform::identity(), None);
    let mut st = Stroke::default();
    st.width = k.max(1.0);
    let inner = rrect(r.x0 + 0.5 * k, r.y0 + 0.5 * k, r.w() - k, r.h() - k, rad - 0.5 * k);
    pm.stroke_path(&inner, &paint(theme.edge()), &st, Transform::identity(), None);
}

fn stroke(w: f32) -> Stroke {
    let mut s = Stroke::default();
    s.width = w;
    s.line_cap = LineCap::Round;
    s.line_join = LineJoin::Round;
    s
}

fn poly(pts: &[P], t: P) -> Option<resvg::tiny_skia::Path> {
    let mut pb = PathBuilder::new();
    pb.move_to(pts[0].0 - t.0, pts[0].1 - t.1);
    if pts.len() == 1 {
        pb.line_to(pts[0].0 - t.0 + 0.01, pts[0].1 - t.1);
    }
    for p in &pts[1..] {
        pb.line_to(p.0 - t.0, p.1 - t.1);
    }
    pb.finish()
}

/// `t` — зсув початку координат (для експорту), `clip` — межі в екранних пікселях.
fn draw_shape(pm: &mut PixmapMut, s: &Shape, orig: &Pixmap, t: P, mask: Option<&Mask>, clip: (i32, i32, i32, i32), fonts: &Fonts) {
    let id = Transform::identity();
    match s {
        Shape::Pen { pts, c, w } => {
            if let Some(p) = poly(pts, t) {
                pm.stroke_path(&p, &paint(*c), &stroke(*w), id, mask);
            }
        }
        Shape::Marker { pts, c, w } => {
            if let Some(p) = poly(pts, t) {
                let mut s = stroke(*w);
                s.line_cap = LineCap::Square;
                pm.stroke_path(&p, &paint([c[0], c[1], c[2], 105]), &s, id, mask);
            }
        }
        Shape::Line { a, b, c, w } => {
            if let Some(p) = poly(&[*a, *b], t) {
                pm.stroke_path(&p, &paint(*c), &stroke(*w), id, mask);
            }
        }
        Shape::Rect { a, b, c, w } => {
            let r = R { x0: a.0, y0: a.1, x1: b.0, y1: b.1 }.norm();
            let p = rrect(r.x0 - t.0, r.y0 - t.1, r.w(), r.h(), w * 0.8);
            pm.stroke_path(&p, &paint(*c), &stroke(*w), id, mask);
        }
        Shape::Arrow { a, b, c, w } => {
            let (dx, dy) = (b.0 - a.0, b.1 - a.1);
            let len = dx.hypot(dy);
            if len < 1.0 {
                return;
            }
            let (ux, uy) = (dx / len, dy / len);
            let hl = (w * 3.2 + 10.0).min(len * 0.6);
            let hw = hl * 0.6;
            let base = (b.0 - ux * hl, b.1 - uy * hl);
            if let Some(p) = poly(&[*a, (base.0 + ux * 1.5, base.1 + uy * 1.5)], t) {
                pm.stroke_path(&p, &paint(*c), &stroke(*w), id, mask);
            }
            let mut pb = PathBuilder::new();
            pb.move_to(b.0 - t.0, b.1 - t.1);
            pb.line_to(base.0 - uy * hw - t.0, base.1 + ux * hw - t.1);
            pb.line_to(base.0 + uy * hw - t.0, base.1 - ux * hw - t.1);
            pb.close();
            if let Some(p) = pb.finish() {
                pm.fill_path(&p, &paint(*c), FillRule::Winding, id, mask);
                let mut s = stroke(w * 0.6);
                s.line_join = LineJoin::Round;
                pm.stroke_path(&p, &paint(*c), &s, id, mask);
            }
        }
        Shape::Text { pos, text, c, size } => {
            for (i, line) in text.split('\n').enumerate() {
                let y = pos.1 - t.1 + size * 0.9 + i as f32 * size * 1.2;
                // легка тінь для читабельності на будь-якому фоні
                let sh = if c[0] as u32 + c[1] as u32 + c[2] as u32 > 380 { [0, 0, 0, 120] } else { [255, 255, 255, 110] };
                let x = pos.0 - t.0;
                let mut tmp = |dx: f32, dy: f32, col| {
                    gfx::draw_text(pm, &fonts.medium, *size, x + dx, y + dy, col, line);
                };
                tmp(0.0, size * 0.05, sh);
                tmp(0.0, 0.0, *c);
            }
            let _ = (mask, clip);
        }
        Shape::Pixel { a, b, block } => {
            let r = R { x0: a.0, y0: a.1, x1: b.0, y1: b.1 }.norm();
            let (ow, oh) = (orig.width() as i32, orig.height() as i32);
            let x0 = (r.x0.round() as i32).max(clip.0).max(0);
            let y0 = (r.y0.round() as i32).max(clip.1).max(0);
            let x1 = (r.x1.round() as i32).min(clip.2).min(ow);
            let y1 = (r.y1.round() as i32).min(clip.3).min(oh);
            if x1 <= x0 || y1 <= y0 {
                return;
            }
            let bs = *block as i32;
            let src = orig.data();
            let (dw, dh) = (pm.width() as i32, pm.height() as i32);
            let dst = pm.data_mut();
            let mut by = (y0 / bs) * bs;
            while by < y1 {
                let mut bx = (x0 / bs) * bs;
                while bx < x1 {
                    let (cx0, cy0) = (bx.max(x0), by.max(y0));
                    let (cx1, cy1) = ((bx + bs).min(x1), (by + bs).min(y1));
                    let mut acc = [0u32; 3];
                    let mut n = 0u32;
                    for y in by.max(0)..(by + bs).min(oh) {
                        for x in bx.max(0)..(bx + bs).min(ow) {
                            let i = ((y * ow + x) * 4) as usize;
                            acc[0] += src[i] as u32;
                            acc[1] += src[i + 1] as u32;
                            acc[2] += src[i + 2] as u32;
                            n += 1;
                        }
                    }
                    let n = n.max(1);
                    let c = [(acc[0] / n) as u8, (acc[1] / n) as u8, (acc[2] / n) as u8, 255];
                    for y in cy0..cy1 {
                        let dy = y - t.1 as i32;
                        if dy < 0 || dy >= dh {
                            continue;
                        }
                        for x in cx0..cx1 {
                            let dx = x - t.0 as i32;
                            if dx < 0 || dx >= dw {
                                continue;
                            }
                            let i = ((dy * dw + dx) * 4) as usize;
                            dst[i..i + 4].copy_from_slice(&c);
                        }
                    }
                    bx += bs;
                }
                by += bs;
            }
        }
    }
}
