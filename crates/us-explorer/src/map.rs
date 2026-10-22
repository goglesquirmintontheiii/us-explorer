//! The corner minimap and the full map (search, bookmarks, recent places, teleport).
//! Both draw Web Mercator tiles (Esri street map or imagery) straight into egui,
//! sharing the game's disk cache.

use crate::player::Player;
use bevy::prelude::*;
use bevy_egui::egui::{self, Color32, Pos2, Rect, Vec2 as EVec2};
use std::collections::HashMap;
use usx_core::geo::{Geodetic, parse_lat_lon};
use usx_core::geocode::{self, Place};
use usx_core::net::WorkPool;
use usx_core::settings::{Bookmark, MapStyle};
use usx_core::sources;
use usx_core::tile::{MAX_MERCATOR_LAT, TileId};

const TILE: f64 = 256.0;
const MAX_TEX: usize = 320;

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, PartialOrd, Ord)]
struct MapKey {
    imagery: bool,
    id: TileId,
}

struct Entry {
    tex: Option<egui::TextureHandle>,
    missing: bool,
    last_used: u64,
    /// Frame the request was made (to retry failures later).
    asked: u64,
}

/// Map tiles as egui textures, loaded on a few worker threads.
#[derive(Resource)]
pub struct MapTiles {
    pool: WorkPool<MapKey, (MapKey, Option<egui::ColorImage>)>,
    tiles: HashMap<MapKey, Entry>,
    frame: u64,
}

impl Default for MapTiles {
    fn default() -> Self {
        let pool = WorkPool::new("maptile", 4, |_, k: MapKey| {
            let (key, url) = if k.imagery {
                (format!("img/esri/{}.jpg", k.id.path()), sources::ImagerySource::Esri.url(k.id))
            } else {
                (format!("map/streets/{}.jpg", k.id.path()), sources::street_map_url(k.id))
            };
            let img = crate::terrain::load::fetch(&key, &url).ok().and_then(|b| usx_core::image::load_from_memory(&b).ok()).map(|i| {
                let i = i.to_rgba8();
                egui::ColorImage::from_rgba_unmultiplied([i.width() as usize, i.height() as usize], i.as_raw())
            });
            (k, img)
        });
        Self { pool, tiles: HashMap::new(), frame: 0 }
    }
}

impl MapTiles {
    fn pump(&mut self, ctx: &egui::Context) {
        self.frame += 1;
        let mut n = 0;
        while n < 12 {
            let Some((k, img)) = self.pool.try_recv() else { break };
            n += 1;
            let e = self.tiles.entry(k).or_insert(Entry { tex: None, missing: false, last_used: self.frame, asked: self.frame });
            match img {
                Some(img) => {
                    e.tex = Some(ctx.load_texture(format!("map-{}-{}", k.imagery, k.id.path()), img, egui::TextureOptions::LINEAR))
                }
                None => e.missing = true,
            }
        }
        // Requests for tiles nobody has looked at for a second (the view moved on) are
        // dropped so they don't hold up the tiles on screen now; failed ones are tried
        // again after ten seconds.
        let frame = self.frame;
        let stale: Vec<MapKey> = self
            .tiles
            .iter()
            .filter(|(_, e)| e.tex.is_none() && (!e.missing && frame - e.last_used > 60 || e.missing && frame - e.asked > 600))
            .map(|(k, _)| *k)
            .collect();
        for k in stale {
            self.pool.cancel(key_of(k));
            self.tiles.remove(&k);
        }
        let with_tex = self.tiles.values().filter(|e| e.tex.is_some()).count();
        if with_tex > MAX_TEX {
            let mut v: Vec<(u64, MapKey)> = self.tiles.iter().filter(|(_, e)| e.tex.is_some()).map(|(k, e)| (e.last_used, *k)).collect();
            v.sort();
            for (_, k) in v.into_iter().take(with_tex - MAX_TEX) {
                self.tiles.remove(&k);
            }
        }
    }

    /// Texture for a tile (or a parent with the sub-rectangle to use), requesting it if needed.
    fn get(&mut self, imagery: bool, id: TileId, priority: f32) -> Option<(egui::TextureId, Rect)> {
        let frame = self.frame;
        let k = MapKey { imagery, id };
        match self.tiles.get_mut(&k) {
            Some(e) => {
                e.last_used = frame;
                if e.tex.is_none() && !e.missing {
                    self.pool.reprioritize(key_of(k), priority);
                }
            }
            None => {
                self.tiles.insert(k, Entry { tex: None, missing: false, last_used: frame, asked: frame });
                self.pool.submit(key_of(k), priority, k);
            }
        }
        for up in 0..=5u8 {
            if up > id.z {
                break;
            }
            let a = id.ancestor(id.z - up);
            if let Some(e) = self.tiles.get_mut(&MapKey { imagery, id: a }) {
                e.last_used = frame;
                if let Some(t) = &e.tex {
                    let (u, v, s) = id.rect_in_ancestor(a.z);
                    return Some((t.id(), Rect::from_min_size(Pos2::new(u as f32, v as f32), EVec2::splat(s as f32))));
                }
            }
        }
        None
    }
}

fn key_of(k: MapKey) -> u64 {
    ((k.imagery as u64) << 63) | ((k.id.z as u64) << 56) | ((k.id.x as u64) << 28) | k.id.y as u64
}

/// World pixel position of a point at a zoom level.
fn world_px(lat: f64, lon: f64, z: f64) -> (f64, f64) {
    let n = TILE * 2f64.powf(z);
    let lat = lat.clamp(-MAX_MERCATOR_LAT, MAX_MERCATOR_LAT).to_radians();
    ((lon + 180.0) / 360.0 * n, (1.0 - (lat.tan() + 1.0 / lat.cos()).ln() / std::f64::consts::PI) / 2.0 * n)
}

fn from_world_px(x: f64, y: f64, z: f64) -> (f64, f64) {
    let n = TILE * 2f64.powf(z);
    let lon = x / n * 360.0 - 180.0;
    let lat = (std::f64::consts::PI * (1.0 - 2.0 * y / n)).sinh().atan().to_degrees();
    (lat, lon)
}

/// A map view: what is in the middle, how far in, and how it is turned.
#[derive(Clone, Copy)]
struct View {
    lat: f64,
    lon: f64,
    zoom: f64,
    /// Radians; positive turns the map clockwise.
    rot: f32,
    rect: Rect,
}

impl View {
    fn to_screen(&self, lat: f64, lon: f64) -> Pos2 {
        let (cx, cy) = world_px(self.lat, self.lon, self.zoom);
        let (x, y) = world_px(lat, lon, self.zoom);
        let d = EVec2::new((x - cx) as f32, (y - cy) as f32);
        self.rect.center() + rotate(d, self.rot)
    }
    fn to_geo(&self, p: Pos2) -> (f64, f64) {
        let d = rotate(p - self.rect.center(), -self.rot);
        let (cx, cy) = world_px(self.lat, self.lon, self.zoom);
        from_world_px(cx + d.x as f64, cy + d.y as f64, self.zoom)
    }
}

fn rotate(v: EVec2, a: f32) -> EVec2 {
    let (s, c) = a.sin_cos();
    EVec2::new(v.x * c - v.y * s, v.x * s + v.y * c)
}

fn draw_tiles(painter: &egui::Painter, tiles: &mut MapTiles, v: View, imagery: bool) {
    let zi = (v.zoom.floor() as i64).clamp(0, 19) as u8;
    let scale = 2f64.powf(v.zoom - zi as f64);
    let (cx, cy) = world_px(v.lat, v.lon, zi as f64);
    let r = (v.rect.width().hypot(v.rect.height()) as f64 / 2.0) / scale;
    let n = 1i64 << zi;
    let (x0, x1) = (((cx - r) / TILE).floor() as i64, ((cx + r) / TILE).floor() as i64);
    let (y0, y1) = ((((cy - r) / TILE).floor() as i64).max(0), (((cy + r) / TILE).floor() as i64).min(n - 1));
    for ty in y0..=y1 {
        for tx in x0..=x1 {
            let wx = tx.rem_euclid(n) as u32;
            let id = TileId::new(zi, wx, ty as u32);
            let dist = ((tx as f64 + 0.5) * TILE - cx).hypot((ty as f64 + 0.5) * TILE - cy) / TILE;
            let Some((tex, uv)) = tiles.get(imagery, id, dist as f32) else { continue };
            let corner = |dx: f64, dy: f64| {
                let d = EVec2::new(((tx as f64 + dx) * TILE - cx) as f32, ((ty as f64 + dy) * TILE - cy) as f32) * scale as f32;
                v.rect.center() + rotate(d, v.rot)
            };
            let mut mesh = egui::Mesh::with_texture(tex);
            let i = mesh.vertices.len() as u32;
            for (dx, dy, u, vv) in [
                (0.0, 0.0, uv.min.x, uv.min.y),
                (1.0, 0.0, uv.max.x, uv.min.y),
                (1.0, 1.0, uv.max.x, uv.max.y),
                (0.0, 1.0, uv.min.x, uv.max.y),
            ] {
                mesh.vertices.push(egui::epaint::Vertex { pos: corner(dx, dy), uv: Pos2::new(u, vv), color: Color32::WHITE });
            }
            mesh.indices.extend_from_slice(&[i, i + 1, i + 2, i, i + 2, i + 3]);
            painter.add(egui::Shape::mesh(mesh));
        }
    }
}

fn arrow(painter: &egui::Painter, at: Pos2, angle: f32, size: f32, fill: Color32) {
    let pts: Vec<Pos2> =
        [(0.0, -1.0), (0.62, 0.75), (0.0, 0.38), (-0.62, 0.75)].iter().map(|&(x, y)| at + rotate(EVec2::new(x, y) * size, angle)).collect();
    painter.add(egui::Shape::convex_polygon(pts, fill, egui::Stroke::new(1.5, Color32::WHITE)));
}

fn pin(painter: &egui::Painter, at: Pos2, fill: Color32) {
    painter.circle(at - EVec2::new(0.0, 9.0), 6.0, fill, egui::Stroke::new(1.5, Color32::WHITE));
    painter.line_segment([at - EVec2::new(0.0, 3.0), at], egui::Stroke::new(2.0, fill));
}

/// Full-map state.
#[derive(Resource)]
pub struct FullMap {
    pub open: bool,
    lat: f64,
    lon: f64,
    zoom: f64,
    query: String,
    results: Vec<Place>,
    searching: Option<crossbeam_channel::Receiver<Result<Vec<Place>, String>>>,
    error: Option<String>,
    marker: Option<(f64, f64)>,
    focus: bool,
    naming: Vec<(usize, crossbeam_channel::Receiver<Option<String>>)>,
}

impl Default for FullMap {
    fn default() -> Self {
        Self {
            open: false,
            lat: 39.5,
            lon: -98.35,
            zoom: 4.0,
            query: String::new(),
            results: vec![],
            searching: None,
            error: None,
            marker: None,
            focus: false,
            naming: vec![],
        }
    }
}

impl FullMap {
    pub fn open_at(&mut self, lat: f64, lon: f64) {
        self.open = true;
        self.lat = lat;
        self.lon = lon;
        self.zoom = self.zoom.max(11.0);
        self.focus = true;
    }
}

/// Ask the player system to move somewhere.
#[derive(Message, Clone, Copy)]
pub struct TeleportTo {
    pub lat: f64,
    pub lon: f64,
    pub agl: f64,
    pub heading: Option<f64>,
}

/// Start naming a new bookmark from OpenStreetMap (reverse geocoding) in the background.
pub fn add_bookmark(settings: &mut usx_core::settings::Settings, map: &mut FullMap, lat: f64, lon: f64, agl: f64, heading: f64) {
    settings.bookmarks.push(Bookmark { name: format!("{lat:.4}, {lon:.4}"), lat, lon, agl, heading });
    let i = settings.bookmarks.len() - 1;
    let (tx, rx) = crossbeam_channel::bounded(1);
    std::thread::spawn(move || {
        let _ = tx.send(geocode::reverse(lat, lon).ok().flatten());
    });
    map.naming.push((i, rx));
    settings.save();
}

pub struct MapPlugin;

impl Plugin for MapPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<FullMap>()
            .add_message::<TeleportTo>()
            .add_systems(bevy_egui::EguiPrimaryContextPass, draw_maps.after(crate::ui::DrawUi));
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_maps(
    mut contexts: bevy_egui::EguiContexts,
    mut tiles: Local<Option<MapTiles>>,
    mut map: ResMut<FullMap>,
    mut settings: ResMut<crate::GameSettings>,
    player: Res<Player>,
    viewer: Res<crate::globe::Viewer>,
    menus: Res<crate::ui::Menus>,
    intent: Res<crate::controls::Intent>,
    mut tp: MessageWriter<TeleportTo>,
    mut blocked: ResMut<crate::camera::InputBlocked>,
) -> Result {
    let ctx = contexts.ctx_mut()?;
    let tiles = tiles.get_or_insert_with(MapTiles::default);
    tiles.pump(ctx);
    let g = player.geo();
    let imagery = settings.0.map_style == MapStyle::Imagery;

    // Finish bookmark names coming back from reverse geocoding.
    let mut done = vec![];
    for (k, (i, rx)) in map.naming.iter().enumerate() {
        if let Ok(name) = rx.try_recv() {
            if let (Some(n), Some(b)) = (name, settings.0.bookmarks.get_mut(*i)) {
                b.name = n;
            }
            done.push(k);
        }
    }
    if !done.is_empty() {
        for k in done.into_iter().rev() {
            map.naming.remove(k);
        }
        settings.0.save();
    }

    if intent.map && !menus.typing {
        if map.open {
            map.open = false;
        } else {
            map.open_at(g.lat, g.lon);
        }
    }
    if intent.bookmark && !map.open {
        let (s, m) = (&mut settings.0, &mut *map);
        add_bookmark(s, m, g.lat, g.lon, player.agl().unwrap_or(0.0), player.heading);
    }

    // ------------------------------------------------------------ minimap
    if settings.0.show_minimap && !map.open && !menus.pause && !menus.title {
        let size = 210.0;
        let screen = ctx.content_rect();
        let rect = Rect::from_min_size(Pos2::new(screen.max.x - size - 12.0, 12.0), EVec2::splat(size));
        let agl = player.agl().unwrap_or(0.0).max(1.0);
        let zoom = (17.3 - (agl / 120.0).max(1.0).log2()).clamp(5.0, 17.3);
        let rot = if settings.0.minimap_rotates { -(viewer.heading as f32).to_radians() } else { 0.0 };
        let v = View { lat: g.lat, lon: g.lon, zoom, rot, rect };
        egui::Area::new("minimap".into()).fixed_pos(rect.min).show(ctx, |ui| {
            let (resp, painter) = ui.allocate_painter(rect.size(), egui::Sense::click());
            let painter = painter.with_clip_rect(rect);
            painter.rect_filled(rect, 6.0, Color32::from_gray(40));
            draw_tiles(&painter, tiles, v, imagery);
            arrow(&painter, rect.center(), rot + (player.body_heading as f32).to_radians(), 9.0, Color32::from_rgb(30, 120, 230));
            // North marker on the rim.
            let n = rect.center() + rotate(EVec2::new(0.0, -size * 0.43), rot);
            painter.circle_filled(n, 9.0, Color32::from_black_alpha(160));
            painter.text(n, egui::Align2::CENTER_CENTER, "N", egui::FontId::proportional(12.0), Color32::WHITE);
            painter.rect_stroke(rect, 6.0, egui::Stroke::new(2.0, Color32::from_black_alpha(180)), egui::StrokeKind::Inside);
            if resp.clicked() {
                map.open_at(g.lat, g.lon);
            }
            resp.on_hover_text("Open the map (M)");
        });
    }

    // ----------------------------------------------------------- full map
    if !map.open {
        return Ok(());
    }
    let screen = ctx.content_rect();
    let side = 320.0f32.min(screen.width() * 0.35);
    let rect = Rect::from_min_max(Pos2::new(screen.min.x + side, screen.min.y), screen.max);
    let mut close = false;
    egui::Area::new("fullmap".into()).fixed_pos(rect.min).order(egui::Order::Background).show(ctx, |ui| {
        let (resp, painter) = ui.allocate_painter(rect.size(), egui::Sense::click_and_drag());
        let painter = painter.with_clip_rect(rect);
        painter.rect_filled(rect, 0.0, Color32::from_gray(30));
        // Pan and zoom.
        if resp.dragged() {
            let d = resp.drag_delta();
            let (cx, cy) = world_px(map.lat, map.lon, map.zoom);
            let (la, lo) = from_world_px(cx - d.x as f64, cy - d.y as f64, map.zoom);
            map.lat = la;
            map.lon = lo;
        }
        if resp.hovered() {
            let s = ui.input(|i| i.smooth_scroll_delta.y);
            if s != 0.0 {
                let before = View { lat: map.lat, lon: map.lon, zoom: map.zoom, rot: 0.0, rect };
                let ptr = resp.hover_pos().unwrap_or(rect.center());
                let anchor = before.to_geo(ptr);
                map.zoom = (map.zoom + s as f64 / 120.0).clamp(3.0, 19.0);
                // Keep the point under the cursor fixed while zooming.
                let after = View { lat: map.lat, lon: map.lon, zoom: map.zoom, rot: 0.0, rect };
                let moved = after.to_screen(anchor.0, anchor.1) - ptr;
                let (cx, cy) = world_px(map.lat, map.lon, map.zoom);
                let (la, lo) = from_world_px(cx + moved.x as f64, cy + moved.y as f64, map.zoom);
                map.lat = la;
                map.lon = lo;
            }
        }
        let v = View { lat: map.lat, lon: map.lon, zoom: map.zoom, rot: 0.0, rect };
        draw_tiles(&painter, tiles, v, imagery);
        for b in &settings.0.bookmarks {
            pin(&painter, v.to_screen(b.lat, b.lon), Color32::from_rgb(230, 160, 30));
        }
        if let Some((la, lo)) = map.marker {
            pin(&painter, v.to_screen(la, lo), Color32::from_rgb(220, 50, 60));
        }
        arrow(&painter, v.to_screen(g.lat, g.lon), (player.body_heading as f32).to_radians(), 11.0, Color32::from_rgb(30, 120, 230));
        if let Some(p) = resp.hover_pos() {
            let (la, lo) = v.to_geo(p);
            painter.text(
                rect.left_bottom() + EVec2::new(10.0, -10.0),
                egui::Align2::LEFT_BOTTOM,
                format!("{la:.5}, {lo:.5} · double-click to go there, right-click for more"),
                egui::FontId::proportional(13.0),
                Color32::WHITE,
            );
            if resp.double_clicked() {
                tp.write(TeleportTo { lat: la, lon: lo, agl: 300.0, heading: None });
                close = true;
            }
        }
        let attribution =
            if imagery { "Esri, Vantor, Earthstar Geographics" } else { "Esri, HERE, Garmin, USGS, © OpenStreetMap contributors" };
        painter.text(
            rect.right_bottom() + EVec2::new(-8.0, -6.0),
            egui::Align2::RIGHT_BOTTOM,
            attribution,
            egui::FontId::proportional(11.0),
            Color32::from_white_alpha(200),
        );
        let ptr_geo = resp.interact_pointer_pos().map(|p| v.to_geo(p));
        resp.context_menu(|ui| {
            if let Some((la, lo)) = ptr_geo {
                if ui.button("Teleport here").clicked() {
                    tp.write(TeleportTo { lat: la, lon: lo, agl: 300.0, heading: None });
                    close = true;
                    ui.close();
                }
                if ui.button("Teleport here, standing on the ground").clicked() {
                    tp.write(TeleportTo { lat: la, lon: lo, agl: 0.0, heading: None });
                    close = true;
                    ui.close();
                }
                if ui.button("Bookmark this spot").clicked() {
                    let (s, m) = (&mut settings.0, &mut *map);
                    add_bookmark(s, m, la, lo, 300.0, 0.0);
                    ui.close();
                }
            }
        });
    });

    let panel_fill = ctx.style_of(ctx.theme()).visuals.panel_fill;
    egui::Area::new("mapside".into()).fixed_pos(screen.min).order(egui::Order::Foreground).show(ctx, |ui| {
        egui::Frame::new().fill(panel_fill).inner_margin(10.0).show(ui, |ui| {
            ui.set_width(side - 20.0);
            ui.set_min_height(screen.height() - 20.0);
            ui.horizontal(|ui| {
                ui.heading("Map");
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button("Close (M)").clicked() {
                        close = true;
                    }
                });
            });
            ui.separator();
            let r = ui.add(
                egui::TextEdit::singleline(&mut map.query).hint_text("Search a place, address or coordinates").desired_width(f32::INFINITY),
            );
            if map.focus {
                r.request_focus();
                map.focus = false;
            }
            let enter = ui.input(|i| i.key_pressed(egui::Key::Enter)) && (r.lost_focus() || r.has_focus());
            if (enter || ui.button("Search").clicked()) && !map.query.trim().is_empty() && map.searching.is_none() {
                if let Some((la, lo)) = parse_lat_lon(&map.query) {
                    map.lat = la;
                    map.lon = lo;
                    map.zoom = map.zoom.max(13.0);
                    map.marker = Some((la, lo));
                } else {
                    let q = map.query.clone();
                    let (tx, rx) = crossbeam_channel::bounded(1);
                    std::thread::spawn(move || {
                        let _ = tx.send(geocode::search(&q).map_err(|e| e.to_string()));
                    });
                    map.searching = Some(rx);
                    map.error = None;
                }
            }
            if let Some(rx) = &map.searching {
                match rx.try_recv() {
                    Ok(Ok(res)) => {
                        map.error = res.is_empty().then(|| "No places found. Try adding the state.".to_string());
                        if let Some(p) = res.first() {
                            map.lat = p.lat;
                            map.lon = p.lon;
                            map.zoom = map.zoom.max(13.0);
                            map.marker = Some((p.lat, p.lon));
                        }
                        map.results = res;
                        map.searching = None;
                    }
                    Ok(Err(e)) => {
                        map.error = Some(format!("Search failed ({e})."));
                        map.searching = None;
                    }
                    Err(_) => {
                        ui.spinner();
                    }
                }
            }
            if let Some(e) = &map.error {
                ui.colored_label(Color32::from_rgb(230, 170, 60), e);
            }
            let mut go: Option<TeleportTo> = None;
            egui::ScrollArea::vertical().show(ui, |ui| {
                for p in map.results.clone() {
                    ui.horizontal(|ui| {
                        if ui.small_button("Go").clicked() {
                            go = Some(TeleportTo { lat: p.lat, lon: p.lon, agl: 300.0, heading: None });
                        }
                        if ui.add(egui::Label::new(egui::RichText::new(&p.name).small()).wrap().sense(egui::Sense::click())).clicked() {
                            map.lat = p.lat;
                            map.lon = p.lon;
                            map.marker = Some((p.lat, p.lon));
                        }
                    });
                }
                ui.separator();
                ui.horizontal(|ui| {
                    ui.strong("Bookmarks");
                    if ui.small_button("+ Here").on_hover_text("Bookmark where you are (B)").clicked() {
                        let (s, m) = (&mut settings.0, &mut *map);
                        add_bookmark(s, m, g.lat, g.lon, player.agl().unwrap_or(0.0), player.heading);
                    }
                });
                if settings.0.bookmarks.is_empty() {
                    ui.small("None yet. Press B anywhere, or right-click the map.");
                }
                let mut remove = None;
                let mut changed = false;
                for (i, b) in settings.0.bookmarks.iter_mut().enumerate() {
                    ui.horizontal(|ui| {
                        if ui.small_button("Go").clicked() {
                            go = Some(TeleportTo { lat: b.lat, lon: b.lon, agl: b.agl, heading: Some(b.heading) });
                        }
                        changed |= ui.add(egui::TextEdit::singleline(&mut b.name).desired_width(side - 110.0)).lost_focus();
                        if ui.small_button("✕").on_hover_text("Delete").clicked() {
                            remove = Some(i);
                        }
                    });
                }
                if let Some(i) = remove {
                    settings.0.bookmarks.remove(i);
                    changed = true;
                }
                if changed {
                    settings.0.save();
                }
                ui.separator();
                ui.strong("Recent places");
                for r in settings.0.recent_places.clone() {
                    ui.horizontal(|ui| {
                        if ui.small_button("Go").clicked() {
                            go = Some(TeleportTo { lat: r.lat, lon: r.lon, agl: r.agl, heading: Some(r.heading) });
                        }
                        ui.small(&r.name);
                    });
                }
                ui.separator();
                ui.strong("Layers");
                let mut style = settings.0.map_style;
                ui.radio_value(&mut style, MapStyle::Streets, "Streets");
                ui.radio_value(&mut style, MapStyle::Imagery, "Satellite imagery");
                if style != settings.0.map_style {
                    settings.0.map_style = style;
                    settings.0.save();
                }
                ui.add_enabled_ui(false, |ui| {
                    for (name, phase) in
                        [("Radar", 9), ("Clouds", 8), ("Warnings", 10), ("Hurricanes", 11), ("Fires and smoke", 12), ("Aircraft", 13)]
                    {
                        let mut off = false;
                        ui.checkbox(&mut off, format!("{name} (Phase {phase})"));
                    }
                });
                ui.separator();
                let mut rot = settings.0.minimap_rotates;
                if ui.checkbox(&mut rot, "Minimap turns with you").changed() {
                    settings.0.minimap_rotates = rot;
                    settings.0.save();
                }
                let mut show = settings.0.show_minimap;
                if ui.checkbox(&mut show, "Show the minimap").changed() {
                    settings.0.show_minimap = show;
                    settings.0.save();
                }
            });
            if let Some(t) = go {
                tp.write(t);
                close = true;
            }
        });
    });
    if close || ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
        map.open = false;
    }
    blocked.keyboard |= ctx.egui_wants_keyboard_input();
    blocked.pointer = true;
    let _ = Geodetic::default();
    Ok(())
}
