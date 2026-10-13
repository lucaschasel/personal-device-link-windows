use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Key {
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
    Digit0,
    Digit1,
    Digit2,
    Digit3,
    Digit4,
    Digit5,
    Digit6,
    Digit7,
    Digit8,
    Digit9,
    Space,
    Enter,
    Backspace,
    Tab,
    Escape,
    Minus,
    Equal,
    LeftBracket,
    RightBracket,
    Backslash,
    Semicolon,
    Quote,
    Grave,
    Comma,
    Period,
    Slash,
    LeftShift,
    RightShift,
    LeftControl,
    RightControl,
    LeftAlt,
    RightAlt,
    LeftMeta,
    RightMeta,
    ArrowLeft,
    ArrowRight,
    ArrowUp,
    ArrowDown,
    Delete,
    Home,
    End,
    PageUp,
    PageDown,
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
}

impl Key {
    pub fn is_modifier(self) -> bool {
        matches!(
            self,
            Self::LeftShift
                | Self::RightShift
                | Self::LeftControl
                | Self::RightControl
                | Self::LeftAlt
                | Self::RightAlt
                | Self::LeftMeta
                | Self::RightMeta
        )
    }

    /// IBM PC set-1 scan code and whether the E0 extended prefix is required.
    pub fn scan_code(self) -> (u16, bool) {
        use Key::*;
        let code = match self {
            Escape => 0x01,
            Digit1 => 0x02,
            Digit2 => 0x03,
            Digit3 => 0x04,
            Digit4 => 0x05,
            Digit5 => 0x06,
            Digit6 => 0x07,
            Digit7 => 0x08,
            Digit8 => 0x09,
            Digit9 => 0x0a,
            Digit0 => 0x0b,
            Minus => 0x0c,
            Equal => 0x0d,
            Backspace => 0x0e,
            Tab => 0x0f,
            Q => 0x10,
            W => 0x11,
            E => 0x12,
            R => 0x13,
            T => 0x14,
            Y => 0x15,
            U => 0x16,
            I => 0x17,
            O => 0x18,
            P => 0x19,
            LeftBracket => 0x1a,
            RightBracket => 0x1b,
            Enter => 0x1c,
            LeftControl | RightControl => 0x1d,
            A => 0x1e,
            S => 0x1f,
            D => 0x20,
            F => 0x21,
            G => 0x22,
            H => 0x23,
            J => 0x24,
            K => 0x25,
            L => 0x26,
            Semicolon => 0x27,
            Quote => 0x28,
            Grave => 0x29,
            LeftShift => 0x2a,
            Backslash => 0x2b,
            Z => 0x2c,
            X => 0x2d,
            C => 0x2e,
            V => 0x2f,
            B => 0x30,
            N => 0x31,
            M => 0x32,
            Comma => 0x33,
            Period => 0x34,
            Slash => 0x35,
            RightShift => 0x36,
            LeftAlt | RightAlt => 0x38,
            Space => 0x39,
            F1 => 0x3b,
            F2 => 0x3c,
            F3 => 0x3d,
            F4 => 0x3e,
            F5 => 0x3f,
            F6 => 0x40,
            F7 => 0x41,
            F8 => 0x42,
            F9 => 0x43,
            F10 => 0x44,
            F11 => 0x57,
            F12 => 0x58,
            Home => 0x47,
            ArrowUp => 0x48,
            PageUp => 0x49,
            ArrowLeft => 0x4b,
            ArrowRight => 0x4d,
            End => 0x4f,
            ArrowDown => 0x50,
            PageDown => 0x51,
            Delete => 0x53,
            LeftMeta => 0x5b,
            RightMeta => 0x5c,
        };
        let extended = matches!(
            self,
            RightControl
                | RightAlt
                | LeftMeta
                | RightMeta
                | ArrowLeft
                | ArrowRight
                | ArrowUp
                | ArrowDown
                | Delete
                | Home
                | End
                | PageUp
                | PageDown
        );
        (code, extended)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeyEvent {
    pub key: Key,
    pub pressed: bool,
    pub repeat: bool,
}

pub const SHIFT_FLAG: u64 = 1 << 17;
pub const CONTROL_FLAG: u64 = 1 << 18;
pub const OPTION_FLAG: u64 = 1 << 19;
pub const COMMAND_FLAG: u64 = 1 << 20;
pub const MODIFIER_FLAGS: u64 = SHIFT_FLAG | CONTROL_FLAG | OPTION_FLAG | COMMAND_FLAG;

/// Keep local mouse ownership until its last button-up has been delivered.
pub fn local_pointer_mode(
    allowed: bool,
    was_local: bool,
    flags: u64,
    button_held: bool,
    button_up: bool,
) -> bool {
    allowed
        && (flags & (CONTROL_FLAG | OPTION_FLAG) == (CONTROL_FLAG | OPTION_FLAG)
            || (was_local && (button_held || button_up)))
}

pub fn emergency_exit(code: u16, flags: u64, pressed: bool) -> bool {
    pressed && code == 53 && flags & (CONTROL_FLAG | OPTION_FLAG) == (CONTROL_FLAG | OPTION_FLAG)
}

#[derive(Debug, Clone, Copy, Default)]
pub struct ModifierMapping {
    /// Reserved configuration point; default Command maps to Control.
    pub command_as_meta: bool,
}

pub fn mac_key(code: u16, mapping: ModifierMapping) -> Option<Key> {
    use Key::*;
    Some(match code {
        0 => A,
        1 => S,
        2 => D,
        3 => F,
        4 => H,
        5 => G,
        6 => Z,
        7 => X,
        8 => C,
        9 => V,
        11 => B,
        12 => Q,
        13 => W,
        14 => E,
        15 => R,
        16 => Y,
        17 => T,
        18 => Digit1,
        19 => Digit2,
        20 => Digit3,
        21 => Digit4,
        22 => Digit6,
        23 => Digit5,
        24 => Equal,
        25 => Digit9,
        26 => Digit7,
        27 => Minus,
        28 => Digit8,
        29 => Digit0,
        30 => RightBracket,
        31 => O,
        32 => U,
        33 => LeftBracket,
        34 => I,
        35 => P,
        36 => Enter,
        37 => L,
        38 => J,
        39 => Quote,
        40 => K,
        41 => Semicolon,
        42 => Backslash,
        43 => Comma,
        44 => Slash,
        45 => N,
        46 => M,
        47 => Period,
        48 => Tab,
        49 => Space,
        50 => Grave,
        51 => Backspace,
        53 => Escape,
        54 => {
            if mapping.command_as_meta {
                RightMeta
            } else {
                RightControl
            }
        }
        55 => {
            if mapping.command_as_meta {
                LeftMeta
            } else {
                LeftControl
            }
        }
        56 => LeftShift,
        58 => LeftAlt,
        59 => LeftControl,
        60 => RightShift,
        61 => RightAlt,
        62 => RightControl,
        96 => F5,
        97 => F6,
        98 => F7,
        99 => F3,
        100 => F8,
        101 => F9,
        103 => F11,
        109 => F10,
        111 => F12,
        115 => Home,
        116 => PageUp,
        117 => Delete,
        118 => F4,
        119 => End,
        120 => F2,
        121 => PageDown,
        122 => F1,
        123 => ArrowLeft,
        124 => ArrowRight,
        125 => ArrowDown,
        126 => ArrowUp,
        _ => return None,
    })
}

/// Tracks source keys separately: Command and Control can map to the same target.
#[derive(Default)]
pub struct MacKeyboard {
    mapping: ModifierMapping,
    sources: BTreeMap<u16, Key>,
}
impl MacKeyboard {
    pub fn for_target(target: &str, shortcut_mapping: bool) -> Self {
        Self {
            mapping: ModifierMapping {
                command_as_meta: target != "windows" || !shortcut_mapping,
            },
            sources: BTreeMap::new(),
        }
    }
    pub fn event(&mut self, code: u16, pressed: bool, repeat: bool) -> Option<KeyEvent> {
        let key = mac_key(code, self.mapping)?;
        if pressed {
            if self.sources.contains_key(&code) {
                return (repeat && !key.is_modifier()).then_some(KeyEvent {
                    key,
                    pressed: true,
                    repeat: true,
                });
            }
            if repeat {
                return None;
            } // Ignore keys already held before capture started.
            let already = self.sources.values().any(|value| *value == key);
            self.sources.insert(code, key);
            (!already).then_some(KeyEvent {
                key,
                pressed: true,
                repeat: false,
            })
        } else {
            let key = self.sources.remove(&code)?;
            (!self.sources.values().any(|value| *value == key)).then_some(KeyEvent {
                key,
                pressed: false,
                repeat: false,
            })
        }
    }
    pub fn flags_changed(&mut self, code: u16, flags: u64) -> Option<KeyEvent> {
        let mask = match code {
            54 | 55 => COMMAND_FLAG,
            56 | 60 => SHIFT_FLAG,
            58 | 61 => OPTION_FLAG,
            59 | 62 => CONTROL_FLAG,
            _ => return None,
        };
        // flagsChanged identifies the changed side; the aggregate flag stays set
        // when the other side of the modifier is still held.
        let pressed = flags & mask != 0 && !self.sources.contains_key(&code);
        self.event(code, pressed, false)
    }
}

/// Canonical key -> macOS virtual key code, preserving Command as Meta.
pub fn mac_code(key: Key) -> Option<u16> {
    (0..128).find(|code| {
        mac_key(
            *code,
            ModifierMapping {
                command_as_meta: true,
            },
        ) == Some(key)
    })
}

pub fn windows_key(scan: u16, extended: bool) -> Option<Key> {
    (0..128)
        .filter_map(|code| {
            mac_key(
                code,
                ModifierMapping {
                    command_as_meta: true,
                },
            )
        })
        .find(|key| key.scan_code() == (scan, extended))
}

#[derive(Default)]
pub struct WindowsKeyboard {
    sources: BTreeMap<(u16, bool), Key>,
}
impl WindowsKeyboard {
    pub fn event(
        &mut self,
        scan: u16,
        extended: bool,
        pressed: bool,
        target: &str,
        shortcuts: bool,
    ) -> Option<KeyEvent> {
        let source = (scan, extended);
        if !pressed {
            return self.sources.remove(&source).map(|key| KeyEvent {
                key,
                pressed: false,
                repeat: false,
            });
        }
        if let Some(key) = self.sources.get(&source) {
            return (!key.is_modifier()).then_some(KeyEvent {
                key: *key,
                pressed: true,
                repeat: true,
            });
        }
        let mut key = windows_key(scan, extended)?;
        if target == "macos" && shortcuts {
            key = match key {
                Key::LeftControl => Key::LeftMeta,
                Key::RightControl => Key::RightMeta,
                Key::LeftMeta => Key::LeftControl,
                Key::RightMeta => Key::RightControl,
                other => other,
            };
        }
        self.sources.insert(source, key);
        Some(KeyEvent {
            key,
            pressed: true,
            repeat: false,
        })
    }
}
