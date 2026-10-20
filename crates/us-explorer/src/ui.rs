//! Menus, the debug overlay, place search, settings, credits and the update popup.

use crate::camera::{FlySpeed, InputBlocked, MouseLock};
use crate::globe::Viewer;
use crate::sun::SimClock;
use crate::terrain::region::RegionDownload;
use crate::terrain::{Teleported, Terrain, TerrainStats};
use crate::updater::{UpdateState, Updater};
use crate::weather_link::{LinkState, WeatherLink};
use bevy::diagnostic::{DiagnosticsStore, FrameTimeDiagnosticsPlugin};
use bevy::ecs::system::SystemParam;
use bevy::prelude::*;
use bevy_egui::{EguiContexts, EguiPrimaryContextPass, egui};
use std::sync::atomic::Ordering;
use std::time::Instant;
use usx_core::geo::{Geodetic, parse_lat_lon};
use usx_core::geocode::{self, Place};
use usx_core::settings::{Preset, Units};
use usx_core::sources::{CREDITS, ImagerySource};

#[derive(Resource, Default)]
pub struct Menus {
    pub pause: bool,
    pub teleport: bool,
    pub settings: bool,
    pub credits: bool,
    pub region: bool,
    query: String,
    results: Vec<Place>,
    searching: Option<crossbeam_channel::Receiver<Result<Vec<Place>, String>>>,
    search_error: Option<String>,
    draft: Option<usx_core::settings::Settings>,
    region_radius_km: f64,
    region_zoom: u8,
    restart_needed: bool,
}

#[derive(Resource)]
struct SaveTimer(Instant);

pub struct UiPlugin;

impl Plugin for UiPlugin {
    fn build(&self, app: &mut App) {
        app.insert_resource(Menus {
            region_radius_km: 5.0,
            region_zoom: 17,
            ..default()
        })
        .insert_resource(SaveTimer(Instant::now()))
        .add_systems(Update, (hotkeys, persist_position))
        .add_systems(EguiPrimaryContextPass, draw);
    }
}

fn hotkeys(
    keys: Res<ButtonInput<KeyCode>>,
    blocked: Res<InputBlocked>,
    mut menus: ResMut<Menus>,
    mut settings: ResMut<crate::GameSettings>,
) {
    if keys.just_pressed(KeyCode::Escape) {
        if menus.teleport || menus.settings || menus.credits || menus.region {
            menus.teleport = false;
            menus.settings = false;
            menus.credits = false;
            menus.region = false;
        } else {
            menus.pause = !menus.pause;
        }
    }
    if blocked.keyboard {
        return;
    }
    if keys.just_pressed(KeyCode::KeyT) || keys.just_pressed(KeyCode::KeyM) {
        menus.teleport = true;
    }
    if keys.just_pressed(KeyCode::F3) {
        settings.0.show_debug_overlay = !settings.0.show_debug_overlay;
        settings.0.save();
    }
}

/// Remember where the player was, so the game reopens in the same place.
fn persist_position(
    mut t: ResMut<SaveTimer>,
    viewer: Res<Viewer>,
    mut settings: ResMut<crate::GameSettings>,
    mut exit: MessageReader<AppExit>,
    args: Res<crate::Args>,
) {
    if !args.shots.is_empty() {
        return;
    }
    let exiting = exit.read().next().is_some();
    if exiting || t.0.elapsed().as_secs() > 30 {
        t.0 = Instant::now();
        settings.0.last_position = Some(viewer.geo());
        settings.0.last_heading_deg = viewer.heading;
        settings.0.save();
    }
}

pub fn teleport(
    viewer: &mut Viewer,
    terrain: &Terrain,
    lat: f64,
    lon: f64,
    tp: &mut MessageWriter<Teleported>,
) {
    let ground = terrain.height_at(lat, lon).unwrap_or(0.0);
    viewer.ecef = Geodetic::new(lat, lon, ground + 350.0).to_ecef();
    viewer.ground = None;
    viewer.hold_agl = Some(350.0);
    viewer.pitch = -15.0;
    tp.write(Teleported);
}

fn fmt_len(m: f64, units: Units) -> String {
    match units {
        Units::Imperial => format!("{:.0} ft", m * 3.28084),
        Units::Metric => format!("{m:.0} m"),
    }
}

fn fmt_speed(mps: f64, units: Units) -> String {
    match units {
        Units::Imperial => format!("{:.0} mph", mps * 2.23694),
        Units::Metric => format!("{:.0} km/h", mps * 3.6),
    }
}

#[derive(SystemParam)]
struct World<'w> {
    viewer: ResMut<'w, Viewer>,
    terrain: ResMut<'w, Terrain>,
    stats: Res<'w, TerrainStats>,
    link: Res<'w, WeatherLink>,
    region: ResMut<'w, RegionDownload>,
    clock: ResMut<'w, SimClock>,
    speed: Res<'w, FlySpeed>,
    diagnostics: Res<'w, DiagnosticsStore>,
}

#[derive(SystemParam)]
struct Out<'w> {
    blocked: ResMut<'w, InputBlocked>,
    lock: ResMut<'w, MouseLock>,
    tp: MessageWriter<'w, Teleported>,
    exit: MessageWriter<'w, AppExit>,
}

fn draw(
    mut contexts: EguiContexts,
    mut menus: ResMut<Menus>,
    mut settings: ResMut<crate::GameSettings>,
    mut updater: ResMut<Updater>,
    w: World,
    o: Out,
    mut commands: Commands,
) -> Result {
    let World {
        mut viewer,
        mut terrain,
        stats,
        link,
        mut region,
        mut clock,
        speed,
        diagnostics,
    } = w;
    let Out {
        mut blocked,
        mut lock,
        mut tp,
        mut exit,
    } = o;
    let ctx = contexts.ctx_mut()?;
    let units = settings.0.units;
    let geo = viewer.geo();

    // ------------------------------------------------------------- overlay
    if settings.0.show_debug_overlay {
        let fps = diagnostics
            .get(&FrameTimeDiagnosticsPlugin::FPS)
            .and_then(|d| d.smoothed())
            .unwrap_or(0.0);
        let ms = diagnostics
            .get(&FrameTimeDiagnosticsPlugin::FRAME_TIME)
            .and_then(|d| d.smoothed())
            .unwrap_or(0.0);
        let net = usx_core::net::stats();
        let cache = crate::terrain::load::cache();
        egui::Area::new("overlay".into())
            .fixed_pos([10.0, 10.0])
            .interactable(false)
            .show(ctx, |ui| {
                egui::Frame::new()
                    .fill(egui::Color32::from_black_alpha(150))
                    .inner_margin(8.0)
                    .corner_radius(4.0)
                    .show(ui, |ui| {
                        ui.set_min_width(360.0);
                        ui.style_mut().override_text_style = Some(egui::TextStyle::Monospace);
                        ui.visuals_mut().override_text_color = Some(egui::Color32::from_gray(230));
                        ui.label(format!("{fps:5.0} fps  {ms:5.1} ms"));
                        ui.label(format!("{:.5}, {:.5}", geo.lat, geo.lon));
                        ui.label(format!(
                            "alt {}  agl {}  hdg {:03.0}°",
                            fmt_len(geo.h, units),
                            viewer
                                .agl()
                                .map(|a| fmt_len(a, units))
                                .unwrap_or("…".into()),
                            viewer.heading
                        ));
                        ui.label(format!(
                            "speed {}  (wheel: {})",
                            fmt_speed(viewer.velocity.length(), units),
                            fmt_speed(speed.0, units)
                        ));
                        ui.label(format!(
                            "tiles {} shown / {} loaded / {} queued  z{}  detail x{:.2}",
                            stats.visible,
                            stats.loaded,
                            stats.pending,
                            stats.deepest_visible,
                            terrain.adapt
                        ));
                        ui.label(format!(
                            "tile z{} (imagery z{})  elev: {}",
                            stats.imagery_zoom_here,
                            stats.imagery_source_zoom,
                            stats.elevation_source
                        ));
                        ui.label(format!(
                            "terrain VRAM ~{:.0} MB",
                            stats.gpu_bytes as f64 / 1e6
                        ));
                        ui.label(format!(
                            "net {:.1} MB, {} req, {} err, cap {:.0} Mbps",
                            net.bytes.load(Ordering::Relaxed) as f64 / 1e6,
                            net.requests.load(Ordering::Relaxed),
                            net.errors.load(Ordering::Relaxed),
                            usx_core::net::limiter().mbps()
                        ));
                        ui.label(format!(
                            "cache {:.2} / {:.0} GB ({} files)",
                            cache.size() as f64 / 1e9,
                            cache.limit() as f64 / 1e9,
                            cache.len()
                        ));
                        ui.label(match &link.state {
                            LinkState::Connected { version, rtt_ms } => format!(
                                "weather service v{version}, ping {}",
                                rtt_ms.map(|r| format!("{r:.0} ms")).unwrap_or("…".into())
                            ),
                            LinkState::Starting => "weather service starting…".into(),
                            LinkState::Disabled => "weather service off".into(),
                            LinkState::Failed(e) => format!("weather service: {e}"),
                        });
                        ui.label(format!(
                            "{}{}",
                            crate::sun::format_utc(clock.time()),
                            if clock.offset != 0.0 {
                                "  (shifted)"
                            } else {
                                ""
                            }
                        ));
                        if let Some(s) = stats.settle {
                            ui.label(format!("last arrival loaded in {s:.1} s"));
                        }
                    });
            });
    }

    // Streaming indicator.
    if stats.pending > 0 || region.active() {
        egui::Area::new("loading".into())
            .anchor(egui::Align2::RIGHT_BOTTOM, [-12.0, -12.0])
            .interactable(false)
            .show(ctx, |ui| {
                egui::Frame::new()
                    .fill(egui::Color32::from_black_alpha(140))
                    .inner_margin(6.0)
                    .corner_radius(4.0)
                    .show(ui, |ui| {
                        ui.visuals_mut().override_text_color = Some(egui::Color32::from_gray(225));
                        if stats.pending > 0 {
                            ui.horizontal(|ui| {
                                ui.spinner();
                                ui.label(format!(
                                    "Loading terrain and imagery · {}",
                                    stats.pending
                                ));
                            });
                        }
                        if region.active() {
                            ui.label(format!(
                                "Offline download: {} / {} tiles",
                                region.finished(),
                                region.total
                            ));
                        }
                    });
            });
    }

    // Controls hint.
    if !menus.pause {
        egui::Area::new("hint".into()).anchor(egui::Align2::LEFT_BOTTOM, [10.0, -10.0]).interactable(false).show(ctx, |ui| {
            egui::Frame::new().fill(egui::Color32::from_black_alpha(120)).inner_margin(egui::Margin::symmetric(8, 4)).corner_radius(4.0).show(ui, |ui| {
                ui.visuals_mut().override_text_color = Some(egui::Color32::from_gray(225));
                ui.small("Right mouse or Tab: look · WASD move · Space/Ctrl up/down · Shift fast · wheel speed · T go to place · Esc menu · F3 info");
            });
        });
    }

    // ---------------------------------------------------------------- pause
    if menus.pause {
        egui::Window::new("US Explorer")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, [0.0, 0.0])
            .show(ctx, |ui| {
                ui.set_min_width(220.0);
                ui.label(format!("Version {}", usx_core::VERSION));
                ui.separator();
                let w = egui::vec2(220.0, 28.0);
                if ui.add_sized(w, egui::Button::new("Resume")).clicked() {
                    menus.pause = false;
                }
                if ui
                    .add_sized(w, egui::Button::new("Go to a place…"))
                    .clicked()
                {
                    menus.teleport = true;
                    menus.pause = false;
                }
                if ui.add_sized(w, egui::Button::new("Settings")).clicked() {
                    menus.settings = true;
                    menus.draft = Some(settings.0.clone());
                }
                if ui
                    .add_sized(w, egui::Button::new("Offline download…"))
                    .clicked()
                {
                    menus.region = true;
                }
                if ui.add_sized(w, egui::Button::new("Credits")).clicked() {
                    menus.credits = true;
                }
                if ui
                    .add_sized(w, egui::Button::new("Check for updates"))
                    .clicked()
                {
                    updater.check_now();
                }
                match &updater.state {
                    UpdateState::Checking(_) => {
                        ui.label("Checking…");
                    }
                    UpdateState::UpToDate => {
                        ui.label("You have the latest version.");
                    }
                    UpdateState::Failed(e) => {
                        ui.colored_label(egui::Color32::from_rgb(230, 170, 60), e);
                    }
                    _ => {}
                }
                ui.separator();
                if ui.add_sized(w, egui::Button::new("Quit")).clicked() {
                    exit.write(AppExit::Success);
                }
            });
    }

    // ------------------------------------------------------------- teleport
    if menus.teleport {
        let mut open = true;
        egui::Window::new("Go to a place").open(&mut open).collapsible(false).default_width(420.0).anchor(egui::Align2::CENTER_TOP, [0.0, 60.0]).show(ctx, |ui| {
            ui.label("Type an address, a place name or coordinates (e.g. 36.1069, -112.1129), then press Enter.");
            let r = ui.add(egui::TextEdit::singleline(&mut menus.query).desired_width(f32::INFINITY).hint_text("Grand Canyon Village, AZ"));
            r.request_focus();
            let go = r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            if go && !menus.query.trim().is_empty() {
                if let Some((lat, lon)) = parse_lat_lon(&menus.query) {
                    teleport(&mut viewer, &terrain, lat, lon, &mut tp);
                    menus.teleport = false;
                } else if menus.searching.is_none() {
                    let q = menus.query.clone();
                    let (tx, rx) = crossbeam_channel::bounded(1);
                    std::thread::spawn(move || {
                        let _ = tx.send(geocode::search(&q).map_err(|e| e.to_string()));
                    });
                    menus.searching = Some(rx);
                    menus.search_error = None;
                }
            }
            if let Some(rx) = &menus.searching {
                match rx.try_recv() {
                    Ok(Ok(res)) => {
                        menus.search_error = res.is_empty().then(|| "No places found. Try adding the state, or use coordinates.".to_string());
                        menus.results = res;
                        menus.searching = None;
                    }
                    Ok(Err(e)) => {
                        menus.search_error = Some(format!("Search failed ({e}). Check your connection, or use coordinates."));
                        menus.searching = None;
                    }
                    Err(_) => {
                        ui.horizontal(|ui| {
                            ui.spinner();
                            ui.label("Searching…");
                        });
                    }
                }
            }
            if let Some(e) = &menus.search_error {
                ui.colored_label(egui::Color32::from_rgb(230, 170, 60), e);
            }
            let mut pick = None;
            for p in &menus.results {
                if ui.add(egui::Button::new(&p.name).wrap()).clicked() {
                    pick = Some((p.lat, p.lon));
                }
            }
            if let Some((lat, lon)) = pick {
                teleport(&mut viewer, &terrain, lat, lon, &mut tp);
                menus.teleport = false;
            }
            ui.separator();
            ui.horizontal(|ui| {
                ui.label(format!("You are at {:.5}, {:.5}", geo.lat, geo.lon));
                if ui.small_button("Copy").clicked() {
                    ui.ctx().copy_text(format!("{:.6}, {:.6}", geo.lat, geo.lon));
                }
            });
        });
        menus.teleport &= open;
    }

    // ------------------------------------------------------------- settings
    if menus.settings {
        let mut open = true;
        let mut draft = menus.draft.clone().unwrap_or_else(|| settings.0.clone());
        let mut apply = false;
        egui::Window::new("Settings")
            .open(&mut open)
            .collapsible(false)
            .pivot(egui::Align2::CENTER_CENTER)
            .default_pos(ctx.content_rect().center())
            .default_width(380.0)
            .show(ctx, |ui| {
                egui::Grid::new("settings")
                    .num_columns(2)
                    .spacing([16.0, 8.0])
                    .show(ui, |ui| {
                        ui.label("Graphics preset");
                        egui::ComboBox::from_id_salt("preset")
                            .selected_text(draft.preset.label())
                            .show_ui(ui, |ui| {
                                for p in Preset::ALL {
                                    ui.selectable_value(&mut draft.preset, p, p.label());
                                }
                            });
                        ui.end_row();
                        ui.label("Imagery");
                        egui::ComboBox::from_id_salt("imagery")
                            .selected_text(draft.imagery.label())
                            .show_ui(ui, |ui| {
                                for s in [ImagerySource::Esri, ImagerySource::Usgs] {
                                    ui.selectable_value(&mut draft.imagery, s, s.label());
                                }
                            });
                        ui.end_row();
                        ui.label("Lidar elevation");
                        ui.checkbox(
                            &mut draft.lidar_elevation,
                            "Use USGS 3DEP lidar where available",
                        );
                        ui.end_row();
                        ui.label("Units");
                        ui.horizontal(|ui| {
                            ui.radio_value(&mut draft.units, Units::Imperial, "Imperial");
                            ui.radio_value(&mut draft.units, Units::Metric, "Metric");
                        });
                        ui.end_row();
                        ui.label("Field of view");
                        ui.add(egui::Slider::new(&mut draft.fov_deg, 50.0..=110.0).suffix("°"));
                        ui.end_row();
                        ui.label("Mouse sensitivity");
                        ui.add(egui::Slider::new(&mut draft.mouse_sensitivity, 0.2..=3.0));
                        ui.end_row();
                        ui.label("VSync");
                        ui.checkbox(
                            &mut draft.vsync,
                            "Limit to the display's refresh rate (restart)",
                        );
                        ui.end_row();
                        ui.label("Download cap");
                        ui.add(
                            egui::Slider::new(&mut draft.bandwidth_mbps, 10.0..=1000.0)
                                .suffix(" Mbps")
                                .logarithmic(true),
                        );
                        ui.end_row();
                        ui.label("Cache size");
                        ui.add(
                            egui::Slider::new(&mut draft.cache_size_gb, 2.0..=500.0)
                                .suffix(" GB")
                                .logarithmic(true),
                        );
                        ui.end_row();
                        ui.label("Cache folder");
                        let mut path = draft.cache_dir().display().to_string();
                        if ui.text_edit_singleline(&mut path).changed() {
                            draft.cache_dir = Some(path.into());
                        }
                        ui.end_row();
                        ui.label("Streaming");
                        ui.checkbox(
                            &mut draft.cap_speed_while_loading,
                            "Slow flight while terrain loads",
                        );
                        ui.end_row();
                        ui.label("Updates");
                        ui.checkbox(&mut draft.check_updates, "Check for new versions at start");
                        ui.end_row();
                        ui.label("Overlay");
                        ui.checkbox(&mut draft.show_debug_overlay, "Show info overlay (F3)");
                        ui.end_row();
                    });
                ui.separator();
                ui.horizontal(|ui| {
                    if ui.button("Save").clicked() {
                        apply = true;
                    }
                    if ui.button("Clear cache").clicked() {
                        crate::terrain::load::cache().clear();
                    }
                    if menus.restart_needed {
                        ui.label("Some changes apply after a restart.");
                    }
                });
            });
        if apply {
            let old = settings.0.clone();
            menus.restart_needed |= draft.cache_dir != old.cache_dir || draft.vsync != old.vsync;
            usx_core::net::limiter().set_mbps(draft.bandwidth_mbps * 0.9);
            crate::terrain::load::cache().set_limit(draft.cache_bytes());
            if draft.imagery != old.imagery
                || draft.lidar_elevation != old.lidar_elevation
                || draft.preset != old.preset
            {
                terrain.reset(&mut commands, &draft);
            }
            settings.0 = draft.clone();
            settings.0.save();
        }
        menus.draft = Some(draft);
        menus.settings &= open;
    }

    // -------------------------------------------------------------- credits
    if menus.credits {
        let mut open = true;
        egui::Window::new("Credits")
            .open(&mut open)
            .collapsible(false)
            .pivot(egui::Align2::CENTER_CENTER)
            .default_pos(ctx.content_rect().center())
            .default_width(460.0)
            .show(ctx, |ui| {
                ui.label("US Explorer is built on open data and open-source software.");
                egui::ScrollArea::vertical()
                    .max_height(420.0)
                    .show(ui, |ui| {
                        for c in CREDITS {
                            ui.separator();
                            ui.strong(c.name);
                            ui.small(c.used_for);
                            ui.label(c.attribution);
                            ui.hyperlink(c.url);
                        }
                    });
            });
        menus.credits &= open;
    }

    // ------------------------------------------------------ offline region
    if menus.region {
        let mut open = true;
        egui::Window::new("Offline download")
            .open(&mut open)
            .collapsible(false)
            .pivot(egui::Align2::CENTER_CENTER)
            .default_pos(ctx.content_rect().center())
            .default_width(380.0)
            .show(ctx, |ui| {
                ui.label("Save the area around you to the cache so it works without a connection.");
                ui.add(
                    egui::Slider::new(&mut menus.region_radius_km, 1.0..=60.0).text("radius (km)"),
                );
                ui.add(
                    egui::Slider::new(&mut menus.region_zoom, 12..=19).text("detail (zoom level)"),
                );
                let plan = RegionDownload::plan(
                    geo.lat,
                    geo.lon,
                    menus.region_radius_km,
                    menus.region_zoom,
                );
                ui.label(format!(
                    "{} tiles, about {:.0} MB",
                    plan.len(),
                    RegionDownload::estimate_mb(&plan)
                ));
                if region.active() {
                    let frac = region.finished() as f32 / region.total.max(1) as f32;
                    ui.add(egui::ProgressBar::new(frac).text(format!(
                        "{} / {}",
                        region.finished(),
                        region.total
                    )));
                    if ui.button("Stop").clicked() {
                        region.stop();
                    }
                } else if ui.button("Download").clicked() {
                    let label = format!("{:.3}, {:.3}", geo.lat, geo.lon);
                    region.start(plan, settings.0.imagery, label);
                }
            });
        menus.region &= open;
    }

    // ---------------------------------------------------------- update popup
    let mut dismiss = false;
    let mut skip = None;
    let mut install = false;
    match &updater.state {
        UpdateState::Available(rel) if !updater.dismissed => {
            egui::Window::new("Update available")
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_BOTTOM, [0.0, -40.0])
                .show(ctx, |ui| {
                    ui.label(format!(
                        "US Explorer {} is out. You have {}.",
                        rel.version,
                        usx_core::VERSION
                    ));
                    if !rel.notes.is_empty() {
                        egui::CollapsingHeader::new("What's new").show(ui, |ui| {
                            egui::ScrollArea::vertical()
                                .max_height(160.0)
                                .show(ui, |ui| ui.label(&rel.notes));
                        });
                    }
                    ui.horizontal(|ui| {
                        if usx_core::update::can_self_install() && rel.appimage_url.is_some() {
                            if ui
                                .button(format!(
                                    "Update now ({:.0} MB)",
                                    rel.appimage_size as f64 / 1e6
                                ))
                                .clicked()
                            {
                                install = true;
                            }
                        } else {
                            ui.hyperlink_to("Open the download page", &rel.url);
                        }
                        if ui.button("Later").clicked() {
                            dismiss = true;
                        }
                        if ui.button("Skip this version").clicked() {
                            skip = Some(rel.version.to_string());
                        }
                    });
                });
        }
        UpdateState::Downloading {
            rel, done, total, ..
        } => {
            egui::Window::new("Updating")
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_BOTTOM, [0.0, -40.0])
                .show(ctx, |ui| {
                    let (d, t) = (
                        done.load(Ordering::Relaxed),
                        total.load(Ordering::Relaxed).max(1),
                    );
                    ui.label(format!(
                        "Downloading US Explorer {}… the game restarts when it's done.",
                        rel.version
                    ));
                    ui.add(egui::ProgressBar::new(d as f32 / t as f32).text(format!(
                        "{:.0} / {:.0} MB",
                        d as f64 / 1e6,
                        t as f64 / 1e6
                    )));
                });
        }
        _ => {}
    }
    if install {
        updater.install();
    }
    if dismiss {
        updater.dismissed = true;
    }
    if let Some(v) = skip {
        settings.0.skipped_version = Some(v);
        settings.0.save();
        updater.dismissed = true;
    }

    // Time shift for checking lighting: [ and ] move the sun an hour.
    if !ctx.egui_wants_keyboard_input() {
        ctx.input(|i| {
            if i.key_pressed(egui::Key::OpenBracket) {
                clock.offset -= 3600.0;
            }
            if i.key_pressed(egui::Key::CloseBracket) {
                clock.offset += 3600.0;
            }
            if i.key_pressed(egui::Key::Backslash) {
                clock.offset = 0.0;
            }
        });
    }

    let any_menu = menus.pause || menus.teleport || menus.settings || menus.credits || menus.region;
    blocked.keyboard = ctx.egui_wants_keyboard_input() || menus.teleport;
    blocked.pointer = ctx.is_pointer_over_egui() || ctx.egui_wants_pointer_input();
    if any_menu {
        lock.0 = false;
    }
    Ok(())
}
