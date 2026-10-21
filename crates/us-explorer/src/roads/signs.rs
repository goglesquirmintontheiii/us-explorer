//! Painting sign faces: lettering and shapes rasterized into RGBA atlases, with alpha
//! for the shape outline (stop octagons, shields...). Fonts: DejaVu Sans Condensed Bold.

use ab_glyph::{Font, FontRef, PxScale, ScaleFont};
use std::sync::OnceLock;

/// Pixels per metre of sign face.
pub const PPM: f32 = 170.0;

fn font() -> &'static FontRef<'static> {
    static F: OnceLock<FontRef<'static>> = OnceLock::new();
    F.get_or_init(|| FontRef::try_from_slice(dejavu::sans_condensed::bold()).expect("bundled font"))
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Rgba(pub [u8; 4]);

pub const WHITE: Rgba = Rgba([245, 246, 244, 255]);
pub const BLACK: Rgba = Rgba([22, 22, 24, 255]);
pub const GREEN: Rgba = Rgba([0, 106, 66, 255]);
pub const RED: Rgba = Rgba([196, 22, 28, 255]);
pub const BLUE: Rgba = Rgba([0, 62, 140, 255]);
pub const YELLOW: Rgba = Rgba([250, 204, 20, 255]);

pub struct Canvas {
    pub w: u32,
    pub h: u32,
    pub px: Vec<u8>,
}

impl Canvas {
    pub fn new(w: u32, h: u32) -> Self {
        Self { w, h, px: vec![0; (w * h * 4) as usize] }
    }

    fn blend(&mut self, x: i32, y: i32, c: Rgba, a: f32) {
        if x < 0 || y < 0 || x >= self.w as i32 || y >= self.h as i32 || a <= 0.0 {
            return;
        }
        let i = ((y as u32 * self.w + x as u32) * 4) as usize;
        let a = a.min(1.0) * c.0[3] as f32 / 255.0;
        for k in 0..3 {
            self.px[i + k] = (self.px[i + k] as f32 * (1.0 - a) + c.0[k] as f32 * a) as u8;
        }
        self.px[i + 3] = (self.px[i + 3] as f32 + (255.0 - self.px[i + 3] as f32) * a) as u8;
    }

    /// Fill a polygon (pixel coordinates) with 4x4 supersampled edges.
    pub fn poly(&mut self, x0: i32, y0: i32, pts: &[(f32, f32)], c: Rgba) {
        let (mut minx, mut miny, mut maxx, mut maxy) = (f32::MAX, f32::MAX, f32::MIN, f32::MIN);
        for &(x, y) in pts {
            minx = minx.min(x);
            miny = miny.min(y);
            maxx = maxx.max(x);
            maxy = maxy.max(y);
        }
        let inside = |px: f32, py: f32| {
            let mut c = false;
            let n = pts.len();
            for i in 0..n {
                let (a, b) = (pts[i], pts[(i + n - 1) % n]);
                if (a.1 > py) != (b.1 > py) && px < (b.0 - a.0) * (py - a.1) / (b.1 - a.1) + a.0 {
                    c = !c;
                }
            }
            c
        };
        for y in miny.floor() as i32..=maxy.ceil() as i32 {
            for x in minx.floor() as i32..=maxx.ceil() as i32 {
                let mut cov = 0;
                for sy in 0..4 {
                    for sx in 0..4 {
                        if inside(x as f32 + (sx as f32 + 0.5) / 4.0, y as f32 + (sy as f32 + 0.5) / 4.0) {
                            cov += 1;
                        }
                    }
                }
                if cov > 0 {
                    self.blend(x0 + x, y0 + y, c, cov as f32 / 16.0);
                }
            }
        }
    }

    pub fn rect(&mut self, x: i32, y: i32, w: i32, h: i32, c: Rgba) {
        self.poly(
            0,
            0,
            &[(x as f32, y as f32), ((x + w) as f32, y as f32), ((x + w) as f32, (y + h) as f32), (x as f32, (y + h) as f32)],
            c,
        );
    }

    pub fn round_rect(&mut self, x: f32, y: f32, w: f32, h: f32, r: f32, c: Rgba) {
        let mut pts = Vec::new();
        let corners = [(x + w - r, y + r, -90.0f32), (x + w - r, y + h - r, 0.0), (x + r, y + h - r, 90.0), (x + r, y + r, 180.0)];
        for (cx, cy, a0) in corners {
            for k in 0..=6 {
                let a = (a0 + k as f32 * 15.0).to_radians();
                pts.push((cx + r * a.cos(), cy + r * a.sin()));
            }
        }
        self.poly(0, 0, &pts, c);
    }

    pub fn text_width(text: &str, size: f32) -> f32 {
        let f = font().as_scaled(PxScale::from(size));
        let mut w = 0.0;
        let mut prev = None;
        for ch in text.chars() {
            let g = f.glyph_id(ch);
            if let Some(p) = prev {
                w += f.kern(p, g);
            }
            w += f.h_advance(g);
            prev = Some(g);
        }
        w
    }

    /// Draw text centred in a box, shrunk (never stretched) to fit its width.
    pub fn text(&mut self, cx: f32, cy: f32, max_w: f32, size: f32, text: &str, c: Rgba) {
        let mut size = size;
        let w = Self::text_width(text, size);
        if w > max_w {
            size *= max_w / w;
        }
        let f = font().as_scaled(PxScale::from(size));
        let w = Self::text_width(text, size);
        let mut x = cx - w * 0.5;
        let base = cy + (f.ascent() + f.descent()) * 0.5;
        let mut prev = None;
        for ch in text.chars() {
            let gid = f.glyph_id(ch);
            if let Some(p) = prev {
                x += f.kern(p, gid);
            }
            let g = gid.with_scale_and_position(PxScale::from(size), ab_glyph::point(x, base));
            if let Some(o) = font().outline_glyph(g) {
                let b = o.px_bounds();
                o.draw(|gx, gy, cov| self.blend(b.min.x as i32 + gx as i32, b.min.y as i32 + gy as i32, c, cov));
            }
            x += f.h_advance(gid);
            prev = Some(gid);
        }
    }

    /// Copy another canvas in at (x, y).
    pub fn blit(&mut self, src: &Canvas, x: u32, y: u32) {
        for row in 0..src.h {
            let d = (((y + row) * self.w + x) * 4) as usize;
            let s = (row * src.w * 4) as usize;
            self.px[d..d + (src.w * 4) as usize].copy_from_slice(&src.px[s..s + (src.w * 4) as usize]);
        }
    }
}

// ------------------------------------------------------------------ faces

/// A sign face: its canvas and physical size in metres.
pub struct Face {
    pub canvas: Canvas,
    pub w_m: f32,
    pub h_m: f32,
}

fn px(m: f32) -> u32 {
    (m * PPM).round().max(4.0) as u32
}

/// A street name blade: white capitals-and-lowercase on green, white border.
pub fn street_blade(name: &str) -> Face {
    let name = abbreviate(name);
    let h_m = 0.23;
    let text_h = 0.13 * PPM;
    let w_m = (Canvas::text_width(&name, text_h) / PPM + 0.16).clamp(0.6, 1.8);
    let (w, h) = (px(w_m), px(h_m));
    let mut c = Canvas::new(w, h);
    c.round_rect(0.0, 0.0, w as f32, h as f32, 6.0, WHITE);
    c.round_rect(3.0, 3.0, w as f32 - 6.0, h as f32 - 6.0, 4.0, GREEN);
    c.text(w as f32 * 0.5, h as f32 * 0.5, w as f32 - 18.0, text_h, &name, WHITE);
    Face { canvas: c, w_m, h_m }
}

/// USPS-style street suffixes, as on real blades.
pub fn abbreviate(name: &str) -> String {
    const SUFFIX: &[(&str, &str)] = &[
        ("Street", "St"),
        ("Avenue", "Ave"),
        ("Boulevard", "Blvd"),
        ("Road", "Rd"),
        ("Drive", "Dr"),
        ("Lane", "Ln"),
        ("Court", "Ct"),
        ("Place", "Pl"),
        ("Parkway", "Pkwy"),
        ("Highway", "Hwy"),
        ("Terrace", "Ter"),
        ("Circle", "Cir"),
        ("Trail", "Trl"),
        ("Square", "Sq"),
        ("Expressway", "Expy"),
        ("Freeway", "Fwy"),
    ];
    const DIR: &[(&str, &str)] = &[
        ("North", "N"),
        ("South", "S"),
        ("East", "E"),
        ("West", "W"),
        ("Northeast", "NE"),
        ("Northwest", "NW"),
        ("Southeast", "SE"),
        ("Southwest", "SW"),
    ];
    let words: Vec<&str> = name.split_whitespace().collect();
    let n = words.len();
    words
        .iter()
        .enumerate()
        .map(|(i, w)| {
            if n > 1
                && (i == n - 1 || i == n - 2 && DIR.iter().any(|d| d.0 == words[n - 1]))
                && let Some((_, s)) = SUFFIX.iter().find(|(l, _)| l == w)
            {
                return s.to_string();
            }
            if n > 1 && (i == 0 || i == n - 1) {
                if let Some((_, d)) = DIR.iter().find(|(l, _)| l == w) {
                    return d.to_string();
                }
            }
            w.to_string()
        })
        .collect::<Vec<_>>()
        .join(" ")
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Network {
    Interstate,
    Us,
    State,
}

/// Split an OSM `ref` like "I 25", "US 87", "CO 470", "SR-1" into network and number.
pub fn route(r: &str) -> (Network, String) {
    let r = r.trim();
    let mut parts = r.splitn(2, [' ', '-']);
    let pre = parts.next().unwrap_or("").to_ascii_uppercase();
    let num = parts.next().unwrap_or("").trim().to_string();
    if num.is_empty() {
        return (Network::State, r.to_string());
    }
    match pre.as_str() {
        "I" => (Network::Interstate, num),
        "US" => (Network::Us, num),
        _ => (Network::State, num),
    }
}

/// Route marker: interstate (blue and red shield), US (white shield on black), state (white circle).
pub fn shield(r: &str) -> Face {
    let (net, num) = route(r);
    let s_m = if num.len() >= 3 { 0.75 } else { 0.6 };
    let w_m = if net == Network::State || num.len() >= 3 { s_m * 1.25 } else { s_m };
    let (w, h) = (px(w_m), px(s_m));
    let (wf, hf) = (w as f32, h as f32);
    let mut c = Canvas::new(w, h);
    match net {
        Network::Interstate => {
            let shape = |inset: f32| {
                let mut p = Vec::new();
                let (l, r, t, b) = (inset, wf - inset, inset, hf - inset);
                p.push((l, t + hf * 0.08));
                p.push((wf * 0.25, t));
                p.push((wf * 0.5, t + hf * 0.06));
                p.push((wf * 0.75, t));
                p.push((r, t + hf * 0.08));
                for k in 0..=10 {
                    let a = k as f32 / 10.0;
                    let x = r - (r - wf * 0.5) * a;
                    let y = t + hf * 0.08 + (b - t - hf * 0.08) * (a.powf(0.6) * 0.9 + 0.1 * a);
                    p.push((x, y.min(b)));
                }
                for k in 1..=10 {
                    let a = 1.0 - k as f32 / 10.0;
                    let x = l + (wf * 0.5 - l) * a;
                    let y = t + hf * 0.08 + (b - t - hf * 0.08) * (a.powf(0.6) * 0.9 + 0.1 * a);
                    p.push((x, y.min(b)));
                }
                p
            };
            c.poly(0, 0, &shape(0.0), WHITE);
            c.poly(0, 0, &shape(4.0), BLUE);
            c.poly(
                0,
                0,
                &[
                    (4.0, 4.0 + hf * 0.08),
                    (wf * 0.25, 4.0),
                    (wf * 0.5, 4.0 + hf * 0.06),
                    (wf * 0.75, 4.0),
                    (wf - 4.0, 4.0 + hf * 0.08),
                    (wf - 4.0, hf * 0.27),
                    (4.0, hf * 0.27),
                ],
                RED,
            );
            c.text(wf * 0.5, hf * 0.17, wf * 0.6, hf * 0.12, "INTERSTATE", WHITE);
            c.text(wf * 0.5, hf * 0.58, wf * 0.78, hf * 0.42, &num, WHITE);
        }
        Network::Us => {
            c.rect(0, 0, w as i32, h as i32, BLACK);
            let shape = [
                (wf * 0.12, hf * 0.08),
                (wf * 0.88, hf * 0.08),
                (wf * 0.92, hf * 0.22),
                (wf * 0.88, hf * 0.62),
                (wf * 0.5, hf * 0.93),
                (wf * 0.12, hf * 0.62),
                (wf * 0.08, hf * 0.22),
            ];
            c.poly(0, 0, &shape, WHITE);
            c.text(wf * 0.5, hf * 0.43, wf * 0.66, hf * 0.42, &num, BLACK);
        }
        Network::State => {
            c.round_rect(0.0, 0.0, wf, hf, 10.0, BLACK);
            let mut circle = Vec::new();
            for k in 0..48 {
                let a = k as f32 / 48.0 * std::f32::consts::TAU;
                circle.push((wf * 0.5 + a.cos() * (wf * 0.44), hf * 0.5 + a.sin() * (hf * 0.42)));
            }
            c.poly(0, 0, &circle, WHITE);
            c.text(wf * 0.5, hf * 0.5, wf * 0.7, hf * 0.42, &num, BLACK);
        }
    }
    Face { canvas: c, w_m, h_m: s_m }
}

/// Green exit panel: "EXIT 210A" tab and the destination underneath.
pub fn exit_panel(exit: &str, text: &str) -> Face {
    let line = text.split(';').next().unwrap_or(text).trim();
    let w_m = 4.2;
    let h_m = 2.2;
    let (w, h) = (px(w_m), px(h_m));
    let (wf, hf) = (w as f32, h as f32);
    let mut c = Canvas::new(w, h);
    let tab = if exit.is_empty() { 0.0 } else { hf * 0.28 };
    if tab > 0.0 {
        c.round_rect(wf * 0.55, 0.0, wf * 0.45, tab + 14.0, 10.0, WHITE);
        c.round_rect(wf * 0.55 + 5.0, 5.0, wf * 0.45 - 10.0, tab + 4.0, 7.0, GREEN);
        c.text(wf * 0.775, tab * 0.55, wf * 0.4, tab * 0.6, &format!("EXIT {exit}"), WHITE);
    }
    c.round_rect(0.0, tab, wf, hf - tab, 12.0, WHITE);
    c.round_rect(6.0, tab + 6.0, wf - 12.0, hf - tab - 12.0, 9.0, GREEN);
    if !line.is_empty() {
        c.text(wf * 0.5, tab + (hf - tab) * 0.5, wf * 0.9, (hf - tab) * 0.36, line, WHITE);
    } else {
        c.text(wf * 0.5, tab + (hf - tab) * 0.5, wf * 0.9, (hf - tab) * 0.36, "NEXT EXIT", WHITE);
    }
    Face { canvas: c, w_m, h_m }
}

pub fn stop() -> Face {
    let s = 0.76;
    let n = px(s);
    let nf = n as f32;
    let mut c = Canvas::new(n, n);
    let oct = |r: f32| -> Vec<(f32, f32)> {
        (0..8)
            .map(|k| {
                let a = (k as f32 * 45.0 + 22.5).to_radians();
                (nf * 0.5 + a.cos() * r, nf * 0.5 + a.sin() * r)
            })
            .collect()
    };
    c.poly(0, 0, &oct(nf * 0.5 / (22.5f32.to_radians().cos())), WHITE);
    c.poly(0, 0, &oct(nf * 0.5 / (22.5f32.to_radians().cos()) - 6.0), RED);
    c.text(nf * 0.5, nf * 0.5, nf * 0.8, nf * 0.33, "STOP", WHITE);
    Face { canvas: c, w_m: s, h_m: s }
}

pub fn all_way() -> Face {
    let (w_m, h_m) = (0.45, 0.15);
    let (w, h) = (px(w_m), px(h_m));
    let mut c = Canvas::new(w, h);
    c.rect(0, 0, w as i32, h as i32, RED);
    c.rect(3, 3, w as i32 - 6, h as i32 - 6, WHITE);
    c.text(w as f32 * 0.5, h as f32 * 0.5, w as f32 * 0.85, h as f32 * 0.6, "ALL WAY", RED);
    Face { canvas: c, w_m, h_m }
}

pub fn yield_sign() -> Face {
    let s = 0.9;
    let n = px(s);
    let nf = n as f32;
    let mut c = Canvas::new(n, n);
    let tri = |i: f32| vec![(i * 1.7, i), (nf - i * 1.7, i), (nf * 0.5, nf * 0.87 - i * 1.4)];
    c.poly(0, 0, &tri(0.0), RED);
    c.poly(0, 0, &tri(nf * 0.14), WHITE);
    c.text(nf * 0.5, nf * 0.3, nf * 0.45, nf * 0.12, "YIELD", RED);
    Face { canvas: c, w_m: s, h_m: s }
}

pub fn speed_limit(mph: u32) -> Face {
    let (w_m, h_m) = (0.6, 0.75);
    let (w, h) = (px(w_m), px(h_m));
    let (wf, hf) = (w as f32, h as f32);
    let mut c = Canvas::new(w, h);
    c.round_rect(0.0, 0.0, wf, hf, 8.0, WHITE);
    c.round_rect(5.0, 5.0, wf - 10.0, hf - 10.0, 6.0, BLACK);
    c.round_rect(9.0, 9.0, wf - 18.0, hf - 18.0, 4.0, WHITE);
    c.text(wf * 0.5, hf * 0.2, wf * 0.75, hf * 0.12, "SPEED", BLACK);
    c.text(wf * 0.5, hf * 0.34, wf * 0.75, hf * 0.12, "LIMIT", BLACK);
    c.text(wf * 0.5, hf * 0.66, wf * 0.8, hf * 0.38, &mph.to_string(), BLACK);
    Face { canvas: c, w_m, h_m }
}

/// R7-8: "RESERVED PARKING" in green over the blue accessibility symbol.
pub fn accessible_parking() -> Face {
    let (w_m, h_m) = (0.3, 0.45);
    let (w, h) = (px(w_m), px(h_m));
    let (wf, hf) = (w as f32, h as f32);
    let mut c = Canvas::new(w, h);
    c.round_rect(0.0, 0.0, wf, hf, 5.0, WHITE);
    c.round_rect(3.0, 3.0, wf - 6.0, hf - 6.0, 4.0, GREEN);
    c.round_rect(6.0, 6.0, wf - 12.0, hf - 12.0, 3.0, WHITE);
    c.text(wf * 0.5, hf * 0.14, wf * 0.8, hf * 0.11, "RESERVED", GREEN);
    c.text(wf * 0.5, hf * 0.27, wf * 0.8, hf * 0.11, "PARKING", GREEN);
    // The blue square with the wheelchair symbol.
    let s = wf * 0.62;
    let (x0, y0) = (wf * 0.5 - s * 0.5, hf * 0.38);
    c.rect(x0 as i32, y0 as i32, s as i32, s as i32, BLUE);
    accessibility_symbol(&mut c, x0 + s * 0.5, y0 + s * 0.5, s * 0.8, WHITE, BLUE);
    Face { canvas: c, w_m, h_m }
}

fn disc(c: &mut Canvas, cx: f32, cy: f32, r: f32, col: Rgba) {
    let pts: Vec<(f32, f32)> = (0..24)
        .map(|k| {
            let a = k as f32 / 24.0 * std::f32::consts::TAU;
            (cx + r * a.cos(), cy + r * a.sin())
        })
        .collect();
    c.poly(0, 0, &pts, col);
}

fn bar(c: &mut Canvas, a: (f32, f32), b: (f32, f32), w: f32, col: Rgba) {
    let (dx, dy) = (b.0 - a.0, b.1 - a.1);
    let l = (dx * dx + dy * dy).sqrt().max(1e-3);
    let (nx, ny) = (-dy / l * w * 0.5, dx / l * w * 0.5);
    c.poly(0, 0, &[(a.0 + nx, a.1 + ny), (b.0 + nx, b.1 + ny), (b.0 - nx, b.1 - ny), (a.0 - nx, a.1 - ny)], col);
}

/// The International Symbol of Access, `s` pixels tall, centred.
pub fn accessibility_symbol(c: &mut Canvas, cx: f32, cy: f32, s: f32, fg: Rgba, bg: Rgba) {
    let u = s / 10.0;
    let (ox, oy) = (cx - 0.2 * u, cy);
    // Wheel.
    disc(c, ox + 0.2 * u, oy + 1.6 * u, 3.0 * u, fg);
    disc(c, ox + 0.2 * u, oy + 1.6 * u, 2.25 * u, bg);
    // Head, back, seat, leg.
    disc(c, ox - 0.4 * u, oy - 4.0 * u, 0.95 * u, fg);
    bar(c, (ox - 0.6 * u, oy - 2.6 * u), (ox - 0.3 * u, oy + 0.9 * u), 1.0 * u, fg);
    bar(c, (ox - 0.3 * u, oy + 0.9 * u), (ox + 2.4 * u, oy + 0.9 * u), 1.0 * u, fg);
    bar(c, (ox + 2.4 * u, oy + 0.9 * u), (ox + 3.6 * u, oy + 3.9 * u), 0.9 * u, fg);
    bar(c, (ox - 0.5 * u, oy - 1.2 * u), (ox + 1.8 * u, oy - 1.2 * u), 0.8 * u, fg);
}

pub fn bus_stop() -> Face {
    let (w_m, h_m) = (0.45, 0.6);
    let (w, h) = (px(w_m), px(h_m));
    let (wf, hf) = (w as f32, h as f32);
    let mut c = Canvas::new(w, h);
    c.round_rect(0.0, 0.0, wf, hf, 8.0, WHITE);
    c.round_rect(5.0, 5.0, wf - 10.0, hf - 10.0, 6.0, BLUE);
    c.text(wf * 0.5, hf * 0.3, wf * 0.8, hf * 0.2, "BUS", WHITE);
    c.text(wf * 0.5, hf * 0.62, wf * 0.8, hf * 0.16, "STOP", WHITE);
    Face { canvas: c, w_m, h_m }
}

/// A freeway mile marker: green, "MILE" over the number.
pub fn mile_marker(n: &str) -> Face {
    let (w_m, h_m) = (0.3, 0.6);
    let (w, h) = (px(w_m), px(h_m));
    let (wf, hf) = (w as f32, h as f32);
    let mut c = Canvas::new(w, h);
    c.round_rect(0.0, 0.0, wf, hf, 5.0, WHITE);
    c.round_rect(3.0, 3.0, wf - 6.0, hf - 6.0, 4.0, GREEN);
    c.text(wf * 0.5, hf * 0.2, wf * 0.8, hf * 0.12, "MILE", WHITE);
    let lines: Vec<&str> = if n.len() > 2 { vec![&n[..n.len() - 2], &n[n.len() - 2..]] } else { vec![n] };
    for (k, l) in lines.iter().enumerate() {
        let y = if lines.len() == 1 { 0.6 } else { 0.47 + k as f32 * 0.28 };
        c.text(wf * 0.5, hf * y, wf * 0.85, hf * 0.24, l, WHITE);
    }
    Face { canvas: c, w_m, h_m }
}

/// One blade of a railroad crossbuck.
pub fn crossbuck_blade(word: &str) -> Face {
    let (w_m, h_m) = (1.22, 0.23);
    let (w, h) = (px(w_m), px(h_m));
    let (wf, hf) = (w as f32, h as f32);
    let mut c = Canvas::new(w, h);
    c.round_rect(0.0, 0.0, wf, hf, 4.0, BLACK);
    c.round_rect(3.0, 3.0, wf - 6.0, hf - 6.0, 3.0, WHITE);
    c.text(wf * 0.5, hf * 0.52, wf * 0.86, hf * 0.62, word, BLACK);
    Face { canvas: c, w_m, h_m }
}

/// Pedestrian signal symbols, white on black: the raised hand (drawn in white, tinted
/// orange by the lamp material) or the walking person.
pub fn ped_symbol(walk: bool) -> Canvas {
    let n = 64u32;
    let mut c = Canvas::new(n, n);
    c.rect(0, 0, n as i32, n as i32, BLACK);
    let s = |pts: &[(f32, f32)]| -> Vec<(f32, f32)> { pts.iter().map(|&(x, y)| (x * n as f32, y * n as f32)).collect() };
    if walk {
        // Head, torso leaning forward, stepping legs and swinging arms.
        c.round_rect(0.43 * 64.0, 0.08 * 64.0, 0.14 * 64.0, 0.14 * 64.0, 4.5, WHITE);
        let limbs: [&[(f32, f32)]; 5] = [
            &[(0.44, 0.25), (0.56, 0.25), (0.58, 0.56), (0.46, 0.56)],
            &[(0.47, 0.54), (0.56, 0.56), (0.40, 0.92), (0.31, 0.90)],
            &[(0.50, 0.54), (0.58, 0.52), (0.72, 0.90), (0.63, 0.92)],
            &[(0.45, 0.28), (0.52, 0.32), (0.36, 0.52), (0.30, 0.48)],
            &[(0.53, 0.29), (0.58, 0.26), (0.70, 0.46), (0.64, 0.50)],
        ];
        for l in limbs {
            c.poly(0, 0, &s(l), WHITE);
        }
    } else {
        // Palm and four fingers, thumb out to the side.
        c.round_rect(0.30 * 64.0, 0.42 * 64.0, 0.40 * 64.0, 0.44 * 64.0, 6.0, WHITE);
        for k in 0..4 {
            let x = 0.30 + k as f32 * 0.1;
            let top = [0.18, 0.12, 0.14, 0.2][k];
            c.round_rect(x * 64.0, top * 64.0, 0.085 * 64.0, (0.5 - top) * 64.0, 2.5, WHITE);
        }
        c.poly(0, 0, &s(&[(0.68, 0.62), (0.84, 0.44), (0.9, 0.5), (0.72, 0.76)]), WHITE);
    }
    c
}

// ------------------------------------------------------------------ atlas

/// Faces packed into one texture (shelf packing), with each face's UV rectangle.
pub struct Atlas {
    pub canvas: Canvas,
    /// (u0, v0, u1, v1) and size in metres, per face, in insertion order.
    pub rects: Vec<([f32; 4], f32, f32)>,
}

pub fn pack(faces: Vec<Face>) -> Option<Atlas> {
    if faces.is_empty() {
        return None;
    }
    let width = 1024u32;
    let pad = 2;
    let mut placed = Vec::with_capacity(faces.len());
    let (mut x, mut y, mut shelf) = (0u32, 0u32, 0u32);
    for f in &faces {
        let (w, h) = (f.canvas.w.min(width - 2 * pad), f.canvas.h);
        if x + w + pad > width {
            x = 0;
            y += shelf + pad;
            shelf = 0;
        }
        placed.push((x + pad, y + pad));
        x += w + pad * 2;
        shelf = shelf.max(h + pad);
    }
    let height = (y + shelf + pad).next_power_of_two().max(64);
    if height > 4096 {
        return None;
    }
    let mut canvas = Canvas::new(width, height);
    let mut rects = Vec::with_capacity(faces.len());
    for (f, &(px0, py0)) in faces.iter().zip(&placed) {
        if f.canvas.w > width - 2 * pad {
            rects.push(([0.0; 4], f.w_m, f.h_m));
            continue;
        }
        canvas.blit(&f.canvas, px0, py0);
        // Half-texel inset so filtering never reads the neighbour.
        let (u0, v0) = ((px0 as f32 + 0.5) / width as f32, (py0 as f32 + 0.5) / height as f32);
        let (u1, v1) = ((px0 + f.canvas.w) as f32 - 0.5, (py0 + f.canvas.h) as f32 - 0.5);
        rects.push(([u0, v0, u1 / width as f32, v1 / height as f32], f.w_m, f.h_m));
    }
    Some(Atlas { canvas, rects })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn abbreviations_and_routes() {
        assert_eq!(abbreviate("East Colfax Avenue"), "E Colfax Ave");
        assert_eq!(abbreviate("Broadway"), "Broadway");
        assert_eq!(abbreviate("Martin Luther King Jr Boulevard"), "Martin Luther King Jr Blvd");
        assert_eq!(abbreviate("Main Street North"), "Main St N");
        assert_eq!(route("I 25"), (Network::Interstate, "25".into()));
        assert_eq!(route("US 287"), (Network::Us, "287".into()));
        assert_eq!(route("CO 470"), (Network::State, "470".into()));
    }

    #[test]
    fn paints_and_packs() {
        let faces =
            vec![street_blade("Larimer Street"), shield("I 70"), shield("US 6"), shield("CO 2"), exit_panel("214A", "Downtown"), stop()];
        let a = pack(faces).unwrap();
        assert_eq!(a.rects.len(), 6);
        // Something was drawn, with transparent corners on the stop sign's octagon.
        assert!(a.canvas.px.chunks(4).filter(|p| p[3] > 0).count() > 50_000);
        let r = a.rects[0].0;
        assert!(r[0] < r[2] && r[1] < r[3]);
    }

    /// Writes sample sign art to a PNG for eyeballing: cargo test signs_png -- --ignored
    #[test]
    #[ignore]
    fn signs_png() {
        let faces = vec![
            street_blade("Larimer Street"),
            street_blade("Martin Luther King Jr Boulevard"),
            shield("I 25"),
            shield("US 287"),
            shield("CO 470"),
            exit_panel("214A", "Downtown Denver"),
            stop(),
            all_way(),
            yield_sign(),
            speed_limit(35),
            bus_stop(),
        ];
        let a = pack(faces).unwrap();
        let out = std::env::var("USX_OUT").unwrap_or("/tmp".into());
        usx_core::image::save_buffer(format!("{out}/signs.png"), &a.canvas.px, a.canvas.w, a.canvas.h, usx_core::image::ColorType::Rgba8)
            .unwrap();
    }
}
