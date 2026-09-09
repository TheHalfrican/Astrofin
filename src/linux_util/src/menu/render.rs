use cosmic_text::{Attrs, Buffer, Color, Family, FontSystem, Metrics, Shaping, SwashCache};
use tiny_skia::{Color as SkColor, FillRule, Paint, PathBuilder, Pixmap, Rect, Transform};

use jfn_platform_abi::MenuItem;

// Logical (unscaled) metrics; multiplied by the display scale at layout time.
const FONT_PX: f32 = 13.0;
const ROW_H: f32 = 28.0;
const SEP_H: f32 = 9.0;
const PAD_X: f32 = 12.0;
const PAD_RIGHT: f32 = 24.0;
const PAD_Y: f32 = 4.0;
const MIN_W: f32 = 160.0;
const RADIUS: f32 = 4.0;

fn bg() -> SkColor {
    SkColor::from_rgba8(0x2b, 0x2b, 0x2b, 0xff)
}
fn border() -> SkColor {
    SkColor::from_rgba8(0x55, 0x55, 0x55, 0xff)
}
fn hover() -> SkColor {
    SkColor::from_rgba8(0x3d, 0x3d, 0x3d, 0xff)
}
fn sep() -> SkColor {
    SkColor::from_rgba8(0x44, 0x44, 0x44, 0xff)
}
const TEXT: Color = Color::rgb(0xe0, 0xe0, 0xe0);
const TEXT_DISABLED: Color = Color::rgb(0x66, 0x66, 0x66);

/// Geometry in physical pixels relative to the menu's top-left.
#[derive(Clone)]
pub struct Row {
    pub item: usize,
    pub y: i32,
    pub h: i32,
    pub separator: bool,
    pub enabled: bool,
}

#[derive(Clone)]
pub struct Layout {
    pub width: i32,
    pub height: i32,
    pub rows: Vec<Row>,
    pub selectable: Vec<usize>,
    scale: f32,
}

impl Layout {
    #[cfg(test)]
    pub fn for_test(width: i32, height: i32, rows: Vec<Row>, selectable: Vec<usize>) -> Self {
        Self {
            width,
            height,
            rows,
            selectable,
            scale: 1.0,
        }
    }

    pub fn contains(&self, x: i32, y: i32) -> bool {
        x >= 0 && x < self.width && y >= 0 && y < self.height
    }

    pub fn row_at(&self, x: i32, y: i32) -> Option<usize> {
        if x < 0 || x >= self.width || y < 0 || y >= self.height {
            return None;
        }
        self.rows
            .iter()
            .find(|r| !r.separator && r.enabled && y >= r.y && y < r.y + r.h)
            .map(|r| r.item)
    }

    pub fn step(&self, active: i32, forward: bool) -> i32 {
        if self.selectable.is_empty() {
            return -1;
        }
        let pos = self.selectable.iter().position(|&i| i as i32 == active);
        let next = match pos {
            Some(p) if forward => (p + 1) % self.selectable.len(),
            Some(p) => (p + self.selectable.len() - 1) % self.selectable.len(),
            None if forward => 0,
            None => self.selectable.len() - 1,
        };
        self.selectable[next] as i32
    }
}

pub struct Fonts {
    system: FontSystem,
    cache: SwashCache,
}

impl Fonts {
    pub fn new() -> Self {
        Self {
            system: FontSystem::new(),
            cache: SwashCache::new(),
        }
    }

    fn shape(&mut self, text: &str, font_px: f32) -> Buffer {
        let mut buf = Buffer::new(&mut self.system, Metrics::new(font_px, font_px * 1.3));
        buf.set_size(None, None);
        buf.set_text(
            text,
            &Attrs::new().family(Family::SansSerif),
            Shaping::Advanced,
            None,
        );
        buf.shape_until_scroll(&mut self.system, false);
        buf
    }

    fn text_width(&mut self, text: &str, font_px: f32) -> f32 {
        self.shape(text, font_px)
            .layout_runs()
            .map(|r| r.line_w)
            .fold(0.0_f32, f32::max)
    }
}

impl Default for Fonts {
    fn default() -> Self {
        Self::new()
    }
}

pub fn layout(fonts: &mut Fonts, items: &[MenuItem], scale: f32) -> Layout {
    let s = if scale > 0.0 { scale } else { 1.0 };
    let font_px = FONT_PX * s;
    let row_h = (ROW_H * s).round() as i32;
    let sep_h = (SEP_H * s).round() as i32;
    let pad_y = (PAD_Y * s).round() as i32;
    let text_w_budget = (PAD_X + PAD_RIGHT) * s;

    let mut max_text = MIN_W * s - text_w_budget;
    for it in items {
        if !it.separator {
            max_text = max_text.max(fonts.text_width(&it.label, font_px));
        }
    }
    let width = (max_text + text_w_budget).ceil() as i32;

    let mut rows = Vec::with_capacity(items.len());
    let mut selectable = Vec::new();
    let mut y = pad_y;
    for (i, it) in items.iter().enumerate() {
        let h = if it.separator { sep_h } else { row_h };
        rows.push(Row {
            item: i,
            y,
            h,
            separator: it.separator,
            enabled: it.enabled,
        });
        if !it.separator && it.enabled {
            selectable.push(i);
        }
        y += h;
    }
    let height = y + pad_y;

    Layout {
        width,
        height,
        rows,
        selectable,
        scale: s,
    }
}

pub fn paint(
    fonts: &mut Fonts,
    layout: &Layout,
    items: &[MenuItem],
    active: i32,
) -> Option<Pixmap> {
    let s = layout.scale;
    let mut pm = Pixmap::new(layout.width as u32, layout.height as u32)?;

    let w = layout.width as f32;
    let h = layout.height as f32;
    let radius = RADIUS * s;

    if let Some(path) = rounded_rect(0.5, 0.5, w - 1.0, h - 1.0, radius) {
        let mut bgp = Paint::default();
        bgp.set_color(bg());
        bgp.anti_alias = true;
        pm.fill_path(&path, &bgp, FillRule::Winding, Transform::identity(), None);

        let stroke = tiny_skia::Stroke {
            width: 1.0,
            ..Default::default()
        };
        let mut bp = Paint::default();
        bp.set_color(border());
        bp.anti_alias = true;
        pm.stroke_path(&path, &bp, &stroke, Transform::identity(), None);
    }

    let pad_x = PAD_X * s;
    let font_px = FONT_PX * s;
    for row in &layout.rows {
        let it = &items[row.item];
        if row.separator {
            if let Some(rect) = Rect::from_xywh(
                pad_x,
                row.y as f32 + row.h as f32 / 2.0,
                w - 2.0 * pad_x,
                (1.0 * s).max(1.0),
            ) {
                let mut p = Paint::default();
                p.set_color(sep());
                pm.fill_rect(rect, &p, Transform::identity(), None);
            }
            continue;
        }

        if row.item as i32 == active
            && let Some(rect) = Rect::from_xywh(1.0, row.y as f32, w - 2.0, row.h as f32)
        {
            let mut p = Paint::default();
            p.set_color(hover());
            pm.fill_rect(rect, &p, Transform::identity(), None);
        }

        let color = if it.enabled { TEXT } else { TEXT_DISABLED };
        let baseline_y = row.y as f32 + (row.h as f32 - font_px) / 2.0;
        draw_text(fonts, &mut pm, &it.label, font_px, pad_x, baseline_y, color);
    }

    Some(pm)
}

fn draw_text(
    fonts: &mut Fonts,
    pm: &mut Pixmap,
    text: &str,
    font_px: f32,
    ox: f32,
    oy: f32,
    color: Color,
) {
    let buf = fonts.shape(text, font_px);
    let pw = pm.width() as i32;
    let ph = pm.height() as i32;
    let pixels = pm.pixels_mut();
    let runs: Vec<_> = buf.layout_runs().collect();
    for run in runs {
        let glyphs: Vec<_> = run.glyphs.to_vec();
        for glyph in &glyphs {
            let phys = glyph.physical((ox, oy + run.line_y), 1.0);
            let Some(img) = fonts
                .cache
                .get_image(&mut fonts.system, phys.cache_key)
                .as_ref()
            else {
                continue;
            };
            if img.data.is_empty() {
                continue;
            }
            let gx = phys.x + img.placement.left;
            let gy = phys.y - img.placement.top;
            blend_coverage(
                pixels,
                pw,
                ph,
                &img.data,
                img.placement.width as i32,
                img.placement.height as i32,
                gx,
                gy,
                color,
            );
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn blend_coverage(
    pixels: &mut [tiny_skia::PremultipliedColorU8],
    pw: i32,
    ph: i32,
    coverage: &[u8],
    cw: i32,
    ch: i32,
    ox: i32,
    oy: i32,
    color: Color,
) {
    let (cr, cg, cb) = (color.r() as u32, color.g() as u32, color.b() as u32);
    for row in 0..ch {
        let py = oy + row;
        if py < 0 || py >= ph {
            continue;
        }
        for col in 0..cw {
            let px = ox + col;
            if px < 0 || px >= pw {
                continue;
            }
            let a = coverage[(row * cw + col) as usize] as u32;
            if a == 0 {
                continue;
            }
            let idx = (py * pw + px) as usize;
            let dst = pixels[idx];
            let inv = 255 - a;
            let nr = (cr * a + dst.red() as u32 * inv) / 255;
            let ng = (cg * a + dst.green() as u32 * inv) / 255;
            let nb = (cb * a + dst.blue() as u32 * inv) / 255;
            let na = a + dst.alpha() as u32 * inv / 255;
            if let Some(p) =
                tiny_skia::PremultipliedColorU8::from_rgba(nr as u8, ng as u8, nb as u8, na as u8)
            {
                pixels[idx] = p;
            }
        }
    }
}

fn rounded_rect(x: f32, y: f32, w: f32, h: f32, r: f32) -> Option<tiny_skia::Path> {
    let r = r.min(w / 2.0).min(h / 2.0).max(0.0);
    let mut pb = PathBuilder::new();
    pb.move_to(x + r, y);
    pb.line_to(x + w - r, y);
    pb.quad_to(x + w, y, x + w, y + r);
    pb.line_to(x + w, y + h - r);
    pb.quad_to(x + w, y + h, x + w - r, y + h);
    pb.line_to(x + r, y + h);
    pb.quad_to(x, y + h, x, y + h - r);
    pb.line_to(x, y + r);
    pb.quad_to(x, y, x + r, y);
    pb.close();
    pb.finish()
}

/// Writes premultiplied BGRA (wl_shm ARGB8888 little-endian, X11 ARGB32), and
/// copies `min(dst.len(), pm.width() * pm.height() * 4)` bytes.
pub fn blit_bgra(pm: &Pixmap, dst: &mut [u8]) {
    let (out_px, _) = dst.as_chunks_mut::<4>();
    let (src_px, _) = pm.data().as_chunks::<4>();
    for (out, src) in out_px.iter_mut().zip(src_px) {
        out[0] = src[2];
        out[1] = src[1];
        out[2] = src[0];
        out[3] = src[3];
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

    use super::*;

    fn item(label: &str) -> MenuItem {
        MenuItem {
            id: 1,
            label: label.into(),
            enabled: true,
            separator: false,
        }
    }

    fn disabled(label: &str) -> MenuItem {
        MenuItem {
            id: 2,
            label: label.into(),
            enabled: false,
            separator: false,
        }
    }

    fn separator() -> MenuItem {
        MenuItem {
            id: 0,
            label: String::new(),
            enabled: false,
            separator: true,
        }
    }

    fn row(item: usize, y: i32, h: i32, separator: bool, enabled: bool) -> Row {
        Row {
            item,
            y,
            h,
            separator,
            enabled,
        }
    }

    /// 100x60: a selectable row, a separator, a disabled row, then a padding
    /// band with no row at all.
    fn fixture() -> Layout {
        Layout::for_test(
            100,
            60,
            vec![
                row(0, 0, 20, false, true),
                row(1, 20, 10, true, false),
                row(2, 30, 20, false, false),
            ],
            vec![0],
        )
    }

    #[test]
    fn contains_covers_the_menu_rectangle_and_nothing_outside_it() {
        let l = fixture();
        assert!(l.contains(0, 0));
        assert!(l.contains(99, 59));
        assert!(!l.contains(100, 59), "right edge is exclusive");
        assert!(!l.contains(99, 60), "bottom edge is exclusive");
        assert!(!l.contains(-1, 0));
        assert!(!l.contains(0, -1));
    }

    #[test]
    fn row_at_finds_the_row_under_the_pointer() {
        let l = fixture();
        assert_eq!(l.row_at(10, 0), Some(0));
        assert_eq!(l.row_at(10, 19), Some(0));
    }

    #[test]
    fn row_at_skips_separators_disabled_rows_and_the_padding_band() {
        let l = fixture();
        assert_eq!(l.row_at(10, 25), None, "separator");
        assert_eq!(l.row_at(10, 35), None, "disabled row");
        assert_eq!(l.row_at(10, 55), None, "padding below the last row");
    }

    #[test]
    fn row_at_rejects_points_outside_the_menu() {
        let l = fixture();
        assert_eq!(l.row_at(-1, 10), None);
        assert_eq!(l.row_at(100, 10), None);
        assert_eq!(l.row_at(10, -1), None);
        assert_eq!(l.row_at(10, 60), None);
    }

    #[test]
    fn step_walks_the_selectable_rows_and_wraps_around() {
        let l = Layout::for_test(100, 100, Vec::new(), vec![0, 2, 3]);
        assert_eq!(l.step(0, true), 2);
        assert_eq!(l.step(2, true), 3);
        assert_eq!(l.step(3, true), 0, "wraps forward");
        assert_eq!(l.step(3, false), 2);
        assert_eq!(l.step(0, false), 3, "wraps backward");
    }

    #[test]
    fn step_enters_the_list_at_the_end_it_is_walking_towards() {
        let l = Layout::for_test(100, 100, Vec::new(), vec![0, 2, 3]);
        // -1 is "nothing highlighted", the state a keyboard-opened menu starts
        // in; a row that is no longer selectable behaves the same way.
        assert_eq!(l.step(-1, true), 0);
        assert_eq!(l.step(-1, false), 3);
        assert_eq!(l.step(1, true), 0);
        assert_eq!(l.step(1, false), 3);
    }

    #[test]
    fn step_reports_no_row_when_nothing_is_selectable() {
        let l = Layout::for_test(100, 100, Vec::new(), Vec::new());
        assert_eq!(l.step(-1, true), -1);
        assert_eq!(l.step(0, false), -1);
    }

    #[test]
    fn layout_stacks_the_rows_between_equal_pads_and_keeps_a_minimum_width() {
        let mut fonts = Fonts::new();
        let items = vec![item("One"), separator(), disabled("Two")];
        let l = layout(&mut fonts, &items, 1.0);

        let geometry: Vec<(i32, i32, bool)> =
            l.rows.iter().map(|r| (r.y, r.h, r.separator)).collect();
        assert_eq!(
            geometry,
            vec![(4, 28, false), (32, 9, true), (41, 28, false)]
        );
        assert_eq!(l.height, 4 + 28 + 9 + 28 + 4);
        assert!(
            l.width >= 160,
            "narrow labels still fill MIN_W: {}",
            l.width
        );
        // Separators and disabled rows are never keyboard-reachable.
        assert_eq!(l.selectable, vec![0]);
    }

    #[test]
    fn layout_multiplies_every_metric_by_the_display_scale() {
        let mut fonts = Fonts::new();
        let items = vec![item("One"), separator(), item("Two")];
        let l = layout(&mut fonts, &items, 2.0);

        let geometry: Vec<(i32, i32)> = l.rows.iter().map(|r| (r.y, r.h)).collect();
        assert_eq!(geometry, vec![(8, 56), (64, 18), (82, 56)]);
        assert_eq!(l.height, 8 + 56 + 18 + 56 + 8);
        assert!(l.width >= 320, "{}", l.width);
        assert_eq!(l.selectable, vec![0, 2]);
    }

    #[test]
    fn a_non_positive_scale_is_treated_as_one() {
        let mut fonts = Fonts::new();
        let items = vec![item("One"), item("Two")];
        let one = layout(&mut fonts, &items, 1.0);
        for scale in [0.0, -2.0] {
            let l = layout(&mut fonts, &items, scale);
            assert_eq!((l.width, l.height), (one.width, one.height), "{scale}");
        }
    }

    #[test]
    fn an_empty_menu_is_just_the_two_pads() {
        let mut fonts = Fonts::default();
        let l = layout(&mut fonts, &[], 1.0);
        assert!(l.rows.is_empty());
        assert!(l.selectable.is_empty());
        assert_eq!(l.height, 8);
        assert!(l.width >= 160);
    }

    #[test]
    fn paint_fills_a_pixmap_the_size_of_the_layout() {
        let mut fonts = Fonts::new();
        let items = vec![item("One"), separator(), item("Two")];
        let l = layout(&mut fonts, &items, 1.0);
        let pm = paint(&mut fonts, &l, &items, 0).expect("a pixmap");
        assert_eq!(pm.width(), l.width as u32);
        assert_eq!(pm.height(), l.height as u32);
        // The rounded background covers the interior: no transparent hole.
        let middle = pm.pixels()[(l.height as usize / 2) * l.width as usize + l.width as usize / 2];
        assert_eq!(middle.alpha(), 255);
    }

    #[test]
    fn a_zero_sized_layout_paints_nothing() {
        let mut fonts = Fonts::new();
        let l = Layout::for_test(0, 0, Vec::new(), Vec::new());
        assert!(paint(&mut fonts, &l, &[], -1).is_none());
    }

    #[test]
    fn blit_bgra_swaps_red_and_blue_and_keeps_alpha() {
        let mut pm = Pixmap::new(2, 1).expect("a pixmap");
        let pixels = pm.pixels_mut();
        pixels[0] = tiny_skia::PremultipliedColorU8::from_rgba(10, 20, 30, 255).expect("a color");
        pixels[1] = tiny_skia::PremultipliedColorU8::from_rgba(1, 2, 3, 255).expect("a color");
        let mut dst = [0u8; 8];
        blit_bgra(&pm, &mut dst);
        assert_eq!(dst, [30, 20, 10, 255, 3, 2, 1, 255]);
    }

    #[test]
    fn blit_bgra_copies_only_the_pixels_both_buffers_hold() {
        let mut pm = Pixmap::new(2, 1).expect("a pixmap");
        pm.pixels_mut()[0] =
            tiny_skia::PremultipliedColorU8::from_rgba(10, 20, 30, 255).expect("a color");

        // A destination shorter than the pixmap keeps its own length.
        let mut short = [0u8; 4];
        blit_bgra(&pm, &mut short);
        assert_eq!(short, [30, 20, 10, 255]);

        // A longer destination keeps whatever was past the last pixel.
        let mut long = [9u8; 12];
        blit_bgra(&pm, &mut long);
        assert_eq!(long[0..4], [30, 20, 10, 255]);
        assert_eq!(long[8..12], [9, 9, 9, 9]);
    }
}
