//! CoreGraphics receiver. The session owns modifier/button state and releases
//! it on normal stop, disconnect and protocol failure.
use crate::input::{
    keyboard::{mac_code, Key, COMMAND_FLAG, CONTROL_FLAG, OPTION_FLAG, SHIFT_FLAG},
    options::INJECTED_TAG,
    pointer::{Button, PointerEvent},
    session::InputSink,
};
use std::{
    collections::BTreeSet,
    ffi::c_void,
    io, ptr,
    time::{Duration, Instant},
};

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct Point {
    x: f64,
    y: f64,
}
#[link(name = "CoreGraphics", kind = "framework")]
extern "C" {
    fn CGEventCreate(source: *const c_void) -> *mut c_void;
    fn CGEventCreateKeyboardEvent(source: *const c_void, key: u16, down: bool) -> *mut c_void;
    fn CGEventCreateMouseEvent(
        source: *const c_void,
        kind: u32,
        point: Point,
        button: u32,
    ) -> *mut c_void;
    fn CGEventCreateScrollWheelEvent(
        source: *const c_void,
        units: u32,
        count: u32,
        wheel1: i32,
        ...
    ) -> *mut c_void;
    fn CGEventGetLocation(event: *const c_void) -> Point;
    fn CGEventSetFlags(event: *mut c_void, flags: u64);
    fn CGEventSetIntegerValueField(event: *mut c_void, field: u32, value: i64);
    fn CGEventPost(tap: u32, event: *const c_void);
    fn CGPreflightPostEventAccess() -> bool;
    fn CGRequestPostEventAccess() -> bool;
}
#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {
    fn CFRelease(value: *const c_void);
}

struct Event(*mut c_void);
impl Event {
    fn new(pointer: *mut c_void) -> io::Result<Self> {
        if pointer.is_null() {
            Err(io::Error::other("无法创建 macOS 输入事件"))
        } else {
            Ok(Self(pointer))
        }
    }
    fn post(&self, flags: u64) -> io::Result<()> {
        // Permission can be revoked during a session; stop rather than report success.
        if !unsafe { CGPreflightPostEventAccess() } {
            return Err(io::Error::other(
                "Mac 接收端辅助功能权限已失效，请重新授权并重启接收端",
            ));
        }
        unsafe {
            CGEventSetFlags(self.0, flags);
            CGEventSetIntegerValueField(self.0, 42, INJECTED_TAG);
            CGEventPost(0, self.0);
        }
        Ok(())
    }
}
impl Drop for Event {
    fn drop(&mut self) {
        unsafe {
            CFRelease(self.0);
        }
    }
}

pub struct MacInput {
    keys: BTreeSet<Key>,
    buttons: BTreeSet<Button>,
    last_click: Option<(Button, Point, Instant)>,
    clicks: i64,
    scroll_x: i32,
    scroll_y: i32,
}
impl MacInput {
    pub fn new() -> io::Result<Self> {
        if !unsafe { CGPreflightPostEventAccess() } {
            unsafe {
                CGRequestPostEventAccess();
            }
            return Err(io::Error::new(io::ErrorKind::PermissionDenied,
                "Mac 接收端尚未获得辅助功能权限，请在接收端授权本次 Personal Device Link 应用，完全退出后重新打开"));
        }
        Ok(Self {
            keys: BTreeSet::new(),
            buttons: BTreeSet::new(),
            last_click: None,
            clicks: 1,
            scroll_x: 0,
            scroll_y: 0,
        })
    }
    fn flags(&self) -> u64 {
        self.keys.iter().fold(0, |flags, key| {
            flags
                | match key {
                    Key::LeftShift | Key::RightShift => SHIFT_FLAG,
                    Key::LeftControl | Key::RightControl => CONTROL_FLAG,
                    Key::LeftAlt | Key::RightAlt => OPTION_FLAG,
                    Key::LeftMeta | Key::RightMeta => COMMAND_FLAG,
                    _ => 0,
                }
        })
    }
    fn position() -> io::Result<Point> {
        let event = Event::new(unsafe { CGEventCreate(ptr::null()) })?;
        Ok(unsafe { CGEventGetLocation(event.0) })
    }
}
impl InputSink for MacInput {
    fn send(&mut self, key: Key, pressed: bool) -> io::Result<()> {
        let code = mac_code(key).ok_or_else(|| io::Error::other("Mac 不支持该键位"))?;
        let event = Event::new(unsafe { CGEventCreateKeyboardEvent(ptr::null(), code, pressed) })?;
        let previous = self.keys.contains(&key);
        if pressed {
            self.keys.insert(key);
        } else {
            self.keys.remove(&key);
        }
        unsafe {
            CGEventSetIntegerValueField(
                event.0,
                8,
                i64::from(pressed && previous && !key.is_modifier()),
            );
        }
        if let Err(e) = event.post(self.flags()) {
            if previous {
                self.keys.insert(key);
            } else {
                self.keys.remove(&key);
            }
            return Err(e);
        }
        Ok(())
    }
    fn pointer(&mut self, input: PointerEvent) -> io::Result<()> {
        input.validate()?;
        let point = Self::position()?;
        let event = match input {
            PointerEvent::Move { dx, dy } => {
                let (kind, button) = if self.buttons.contains(&Button::Left) {
                    (6, 0)
                } else if self.buttons.contains(&Button::Right) {
                    (7, 1)
                } else if self.buttons.contains(&Button::Middle) {
                    (27, 2)
                } else {
                    (5, 0)
                };
                Event::new(unsafe {
                    CGEventCreateMouseEvent(
                        ptr::null(),
                        kind,
                        Point {
                            x: point.x + dx as f64,
                            y: point.y + dy as f64,
                        },
                        button,
                    )
                })?
            }
            PointerEvent::Button { button, pressed } => {
                let (down, up, number) = match button {
                    Button::Left => (1, 2, 0),
                    Button::Right => (3, 4, 1),
                    Button::Middle => (25, 26, 2),
                };
                if pressed {
                    let double = self.last_click.is_some_and(|(b, p, t)| {
                        b == button
                            && t.elapsed() < Duration::from_millis(500)
                            && (p.x - point.x).abs() < 4.0
                            && (p.y - point.y).abs() < 4.0
                    });
                    self.clicks = if double && self.clicks == 1 { 2 } else { 1 };
                    self.last_click = Some((button, point, Instant::now()));
                }
                let event = Event::new(unsafe {
                    CGEventCreateMouseEvent(
                        ptr::null(),
                        if pressed { down } else { up },
                        point,
                        number,
                    )
                })?;
                unsafe {
                    CGEventSetIntegerValueField(event.0, 1, self.clicks);
                }
                event.post(self.flags())?;
                if pressed {
                    self.buttons.insert(button);
                } else {
                    self.buttons.remove(&button);
                }
                return Ok(());
            }
            PointerEvent::Scroll {
                horizontal,
                vertical,
            } => {
                // Wire units = 120/detent, CG pixel units = 40/detent. Keep remainders.
                self.scroll_x -= horizontal;
                self.scroll_y += vertical;
                let (x, y) = (self.scroll_x / 3, self.scroll_y / 3);
                self.scroll_x %= 3;
                self.scroll_y %= 3;
                if x == 0 && y == 0 {
                    return Ok(());
                }
                Event::new(unsafe { CGEventCreateScrollWheelEvent(ptr::null(), 0, 2, y, x) })?
            }
        };
        event.post(self.flags())
    }
}
