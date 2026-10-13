use crate::input::pointer::{Button, PointerEvent};
use crate::input::{keyboard::Key, session::InputSink};
use std::{io, mem::size_of};
use windows_sys::Win32::UI::Input::KeyboardAndMouse::{
    SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, INPUT_MOUSE, KEYBDINPUT, KEYEVENTF_EXTENDEDKEY,
    KEYEVENTF_KEYUP, KEYEVENTF_SCANCODE, MOUSEEVENTF_HWHEEL, MOUSEEVENTF_LEFTDOWN,
    MOUSEEVENTF_LEFTUP, MOUSEEVENTF_MIDDLEDOWN, MOUSEEVENTF_MIDDLEUP, MOUSEEVENTF_MOVE,
    MOUSEEVENTF_RIGHTDOWN, MOUSEEVENTF_RIGHTUP, MOUSEEVENTF_WHEEL, MOUSEINPUT,
};

pub struct WindowsInput;

fn keyboard_input(key: Key, pressed: bool) -> INPUT {
    let (scan, extended) = key.scan_code();
    let flags = KEYEVENTF_SCANCODE
        | if extended { KEYEVENTF_EXTENDEDKEY } else { 0 }
        | if pressed { 0 } else { KEYEVENTF_KEYUP };
    INPUT {
        r#type: INPUT_KEYBOARD,
        Anonymous: INPUT_0 {
            ki: KEYBDINPUT {
                wVk: 0,
                wScan: scan,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: crate::input::options::INJECTED_TAG as usize,
            },
        },
    }
}

fn mouse_input(dx: i32, dy: i32, data: i32, flags: u32) -> INPUT {
    INPUT {
        r#type: INPUT_MOUSE,
        Anonymous: INPUT_0 {
            mi: MOUSEINPUT {
                dx,
                dy,
                mouseData: data as u32,
                dwFlags: flags,
                time: 0,
                dwExtraInfo: crate::input::options::INJECTED_TAG as usize,
            },
        },
    }
}

fn pointer_inputs(event: PointerEvent) -> Vec<INPUT> {
    match event {
        PointerEvent::Move { dx, dy } => vec![mouse_input(dx, dy, 0, MOUSEEVENTF_MOVE)],
        PointerEvent::Button { button, pressed } => {
            let flags = match (button, pressed) {
                (Button::Left, true) => MOUSEEVENTF_LEFTDOWN,
                (Button::Left, false) => MOUSEEVENTF_LEFTUP,
                (Button::Right, true) => MOUSEEVENTF_RIGHTDOWN,
                (Button::Right, false) => MOUSEEVENTF_RIGHTUP,
                (Button::Middle, true) => MOUSEEVENTF_MIDDLEDOWN,
                (Button::Middle, false) => MOUSEEVENTF_MIDDLEUP,
            };
            vec![mouse_input(0, 0, 0, flags)]
        }
        PointerEvent::Scroll {
            horizontal,
            vertical,
        } => {
            let mut inputs = Vec::with_capacity(2);
            // WHEEL and HWHEEL cannot share mouseData in one INPUT.
            if vertical != 0 {
                inputs.push(mouse_input(0, 0, vertical, MOUSEEVENTF_WHEEL));
            }
            if horizontal != 0 {
                inputs.push(mouse_input(0, 0, horizontal, MOUSEEVENTF_HWHEEL));
            }
            inputs
        }
    }
}

fn inject(inputs: &[INPUT]) -> io::Result<()> {
    if inputs.is_empty() {
        return Ok(());
    }
    // SAFETY: each union is initialized for its INPUT type; slice size and ABI match.
    if unsafe {
        SendInput(
            inputs.len() as u32,
            inputs.as_ptr(),
            size_of::<INPUT>() as i32,
        )
    } != inputs.len() as u32
    {
        return Err(io::Error::other(format!(
            "SendInput failed ({}); Windows may block input to an elevated or protected window",
            io::Error::last_os_error()
        )));
    }
    Ok(())
}

impl InputSink for WindowsInput {
    fn send(&mut self, key: Key, pressed: bool) -> io::Result<()> {
        inject(&[keyboard_input(key, pressed)])
    }
    fn pointer(&mut self, event: PointerEvent) -> io::Result<()> {
        event.validate()?;
        inject(&pointer_inputs(event))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn relative_motion_buttons_and_signed_wheels_without_injecting() {
        let input = pointer_inputs(PointerEvent::Move { dx: -12, dy: 9 });
        assert_eq!(input[0].r#type, INPUT_MOUSE);
        let mouse = unsafe { input[0].Anonymous.mi };
        assert_eq!(
            (mouse.dx, mouse.dy, mouse.dwFlags),
            (-12, 9, MOUSEEVENTF_MOVE)
        );
        for (button, down, up) in [
            (Button::Left, MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP),
            (Button::Right, MOUSEEVENTF_RIGHTDOWN, MOUSEEVENTF_RIGHTUP),
            (Button::Middle, MOUSEEVENTF_MIDDLEDOWN, MOUSEEVENTF_MIDDLEUP),
        ] {
            for (pressed, flags) in [(true, down), (false, up)] {
                let input = pointer_inputs(PointerEvent::Button { button, pressed });
                assert_eq!(unsafe { input[0].Anonymous.mi.dwFlags }, flags);
            }
        }
        let wheel = pointer_inputs(PointerEvent::Scroll {
            horizontal: 30,
            vertical: -120,
        });
        assert_eq!(wheel.len(), 2);
        assert_eq!(unsafe { wheel[0].Anonymous.mi.dwFlags }, MOUSEEVENTF_WHEEL);
        assert_eq!(unsafe { wheel[0].Anonymous.mi.mouseData } as i32, -120);
        assert_eq!(unsafe { wheel[1].Anonymous.mi.dwFlags }, MOUSEEVENTF_HWHEEL);
        assert_eq!(unsafe { wheel[1].Anonymous.mi.mouseData }, 30);
    }
    #[test]
    fn constructs_scan_code_and_extended_release_without_injecting() {
        let input = keyboard_input(Key::RightControl, false);
        assert_eq!(input.r#type, INPUT_KEYBOARD);
        // SAFETY: constructed with ki above.
        let key = unsafe { input.Anonymous.ki };
        assert_eq!(key.wScan, 0x1d);
        assert_eq!(key.wVk, 0);
        assert_eq!(
            key.dwFlags,
            KEYEVENTF_SCANCODE | KEYEVENTF_EXTENDEDKEY | KEYEVENTF_KEYUP
        );
    }
}
