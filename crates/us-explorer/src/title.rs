//! The title screen the game opens on: Play (choose where to start), Resume (carry on
//! from the last place), Customize (the character) and Settings. The world loads behind
//! it at the last position, with the camera slowly circling the player.

use crate::map::TeleportTo;
use crate::player::Player;
use crate::ui::Menus;
use bevy::prelude::*;
use bevy_egui::{EguiContexts, EguiPrimaryContextPass, egui};
use usx_core::geo::parse_lat_lon;
use usx_core::geocode::{self, Place};
use usx_core::settings::{Character, HairStyle, Outfit};

#[derive(Clone, Copy, PartialEq, Eq, Default)]
enum Page {
    #[default]
    Main,
    Play,
    Customize,
}

#[derive(Resource, Default)]
pub struct Title {
    pub open: bool,
    page: Page,
    query: String,
    results: Vec<Place>,
    searching: Option<crossbeam_channel::Receiver<Result<Vec<Place>, String>>>,
    error: Option<String>,
    spin: f64,
    /// Character being edited (saved on Done).
    draft: Option<(Character, Outfit)>,
}

/// Well-known places to start from.
const FEATURED: &[(&str, f64, f64, f64)] = &[
    ("Times Square, New York", 40.7580, -73.9855, 0.0),
    ("Golden Gate Bridge, San Francisco", 37.8199, -122.4783, 60.0),
    ("Grand Canyon (South Rim)", 36.0590, -112.1090, 0.0),
    ("Downtown Denver", 39.7487, -104.9945, 0.0),
    ("The Loop, Chicago", 41.8826, -87.6233, 0.0),
    ("National Mall, Washington DC", 38.8895, -77.0230, 0.0),
    ("Las Vegas Strip", 36.1147, -115.1728, 0.0),
    ("Space Needle, Seattle", 47.6205, -122.3493, 0.0),
    ("Yosemite Valley", 37.7400, -119.5850, 0.0),
    ("South Beach, Miami", 25.7826, -80.1341, 0.0),
    ("Downtown Austin", 30.2672, -97.7431, 0.0),
    ("Waikiki, Honolulu", 21.2766, -157.8278, 0.0),
];

pub struct TitlePlugin;

impl Plugin for TitlePlugin {
    fn build(&self, app: &mut App) {
        let args = app.world().resource::<crate::Args>().clone();
        let s = &app.world().resource::<crate::GameSettings>().0;
        let open = s.title_screen && args.shots.is_empty() && args.start.is_none();
        app.insert_resource(Title { open, ..default() })
            .add_systems(Update, showcase.before(crate::ui::UiInput))
            .add_systems(EguiPrimaryContextPass, draw.before(crate::ui::DrawUi));
    }
}

fn showcase(time: Res<Time>, mut title: ResMut<Title>, mut menus: ResMut<Menus>, mut p: ResMut<Player>) {
    menus.title = title.open;
    if !title.open {
        if p.showcase.is_some() {
            p.showcase = None;
        }
        return;
    }
    match title.page {
        // Face the character for customising.
        Page::Customize => {
            p.showcase = Some(((p.body_heading + 180.0).rem_euclid(360.0) + 20.0, -6.0, 3.2));
        }
        _ => {
            title.spin = (title.spin + time.delta_secs_f64() * 4.0).rem_euclid(360.0);
            p.showcase = Some((title.spin, -12.0, 9.0));
        }
    }
}

fn big_button(ui: &mut egui::Ui, text: &str) -> bool {
    ui.add_sized(egui::vec2(260.0, 42.0), egui::Button::new(egui::RichText::new(text).size(19.0))).clicked()
}

fn color_row(ui: &mut egui::Ui, label: &str, c: &mut [u8; 3]) {
    ui.label(label);
    ui.color_edit_button_srgb(c);
    ui.end_row();
}

#[allow(clippy::too_many_arguments)]
fn draw(
    mut contexts: EguiContexts,
    mut title: ResMut<Title>,
    mut menus: ResMut<Menus>,
    mut settings: ResMut<crate::GameSettings>,
    mut tp: MessageWriter<TeleportTo>,
    mut map: ResMut<crate::map::FullMap>,
    player: Res<Player>,
    mut exit: MessageWriter<AppExit>,
) -> Result {
    if !title.open {
        return Ok(());
    }
    let ctx = contexts.ctx_mut()?;
    let screen = ctx.content_rect();
    // A dark band down the left so the menu reads over any scenery.
    egui::Area::new("title-shade".into()).fixed_pos(screen.min).order(egui::Order::Background).interactable(false).show(ctx, |ui| {
        let r = egui::Rect::from_min_size(screen.min, egui::vec2(380.0, screen.height()));
        ui.painter().rect_filled(r, 0.0, egui::Color32::from_black_alpha(150));
    });
    let mut close = false;
    egui::Area::new("title".into()).fixed_pos(screen.min + egui::vec2(48.0, 70.0)).show(ctx, |ui| {
        ui.set_max_width(290.0);
        ui.visuals_mut().override_text_color = Some(egui::Color32::from_gray(235));
        ui.label(egui::RichText::new("US Explorer").size(40.0).strong());
        ui.label(egui::RichText::new(format!("Version {}", usx_core::VERSION)).size(13.0).color(egui::Color32::from_gray(170)));
        ui.add_space(28.0);
        match title.page {
            Page::Main => {
                if big_button(ui, "Play") {
                    title.page = Page::Play;
                }
                ui.add_space(6.0);
                let has_last = settings.0.last_position.is_some();
                if ui.add_enabled_ui(has_last, |ui| big_button(ui, "Resume")).inner {
                    close = true;
                }
                if !has_last {
                    ui.small("Nothing to resume yet: press Play to choose a place.");
                }
                ui.add_space(6.0);
                if big_button(ui, "Customize") {
                    title.draft = Some((settings.0.character.clone(), settings.0.outfit));
                    title.page = Page::Customize;
                }
                ui.add_space(6.0);
                if big_button(ui, "Settings") {
                    menus.settings = true;
                    menus.open_settings_draft(&settings.0);
                }
                ui.add_space(6.0);
                if big_button(ui, "Quit") {
                    exit.write(AppExit::Success);
                }
                ui.add_space(18.0);
                let mut show = settings.0.title_screen;
                if ui.checkbox(&mut show, "Show this screen when the game starts").changed() {
                    settings.0.title_screen = show;
                    settings.0.save();
                }
            }
            Page::Play => {
                ui.label(egui::RichText::new("Where do you want to start?").size(18.0));
                ui.add_space(6.0);
                let r =
                    ui.add(egui::TextEdit::singleline(&mut title.query).desired_width(280.0).hint_text("An address, place or lat, lon"));
                let enter = ui.input(|i| i.key_pressed(egui::Key::Enter)) && (r.lost_focus() || r.has_focus());
                if (enter || ui.button("Search").clicked()) && !title.query.trim().is_empty() {
                    if let Some((lat, lon)) = parse_lat_lon(&title.query) {
                        tp.write(TeleportTo { lat, lon, agl: 0.0, heading: None });
                        close = true;
                    } else if title.searching.is_none() {
                        let q = title.query.clone();
                        let (tx, rx) = crossbeam_channel::bounded(1);
                        std::thread::spawn(move || {
                            let _ = tx.send(geocode::search(&q).map_err(|e| e.to_string()));
                        });
                        title.searching = Some(rx);
                        title.error = None;
                    }
                }
                if let Some(rx) = &title.searching {
                    match rx.try_recv() {
                        Ok(Ok(res)) => {
                            title.error = res.is_empty().then(|| "No places found.".to_string());
                            title.results = res;
                            title.searching = None;
                        }
                        Ok(Err(e)) => {
                            title.error = Some(format!("Search failed ({e})."));
                            title.searching = None;
                        }
                        Err(_) => {
                            ui.spinner();
                        }
                    }
                }
                if let Some(e) = &title.error {
                    ui.colored_label(egui::Color32::from_rgb(230, 170, 60), e);
                }
                let mut go: Option<TeleportTo> = None;
                egui::ScrollArea::vertical().max_height(screen.height() - 330.0).show(ui, |ui| {
                    for p in &title.results {
                        if ui.add(egui::Button::new(&p.name).wrap()).clicked() {
                            go = Some(TeleportTo { lat: p.lat, lon: p.lon, agl: 0.0, heading: None });
                        }
                    }
                    if !title.results.is_empty() {
                        ui.separator();
                    }
                    ui.label(egui::RichText::new("Featured").strong());
                    for &(name, lat, lon, agl) in FEATURED {
                        if ui.button(name).clicked() {
                            go = Some(TeleportTo { lat, lon, agl, heading: None });
                        }
                    }
                    if !settings.0.bookmarks.is_empty() {
                        ui.add_space(6.0);
                        ui.label(egui::RichText::new("Bookmarks").strong());
                        for b in &settings.0.bookmarks {
                            if ui.button(&b.name).clicked() {
                                go = Some(TeleportTo { lat: b.lat, lon: b.lon, agl: b.agl, heading: Some(b.heading) });
                            }
                        }
                    }
                });
                ui.add_space(6.0);
                if ui.button("Pick a spot on the map…").clicked() {
                    let g = player.geo();
                    map.open_at(g.lat, g.lon);
                    close = true;
                }
                if ui.button("Back").clicked() {
                    title.page = Page::Main;
                }
                if let Some(t) = go {
                    tp.write(t);
                    close = true;
                }
            }
            Page::Customize => {
                ui.label(egui::RichText::new("Your character").size(18.0));
                ui.add_space(6.0);
                let (mut ch, mut outfit) = title.draft.clone().unwrap_or((settings.0.character.clone(), settings.0.outfit));
                egui::Grid::new("look").num_columns(2).spacing([12.0, 8.0]).show(ui, |ui| {
                    ui.label("Outfit");
                    egui::ComboBox::from_id_salt("outfit").selected_text(outfit.label()).show_ui(ui, |ui| {
                        for o in Outfit::ALL {
                            ui.selectable_value(&mut outfit, o, o.label());
                        }
                    });
                    ui.end_row();
                    ui.label("Hair");
                    egui::ComboBox::from_id_salt("hair").selected_text(ch.hair_style.label()).show_ui(ui, |ui| {
                        for h in HairStyle::ALL {
                            ui.selectable_value(&mut ch.hair_style, h, h.label());
                        }
                    });
                    ui.end_row();
                    color_row(ui, "Skin", &mut ch.skin);
                    color_row(ui, "Hair colour", &mut ch.hair);
                    color_row(ui, "Shirt", &mut ch.shirt);
                    color_row(ui, "Trousers", &mut ch.pants);
                    color_row(ui, "Shoes", &mut ch.shoes);
                    ui.label("Height");
                    ui.add(egui::Slider::new(&mut ch.height, 0.9..=1.1).show_value(false));
                    ui.end_row();
                    ui.label("Build");
                    ui.add(egui::Slider::new(&mut ch.build, 0.85..=1.2).show_value(false));
                    ui.end_row();
                    ui.label("Glasses");
                    ui.checkbox(&mut ch.glasses, "");
                    ui.end_row();
                });
                ui.small("The clothes colours are your casual outfit; the others have their own.");
                // Preview live.
                if settings.0.character != ch || settings.0.outfit != outfit {
                    settings.0.character = ch.clone();
                    settings.0.outfit = outfit;
                }
                title.draft = Some((ch, outfit));
                ui.add_space(10.0);
                ui.horizontal(|ui| {
                    if ui.button("Done").clicked() {
                        settings.0.save();
                        title.page = Page::Main;
                    }
                    if ui.button("Reset").clicked() {
                        title.draft = Some((Character::default(), Outfit::Casual));
                    }
                });
            }
        }
    });
    if close {
        title.open = false;
        title.page = Page::Main;
    }
    Ok(())
}
