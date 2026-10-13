use personal_device_link::input::{
    keyboard::{Key, KeyEvent},
    pointer::{Button, MacPointer, PointerEvent, MAX_DELTA},
    queue::{InputEvent, InputQueue},
    session::{InputSession, InputSink},
};
use std::{
    io,
    sync::{Arc, Mutex},
    time::Duration,
};

fn moved(dx: i32, dy: i32) -> InputEvent {
    InputEvent::Pointer(PointerEvent::Move { dx, dy })
}
fn button(button: Button, pressed: bool) -> PointerEvent {
    PointerEvent::Button { button, pressed }
}
fn key() -> InputEvent {
    InputEvent::Key(KeyEvent {
        key: Key::LeftShift,
        pressed: true,
        repeat: false,
    })
}

#[test]
fn motion_uses_signed_relative_deltas_and_rejects_overflow() {
    assert_eq!(
        MacPointer::movement(-30, 10).unwrap(),
        Some(PointerEvent::Move { dx: -30, dy: 10 })
    );
    assert_eq!(MacPointer::movement(0, 0).unwrap(), None);
    assert!(MacPointer::movement(i64::MAX, 0).is_err());
    assert!(PointerEvent::Move {
        dx: i32::MIN,
        dy: 0
    }
    .validate()
    .is_err());
    assert!(PointerEvent::Scroll {
        horizontal: 0,
        vertical: MAX_DELTA + 1
    }
    .validate()
    .is_err());
}

#[test]
fn scrolling_preserves_fractional_units_and_os_direction() {
    let mut pointer = MacPointer::default();
    assert_eq!(pointer.scroll(0.125, 0.125, true).unwrap(), None);
    assert_eq!(pointer.scroll(0.125, 0.125, true).unwrap(), None);
    assert_eq!(
        pointer.scroll(0.125, 0.125, true).unwrap(),
        Some(PointerEvent::Scroll {
            horizontal: -1,
            vertical: 1
        })
    );
    let mut pointer = MacPointer::default();
    assert_eq!(
        pointer.scroll(-1.0, -2.0, false).unwrap(),
        Some(PointerEvent::Scroll {
            horizontal: 120,
            vertical: -240
        })
    );
    assert_eq!(
        pointer.scroll(40.0, 40.0, true).unwrap(),
        Some(PointerEvent::Scroll {
            horizontal: -120,
            vertical: 120
        })
    );
    assert!(pointer.scroll(f64::NAN, 0.0, true).is_err());
    assert!(pointer.scroll(0.0, f64::INFINITY, true).is_err());
    assert!(pointer.scroll(0.0, 100_000.0, true).is_err());
}

#[test]
fn queue_coalesces_bursts_but_preserves_button_key_and_turn_boundaries() {
    let queue = InputQueue::new(8);
    for _ in 0..1000 {
        queue.push(moved(1, 2)).unwrap();
    }
    let down = InputEvent::Pointer(button(Button::Left, true));
    let up = InputEvent::Pointer(button(Button::Left, false));
    for event in [down, moved(5, 0), moved(-5, 0), up, key(), moved(2, 0)] {
        queue.push(event).unwrap();
    }
    for expected in [
        moved(1000, 2000),
        down,
        moved(5, 0),
        moved(-5, 0),
        up,
        key(),
        moved(2, 0),
    ] {
        assert_eq!(queue.receive(Duration::ZERO).unwrap(), Some(expected));
    }
    assert_eq!(queue.receive(Duration::ZERO).unwrap(), None);
}

#[test]
fn queue_never_drops_button_releases_or_exceeds_delta_limit() {
    let queue = InputQueue::new(2);
    queue.push(moved(MAX_DELTA, 0)).unwrap();
    queue.push(moved(1, 0)).unwrap(); // Must not merge past the wire limit.
    assert!(queue
        .push(InputEvent::Pointer(button(Button::Left, false)))
        .is_err());
    assert_eq!(
        queue.receive(Duration::ZERO).unwrap(),
        Some(moved(MAX_DELTA, 0))
    );
    assert_eq!(queue.receive(Duration::ZERO).unwrap(), Some(moved(1, 0)));
}

#[test]
fn queue_preserves_two_click_cycles_and_scroll_axis_totals() {
    let queue = InputQueue::new(8);
    let down = InputEvent::Pointer(button(Button::Left, true));
    let up = InputEvent::Pointer(button(Button::Left, false));
    for event in [down, up, down, up] {
        queue.push(event).unwrap();
    }
    for _ in 0..8 {
        queue
            .push(InputEvent::Pointer(PointerEvent::Scroll {
                horizontal: -2,
                vertical: 3,
            }))
            .unwrap();
    }
    for expected in [
        down,
        up,
        down,
        up,
        InputEvent::Pointer(PointerEvent::Scroll {
            horizontal: -16,
            vertical: 24,
        }),
    ] {
        assert_eq!(queue.receive(Duration::ZERO).unwrap(), Some(expected));
    }
}

type Trace = Arc<Mutex<Vec<InputEvent>>>;
struct Sink {
    trace: Trace,
    fail_release: bool,
}
impl InputSink for Sink {
    fn send(&mut self, key: Key, pressed: bool) -> io::Result<()> {
        self.trace.lock().unwrap().push(InputEvent::Key(KeyEvent {
            key,
            pressed,
            repeat: false,
        }));
        Ok(())
    }
    fn pointer(&mut self, event: PointerEvent) -> io::Result<()> {
        if matches!(event, PointerEvent::Button { pressed: false, .. }) && self.fail_release {
            self.fail_release = false;
            return Err(io::Error::other("simulated release failure"));
        }
        self.trace.lock().unwrap().push(InputEvent::Pointer(event));
        Ok(())
    }
}

#[test]
fn session_releases_buttons_before_modifiers_and_ignores_unowned_releases() {
    let trace = Trace::default();
    let mut session = InputSession::new(Box::new(Sink {
        trace: trace.clone(),
        fail_release: false,
    }));
    if let InputEvent::Key(key) = key() {
        session.apply(key).unwrap();
    }
    session.apply_pointer(button(Button::Left, false)).unwrap();
    session.apply_pointer(button(Button::Left, true)).unwrap();
    session.apply_pointer(button(Button::Left, true)).unwrap();
    session.apply_pointer(button(Button::Right, true)).unwrap();
    drop(session);
    assert_eq!(
        *trace.lock().unwrap(),
        vec![
            key(),
            InputEvent::Pointer(button(Button::Left, true)),
            InputEvent::Pointer(button(Button::Right, true)),
            InputEvent::Pointer(button(Button::Left, false)),
            InputEvent::Pointer(button(Button::Right, false)),
            InputEvent::Key(KeyEvent {
                key: Key::LeftShift,
                pressed: false,
                repeat: false
            })
        ]
    );
}

#[test]
fn button_release_failure_retries_on_drop_without_skipping_keys_or_other_buttons() {
    let trace = Trace::default();
    let mut session = InputSession::new(Box::new(Sink {
        trace: trace.clone(),
        fail_release: true,
    }));
    if let InputEvent::Key(key) = key() {
        session.apply(key).unwrap();
    }
    session.apply_pointer(button(Button::Left, true)).unwrap();
    session.apply_pointer(button(Button::Middle, true)).unwrap();
    assert!(session.release_all().is_err());
    drop(session);
    let trace = trace.lock().unwrap();
    for b in [Button::Left, Button::Middle] {
        assert_eq!(
            trace
                .iter()
                .filter(|e| **e == InputEvent::Pointer(button(b, false)))
                .count(),
            1
        );
    }
    assert!(trace.contains(&InputEvent::Key(KeyEvent {
        key: Key::LeftShift,
        pressed: false,
        repeat: false
    })));
}

#[test]
fn pointer_protocol_roundtrip_and_invalid_button_rejection() {
    use personal_device_link::network::protocol::*;
    for event in [
        PointerEvent::Move { dx: -20, dy: 10 },
        button(Button::Right, true),
        PointerEvent::Scroll {
            horizontal: -120,
            vertical: 90,
        },
    ] {
        let message = Message::Pointer { event };
        let mut bytes = Vec::new();
        write_message(&mut bytes, &message).unwrap();
        assert_eq!(read_message(&mut bytes.as_slice()).unwrap(), Some(message));
    }
    assert!(serde_json::from_str::<Message>(r#"{"type":"pointer","payload":{"event":{"kind":"button","button":"unknown","pressed":true}}}"#).is_err());
}
