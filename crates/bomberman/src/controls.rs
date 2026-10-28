use crate::settings::{Device, KeyLayout, Settings};
use bomberman_core::{Direction, PlayerInput};
use gilrs::{Axis, Button, EventType, GamepadId, Gilrs};
use macroquad::prelude::*;

pub fn fullscreen_shortcut() -> bool {
    is_key_pressed(KeyCode::F11)
        || (cfg!(target_os = "macos")
            && is_key_pressed(KeyCode::F)
            && (is_key_down(KeyCode::LeftSuper) || is_key_down(KeyCode::RightSuper))
            && (is_key_down(KeyCode::LeftControl) || is_key_down(KeyCode::RightControl)))
}

macro_rules! key_names {
    ($($key:ident),* $(,)?) => {
        pub fn key_named(name: &str) -> Option<KeyCode> {
            match name { $(stringify!($key) => Some(KeyCode::$key),)* _ => None }
        }
    }
}

key_names!(
    Space,
    Apostrophe,
    Comma,
    Minus,
    Period,
    Slash,
    Key0,
    Key1,
    Key2,
    Key3,
    Key4,
    Key5,
    Key6,
    Key7,
    Key8,
    Key9,
    Semicolon,
    Equal,
    A,
    B,
    C,
    D,
    E,
    F,
    G,
    H,
    I,
    J,
    K,
    L,
    M,
    N,
    O,
    P,
    Q,
    R,
    S,
    T,
    U,
    V,
    W,
    X,
    Y,
    Z,
    LeftBracket,
    Backslash,
    RightBracket,
    GraveAccent,
    Escape,
    Enter,
    Tab,
    Backspace,
    Insert,
    Delete,
    Right,
    Left,
    Down,
    Up,
    PageUp,
    PageDown,
    Home,
    End,
    CapsLock,
    ScrollLock,
    NumLock,
    PrintScreen,
    Pause,
    F1,
    F2,
    F3,
    F4,
    F5,
    F6,
    F7,
    F8,
    F9,
    F10,
    F11,
    F12,
    Kp0,
    Kp1,
    Kp2,
    Kp3,
    Kp4,
    Kp5,
    Kp6,
    Kp7,
    Kp8,
    Kp9,
    KpDecimal,
    KpDivide,
    KpMultiply,
    KpSubtract,
    KpAdd,
    KpEnter,
    KpEqual,
    LeftShift,
    LeftControl,
    LeftAlt,
    LeftSuper,
    RightShift,
    RightControl,
    RightAlt,
    RightSuper,
    Menu
);

pub const ACTIONS: [&str; 6] = [
    "Move up",
    "Move right",
    "Move down",
    "Move left",
    "Place bomb",
    "Action / trigger",
];
pub const PAD_BUTTONS: [&str; 6] = [
    "South",
    "East",
    "West",
    "North",
    "LeftTrigger",
    "RightTrigger",
];

fn button_named(name: &str) -> Button {
    match name {
        "East" => Button::East,
        "West" => Button::West,
        "North" => Button::North,
        "LeftTrigger" => Button::LeftTrigger,
        "RightTrigger" => Button::RightTrigger,
        _ => Button::South,
    }
}

fn keyboard(layout: &KeyLayout) -> PlayerInput {
    let held = |i: usize| key_named(&layout.keys[i]).is_some_and(is_key_down);
    let direction = [
        (0, Direction::Up),
        (1, Direction::Right),
        (2, Direction::Down),
        (3, Direction::Left),
    ]
    .into_iter()
    .find_map(|(i, direction)| held(i).then_some(direction));
    PlayerInput {
        direction,
        direction_mask: (0..4).fold(0, |mask, index| mask | (u8::from(held(index)) << index)),
        bomb: held(4),
        action: held(5),
    }
}

pub struct Controllers {
    gilrs: Option<Gilrs>,
    /// Stable device slots for the current process, preserved across disconnection.
    slots: Vec<GamepadId>,
    pub status: String,
    pub confirm: bool,
    pub back: bool,
    pub pause: bool,
    pub up: bool,
    pub down: bool,
    pub left: bool,
    pub right: bool,
}

impl Controllers {
    pub fn new() -> Self {
        let (gilrs, status) = match Gilrs::new() {
            Ok(gilrs) => (Some(gilrs), String::new()),
            Err(error) => (None, format!("Controller service unavailable: {error}")),
        };
        let mut this = Self {
            gilrs,
            slots: Vec::new(),
            status,
            confirm: false,
            back: false,
            pause: false,
            up: false,
            down: false,
            left: false,
            right: false,
        };
        if let Some(gilrs) = &this.gilrs {
            this.slots.extend(gilrs.gamepads().map(|(id, _)| id));
        }
        this
    }

    pub fn poll(&mut self) {
        self.confirm = false;
        self.back = false;
        self.pause = false;
        self.up = false;
        self.down = false;
        self.left = false;
        self.right = false;
        if let Some(gilrs) = &mut self.gilrs {
            while let Some(event) = gilrs.next_event() {
                match event.event {
                    EventType::Connected => {
                        if !self.slots.contains(&event.id) {
                            self.slots.push(event.id);
                        }
                    }
                    EventType::ButtonPressed(button, _) => match button {
                        Button::South => self.confirm = true,
                        Button::East => self.back = true,
                        Button::Start => self.pause = true,
                        Button::DPadUp => self.up = true,
                        Button::DPadDown => self.down = true,
                        Button::DPadLeft => self.left = true,
                        Button::DPadRight => self.right = true,
                        _ => {}
                    },
                    _ => {}
                }
            }
        }
    }

    pub fn devices(&self) -> Vec<String> {
        self.slots
            .iter()
            .enumerate()
            .map(|(i, &id)| {
                let pad = self.gilrs.as_ref().unwrap().gamepad(id);
                format!(
                    "{}: {}{}",
                    i + 1,
                    pad.name(),
                    if pad.is_connected() {
                        ""
                    } else {
                        " (disconnected)"
                    }
                )
            })
            .collect()
    }

    pub fn inputs(&self, settings: &Settings) -> Vec<PlayerInput> {
        settings
            .participants
            .iter()
            .take(settings.participant_count)
            .map(|participant| match participant.device {
                Device::Keyboard1 => keyboard(&settings.keyboard[0]),
                Device::Keyboard2 => keyboard(&settings.keyboard[1]),
                Device::Controller(slot) => self.pad_input(slot, settings),
                Device::Bot => PlayerInput::default(),
            })
            .collect()
    }

    fn pad_input(&self, slot: u8, settings: &Settings) -> PlayerInput {
        let Some((&id, gilrs)) = self.slots.get(slot as usize).zip(self.gilrs.as_ref()) else {
            return PlayerInput::default();
        };
        let pad = gilrs.gamepad(id);
        if !pad.is_connected() {
            return PlayerInput::default();
        }
        let x = pad.value(Axis::LeftStickX);
        let y = pad.value(Axis::LeftStickY);
        // Resolve diagonals in the simulation, where neighboring passability
        // is known. Original joystick thresholds 30/70 map to +/-0.4.
        let directions = [
            (pad.is_pressed(Button::DPadUp) || y > 0.4, Direction::Up),
            (
                pad.is_pressed(Button::DPadRight) || x > 0.4,
                Direction::Right,
            ),
            (
                pad.is_pressed(Button::DPadDown) || y < -0.4,
                Direction::Down,
            ),
            (
                pad.is_pressed(Button::DPadLeft) || x < -0.4,
                Direction::Left,
            ),
        ];
        let direction_mask = directions
            .iter()
            .enumerate()
            .fold(0, |mask, (index, (held, _))| {
                mask | (u8::from(*held) << index)
            });
        let direction = directions
            .into_iter()
            .rev()
            .find_map(|(held, direction)| held.then_some(direction));
        PlayerInput {
            direction,
            direction_mask,
            bomb: pad.is_pressed(button_named(&settings.pad_bomb)),
            action: pad.is_pressed(button_named(&settings.pad_action)),
        }
    }
}
