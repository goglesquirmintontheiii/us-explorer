//! Player actions, their key bindings (rebindable, saved in settings) and gamepad input.

use bevy::input::gamepad::{Gamepad, GamepadAxis, GamepadButton};
use bevy::prelude::*;
use std::collections::HashMap;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum Action {
    Forward,
    Back,
    Left,
    Right,
    Jump,
    Descend,
    Sprint,
    CameraView,
    Map,
    GoTo,
    Noclip,
    LockMouse,
    Bookmark,
}

impl Action {
    pub const ALL: [Action; 13] = [
        Action::Forward,
        Action::Back,
        Action::Left,
        Action::Right,
        Action::Jump,
        Action::Descend,
        Action::Sprint,
        Action::CameraView,
        Action::Map,
        Action::GoTo,
        Action::Bookmark,
        Action::LockMouse,
        Action::Noclip,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Action::Forward => "Move forward",
            Action::Back => "Move back",
            Action::Left => "Move left",
            Action::Right => "Move right",
            Action::Jump => "Jump / fly up (double-tap: fly)",
            Action::Descend => "Fly down",
            Action::Sprint => "Sprint",
            Action::CameraView => "Change camera view",
            Action::Map => "Full map",
            Action::GoTo => "Go to a place",
            Action::Noclip => "No-clip (debug)",
            Action::LockMouse => "Lock / free the mouse",
            Action::Bookmark => "Bookmark this spot",
        }
    }

    pub fn key(self) -> &'static str {
        match self {
            Action::Forward => "forward",
            Action::Back => "back",
            Action::Left => "left",
            Action::Right => "right",
            Action::Jump => "jump",
            Action::Descend => "descend",
            Action::Sprint => "sprint",
            Action::CameraView => "camera",
            Action::Map => "map",
            Action::GoTo => "goto",
            Action::Noclip => "noclip",
            Action::LockMouse => "lock_mouse",
            Action::Bookmark => "bookmark",
        }
    }

    pub fn default_key(self) -> KeyCode {
        match self {
            Action::Forward => KeyCode::KeyW,
            Action::Back => KeyCode::KeyS,
            Action::Left => KeyCode::KeyA,
            Action::Right => KeyCode::KeyD,
            Action::Jump => KeyCode::Space,
            Action::Descend => KeyCode::ShiftLeft,
            Action::Sprint => KeyCode::ControlLeft,
            Action::CameraView => KeyCode::KeyV,
            Action::Map => KeyCode::KeyM,
            Action::GoTo => KeyCode::KeyT,
            Action::Noclip => KeyCode::KeyN,
            Action::LockMouse => KeyCode::Tab,
            Action::Bookmark => KeyCode::KeyB,
        }
    }
}

/// Keys that can be bound, with stable names for the settings file.
const KEYS: &[(&str, KeyCode)] = &[
    ("A", KeyCode::KeyA),
    ("B", KeyCode::KeyB),
    ("C", KeyCode::KeyC),
    ("D", KeyCode::KeyD),
    ("E", KeyCode::KeyE),
    ("F", KeyCode::KeyF),
    ("G", KeyCode::KeyG),
    ("H", KeyCode::KeyH),
    ("I", KeyCode::KeyI),
    ("J", KeyCode::KeyJ),
    ("K", KeyCode::KeyK),
    ("L", KeyCode::KeyL),
    ("M", KeyCode::KeyM),
    ("N", KeyCode::KeyN),
    ("O", KeyCode::KeyO),
    ("P", KeyCode::KeyP),
    ("Q", KeyCode::KeyQ),
    ("R", KeyCode::KeyR),
    ("S", KeyCode::KeyS),
    ("T", KeyCode::KeyT),
    ("U", KeyCode::KeyU),
    ("V", KeyCode::KeyV),
    ("W", KeyCode::KeyW),
    ("X", KeyCode::KeyX),
    ("Y", KeyCode::KeyY),
    ("Z", KeyCode::KeyZ),
    ("0", KeyCode::Digit0),
    ("1", KeyCode::Digit1),
    ("2", KeyCode::Digit2),
    ("3", KeyCode::Digit3),
    ("4", KeyCode::Digit4),
    ("5", KeyCode::Digit5),
    ("6", KeyCode::Digit6),
    ("7", KeyCode::Digit7),
    ("8", KeyCode::Digit8),
    ("9", KeyCode::Digit9),
    ("Space", KeyCode::Space),
    ("Left Shift", KeyCode::ShiftLeft),
    ("Right Shift", KeyCode::ShiftRight),
    ("Left Ctrl", KeyCode::ControlLeft),
    ("Right Ctrl", KeyCode::ControlRight),
    ("Left Alt", KeyCode::AltLeft),
    ("Right Alt", KeyCode::AltRight),
    ("Tab", KeyCode::Tab),
    ("Enter", KeyCode::Enter),
    ("Backspace", KeyCode::Backspace),
    ("Up", KeyCode::ArrowUp),
    ("Down", KeyCode::ArrowDown),
    ("Left", KeyCode::ArrowLeft),
    ("Right", KeyCode::ArrowRight),
    ("F1", KeyCode::F1),
    ("F2", KeyCode::F2),
    ("F4", KeyCode::F4),
    ("F5", KeyCode::F5),
    ("F6", KeyCode::F6),
    ("F7", KeyCode::F7),
    ("F8", KeyCode::F8),
    ("F9", KeyCode::F9),
    ("F10", KeyCode::F10),
    ("F11", KeyCode::F11),
    ("F12", KeyCode::F12),
    ("Caps Lock", KeyCode::CapsLock),
    ("Home", KeyCode::Home),
    ("End", KeyCode::End),
    ("Page Up", KeyCode::PageUp),
    ("Page Down", KeyCode::PageDown),
    ("Insert", KeyCode::Insert),
    ("Delete", KeyCode::Delete),
    ("Comma", KeyCode::Comma),
    ("Period", KeyCode::Period),
    ("Slash", KeyCode::Slash),
    ("Semicolon", KeyCode::Semicolon),
    ("Quote", KeyCode::Quote),
    ("Minus", KeyCode::Minus),
    ("Equal", KeyCode::Equal),
    ("Backquote", KeyCode::Backquote),
    ("Num 0", KeyCode::Numpad0),
    ("Num 1", KeyCode::Numpad1),
    ("Num 2", KeyCode::Numpad2),
    ("Num 3", KeyCode::Numpad3),
    ("Num 4", KeyCode::Numpad4),
    ("Num 5", KeyCode::Numpad5),
    ("Num 6", KeyCode::Numpad6),
    ("Num 7", KeyCode::Numpad7),
    ("Num 8", KeyCode::Numpad8),
    ("Num 9", KeyCode::Numpad9),
];

pub fn key_name(k: KeyCode) -> &'static str {
    KEYS.iter().find(|(_, c)| *c == k).map(|(n, _)| *n).unwrap_or("?")
}

pub fn key_from_name(n: &str) -> Option<KeyCode> {
    KEYS.iter().find(|(name, _)| *name == n).map(|(_, c)| *c)
}

pub fn bindable(k: KeyCode) -> bool {
    KEYS.iter().any(|(_, c)| *c == k)
}

/// The live bindings, built from settings (missing entries use the defaults).
#[derive(Resource, Clone)]
pub struct Bindings(pub HashMap<Action, KeyCode>);

impl Bindings {
    pub fn from_settings(map: &std::collections::BTreeMap<String, String>) -> Self {
        let mut b = HashMap::new();
        for a in Action::ALL {
            let k = map.get(a.key()).and_then(|n| key_from_name(n)).unwrap_or(a.default_key());
            b.insert(a, k);
        }
        Self(b)
    }

    pub fn to_settings(&self) -> std::collections::BTreeMap<String, String> {
        self.0.iter().map(|(a, k)| (a.key().to_string(), key_name(*k).to_string())).collect()
    }

    pub fn key(&self, a: Action) -> KeyCode {
        self.0.get(&a).copied().unwrap_or(a.default_key())
    }
}

/// One frame of player intent, merged from keyboard, mouse and gamepad.
#[derive(Resource, Default)]
pub struct Intent {
    /// x = right, y = forward, each -1..1.
    pub move_axis: Vec2,
    pub up: f32,
    pub sprint: bool,
    pub jump_pressed: bool,
    pub jump_held: bool,
    pub descend: bool,
    pub camera_view: bool,
    pub map: bool,
    pub goto: bool,
    pub noclip: bool,
    pub lock_mouse: bool,
    pub bookmark: bool,
    /// Look change in degrees (yaw right, pitch up).
    pub look: Vec2,
    pub gamepad: bool,
}

pub struct ControlsPlugin;

impl Plugin for ControlsPlugin {
    fn build(&self, app: &mut App) {
        let b = Bindings::from_settings(&app.world().resource::<crate::GameSettings>().0.key_bindings);
        app.insert_resource(b).init_resource::<Intent>().add_systems(PreUpdate, gather.after(bevy::input::InputSystems));
    }
}

fn gather(
    keys: Res<ButtonInput<KeyCode>>,
    bindings: Res<Bindings>,
    blocked: Res<crate::camera::InputBlocked>,
    pads: Query<&Gamepad>,
    time: Res<Time>,
    mut intent: ResMut<Intent>,
) {
    *intent = Intent::default();
    if !blocked.keyboard {
        let pressed = |a: Action| keys.pressed(bindings.key(a));
        let just = |a: Action| keys.just_pressed(bindings.key(a));
        let f = |b: bool| b as i32 as f32;
        intent.move_axis =
            Vec2::new(f(pressed(Action::Right)) - f(pressed(Action::Left)), f(pressed(Action::Forward)) - f(pressed(Action::Back)));
        intent.up = f(pressed(Action::Jump)) - f(pressed(Action::Descend));
        intent.sprint = pressed(Action::Sprint);
        intent.jump_pressed = just(Action::Jump);
        intent.jump_held = pressed(Action::Jump);
        intent.descend = pressed(Action::Descend);
        intent.camera_view = just(Action::CameraView) || keys.just_pressed(KeyCode::F5);
        intent.map = just(Action::Map);
        intent.goto = just(Action::GoTo);
        intent.noclip = just(Action::Noclip);
        intent.lock_mouse = just(Action::LockMouse);
        intent.bookmark = just(Action::Bookmark);
    }
    // Gamepad: left stick moves, right stick looks, A jumps (double-tap flies), B sinks,
    // left stick click sprints, Y changes camera, Select opens the map.
    let dt = time.delta_secs();
    for pad in &pads {
        let ls = Vec2::new(pad.get(GamepadAxis::LeftStickX).unwrap_or(0.0), pad.get(GamepadAxis::LeftStickY).unwrap_or(0.0));
        let rs = Vec2::new(pad.get(GamepadAxis::RightStickX).unwrap_or(0.0), pad.get(GamepadAxis::RightStickY).unwrap_or(0.0));
        let dz = |v: Vec2| if v.length() < 0.15 { Vec2::ZERO } else { v };
        let (ls, rs) = (dz(ls), dz(rs));
        if ls != Vec2::ZERO || rs != Vec2::ZERO || pad.get_pressed().next().is_some() {
            intent.gamepad = true;
        }
        intent.move_axis += ls;
        intent.look += rs * Vec2::new(150.0, 110.0) * dt;
        let a = pad.pressed(GamepadButton::South);
        let b = pad.pressed(GamepadButton::East);
        intent.up += a as i32 as f32 - b as i32 as f32;
        intent.jump_pressed |= pad.just_pressed(GamepadButton::South);
        intent.jump_held |= a;
        intent.descend |= b;
        intent.sprint |= pad.pressed(GamepadButton::LeftThumb) || pad.pressed(GamepadButton::LeftTrigger2);
        intent.camera_view |= pad.just_pressed(GamepadButton::North);
        intent.map |= pad.just_pressed(GamepadButton::Select);
        intent.bookmark |= pad.just_pressed(GamepadButton::West);
    }
    intent.move_axis = intent.move_axis.clamp_length_max(1.0);
    intent.up = intent.up.clamp(-1.0, 1.0);
}
