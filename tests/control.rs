use personal_device_link::input::{
    pointer::{Button, PointerEvent},
    queue::InputEvent,
};
use personal_device_link::{
    input::{
        keyboard::{Key, KeyEvent},
        session::InputSink,
    },
    network::{client::Client, control::ControlAccess, protocol::*, server},
};
use std::{
    io::{self, Write},
    net::{SocketAddr, TcpListener, TcpStream},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    thread,
    time::{Duration, Instant},
};

const TOKEN: &str = "test-only-0123456789abcdef0123456789abcdef";

#[test]
fn keyboard_only_receiver_rejects_pointer_and_releases_keyboard() {
    let h = Harness::start(true);
    let mut client = h.client();
    client
        .start_control_with_options(
            TOKEN.into(),
            personal_device_link::input::options::ControlOptions {
                keyboard: true,
                pointer: false,
                shortcut_mapping: true,
            },
        )
        .unwrap();
    client
        .send_key(KeyEvent {
            key: Key::LeftShift,
            pressed: true,
            repeat: false,
        })
        .unwrap();
    client
        .send_pointer(PointerEvent::Button {
            button: Button::Left,
            pressed: true,
        })
        .unwrap();
    assert!(client.heartbeat().is_err());
    h.wait_for_release(Key::LeftShift, Duration::from_secs(1));
    assert!(h
        .trace
        .lock()
        .unwrap()
        .iter()
        .all(|event| matches!(event, InputEvent::Key(_))));
}

#[test]
fn pointer_only_receiver_rejects_keyboard_and_releases_pointer() {
    let h = Harness::start(true);
    let mut client = h.client();
    client
        .start_control_with_options(
            TOKEN.into(),
            personal_device_link::input::options::ControlOptions {
                keyboard: false,
                pointer: true,
                shortcut_mapping: false,
            },
        )
        .unwrap();
    client
        .send_pointer(PointerEvent::Button {
            button: Button::Left,
            pressed: true,
        })
        .unwrap();
    client
        .send_key(KeyEvent {
            key: Key::A,
            pressed: true,
            repeat: false,
        })
        .unwrap();
    assert!(client.heartbeat().is_err());
    h.wait_for_button_release(Button::Left, Duration::from_secs(1));
    assert!(h.records.lock().unwrap().is_empty());
}

#[test]
fn role_gate_excludes_opposite_direction_and_rolls_back_failed_acquisition() {
    use personal_device_link::network::control::ControlLease;
    let gate = Arc::new(AtomicBool::new(false));
    let access = ControlAccess::new(
        TOKEN.into(),
        Box::new(|| Ok(Box::new(Recorder(Records::default(), Trace::default())))),
    )
    .unwrap()
    .with_gate(Arc::clone(&gate));
    let outbound = ControlLease::reserve(Arc::clone(&gate)).unwrap();
    assert!(access.acquire(TOKEN).is_err());
    assert!(!access.is_active());
    drop(outbound);
    let (incoming, sink) = access.acquire(TOKEN).unwrap();
    assert!(ControlLease::reserve(Arc::clone(&gate)).is_err());
    drop(sink);
    drop(incoming);
    assert!(!gate.load(Ordering::Acquire));
    assert!(ControlLease::reserve(gate).is_ok());
}

#[test]
fn peer_capabilities_and_empty_selection_are_enforced() {
    let h = Harness::start(false);
    let mut client = h.client();
    assert!(!client.capabilities.keyboard && !client.capabilities.pointer);
    assert!(client.start_control(TOKEN.into()).is_err());
    client.heartbeat().unwrap();
    let h = Harness::start(true);
    let mut client = h.client();
    assert!(client
        .start_control_with_options(TOKEN.into(), Default::default())
        .is_err());
    client.heartbeat().unwrap();
}
type Records = Arc<Mutex<Vec<(Key, bool)>>>;
type Trace = Arc<Mutex<Vec<InputEvent>>>;
struct Recorder(Records, Trace);
impl InputSink for Recorder {
    fn send(&mut self, key: Key, pressed: bool) -> io::Result<()> {
        self.0.lock().unwrap().push((key, pressed));
        self.1.lock().unwrap().push(InputEvent::Key(KeyEvent {
            key,
            pressed,
            repeat: false,
        }));
        Ok(())
    }
    fn pointer(&mut self, event: PointerEvent) -> io::Result<()> {
        self.1.lock().unwrap().push(InputEvent::Pointer(event));
        Ok(())
    }
}

struct Harness {
    access: Option<Arc<ControlAccess>>,
    address: SocketAddr,
    running: Arc<AtomicBool>,
    worker: Option<thread::JoinHandle<io::Result<()>>>,
    records: Records,
    trace: Trace,
}
impl Harness {
    fn start(enabled: bool) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let running = Arc::new(AtomicBool::new(true));
        let records: Records = Arc::new(Mutex::new(Vec::new()));
        let copied = Arc::clone(&records);
        let trace = Trace::default();
        let traced = Arc::clone(&trace);
        let control = enabled.then(|| {
            Arc::new(
                ControlAccess::new(
                    TOKEN.into(),
                    Box::new(move || {
                        Ok(Box::new(Recorder(Arc::clone(&copied), Arc::clone(&traced))))
                    }),
                )
                .unwrap(),
            )
        });
        let server_running = Arc::clone(&running);
        let access = control.clone();
        let worker = thread::spawn(move || {
            server::serve_with_control(
                listener,
                Device {
                    name: "Test-Windows".into(),
                    os: "windows".into(),
                },
                &server_running,
                control,
            )
        });
        Self {
            access,
            address,
            running,
            worker: Some(worker),
            records,
            trace,
        }
    }
    fn client(&self) -> Client {
        Client::connect(
            self.address,
            Device {
                name: "Test-Mac".into(),
                os: "macos".into(),
            },
        )
        .unwrap()
    }
    fn wait_for_release(&self, key: Key, timeout: Duration) {
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            if self.records.lock().unwrap().contains(&(key, false)) {
                return;
            }
            thread::sleep(Duration::from_millis(10));
        }
        panic!("key was not released within deadline");
    }
    fn stop(&mut self) {
        self.running.store(false, Ordering::Release);
        if let Some(worker) = self.worker.take() {
            worker.join().unwrap().unwrap();
        }
    }
}
impl Drop for Harness {
    fn drop(&mut self) {
        self.stop();
    }
}
fn down(key: Key) -> KeyEvent {
    KeyEvent {
        key,
        pressed: true,
        repeat: false,
    }
}

#[test]
fn revoking_control_releases_input_without_closing_ordinary_connections() {
    let h = Harness::start(true);
    let access = h.access.as_ref().unwrap();
    let mut ordinary = h.client();
    let mut active = h.client();
    active.start_control(TOKEN.into()).unwrap();
    active.send_key(down(Key::LeftShift)).unwrap();
    active
        .send_pointer(PointerEvent::Button {
            button: Button::Left,
            pressed: true,
        })
        .unwrap();
    active.heartbeat().unwrap();
    access.disable();
    // No more traffic from the controller: the watchdog must still release input.
    h.wait_for_release(Key::LeftShift, Duration::from_secs(3));
    assert!(h
        .trace
        .lock()
        .unwrap()
        .contains(&InputEvent::Pointer(PointerEvent::Button {
            button: Button::Left,
            pressed: false
        })));
    ordinary.heartbeat().unwrap();
    assert!(h.client().start_control(TOKEN.into()).is_err());
    let replacement = "new-code-0123456789abcdef0123456789abcdef";
    access.enable(replacement.into()).unwrap();
    assert!(h.client().start_control(TOKEN.into()).is_err());
    ordinary.start_control(replacement.into()).unwrap();
    ordinary.stop_control().unwrap();
}

#[test]
fn revoked_lease_cannot_become_valid_after_code_rotation() {
    let access = ControlAccess::new(
        TOKEN.into(),
        Box::new(|| Ok(Box::new(Recorder(Records::default(), Trace::default())))),
    )
    .unwrap();
    let (lease, sink) = access.acquire(TOKEN).unwrap();
    access.disable();
    assert!(!lease.is_valid());
    assert!(access.enable(TOKEN.into()).is_err());
    drop(sink);
    drop(lease);
    access.enable(TOKEN.into()).unwrap();
    let (new_lease, _) = access.acquire(TOKEN).unwrap();
    assert!(new_lease.is_valid());
}

#[test]
fn live_update_releases_old_state_pauses_resumes_and_retains_exclusive_lease() {
    use personal_device_link::input::options::ControlOptions;
    let h = Harness::start(true);
    let mut c = h.client();
    assert!(c.update_control(ControlOptions::ALL).is_err());
    c.start_control(TOKEN.into()).unwrap();
    c.send_key(down(Key::LeftControl)).unwrap();
    c.send_pointer(PointerEvent::Button {
        button: Button::Left,
        pressed: true,
    })
    .unwrap();
    c.update_control(ControlOptions::default()).unwrap();
    h.wait_for_release(Key::LeftControl, Duration::from_secs(1));
    assert!(h
        .trace
        .lock()
        .unwrap()
        .contains(&InputEvent::Pointer(PointerEvent::Button {
            button: Button::Left,
            pressed: false
        })));
    assert!(h.client().start_control(TOKEN.into()).is_err());
    c.heartbeat().unwrap();
    c.update_control(ControlOptions {
        keyboard: true,
        pointer: false,
        shortcut_mapping: false,
    })
    .unwrap();
    c.send_key(down(Key::A)).unwrap();
    c.update_control(ControlOptions {
        keyboard: false,
        pointer: true,
        shortcut_mapping: true,
    })
    .unwrap();
    h.wait_for_release(Key::A, Duration::from_secs(1));
    c.send_pointer(PointerEvent::Button {
        button: Button::Right,
        pressed: true,
    })
    .unwrap();
    c.stop_control().unwrap();
    c.heartbeat().unwrap();
}

#[test]
fn live_update_cannot_enable_unselected_input_or_bypass_revocation() {
    use personal_device_link::input::options::ControlOptions;
    let h = Harness::start(true);
    let mut c = h.client();
    c.start_control(TOKEN.into()).unwrap();
    c.update_control(ControlOptions {
        keyboard: false,
        pointer: true,
        shortcut_mapping: true,
    })
    .unwrap();
    c.send_key(down(Key::A)).unwrap();
    assert!(c.heartbeat().is_err());
    assert!(h.records.lock().unwrap().is_empty());
    let h = Harness::start(true);
    let mut c = h.client();
    c.start_control(TOKEN.into()).unwrap();
    h.access.as_ref().unwrap().disable();
    assert!(c.update_control(ControlOptions::ALL).is_err());
}

#[test]
fn disabled_receiver_and_wrong_code_cannot_inject() {
    let disabled = Harness::start(false);
    assert!(disabled.client().start_control(TOKEN.into()).is_err());
    assert!(disabled.records.lock().unwrap().is_empty());
    let enabled = Harness::start(true);
    assert!(enabled
        .client()
        .start_control("wrong-code-0123456789abcdef0123456789".into())
        .is_err());
    assert!(enabled.records.lock().unwrap().is_empty());
}

#[test]
fn stop_releases_modifiers_and_keeps_tcp_usable() {
    let harness = Harness::start(true);
    let mut client = harness.client();
    client.start_control(TOKEN.into()).unwrap();
    client.send_key(down(Key::LeftControl)).unwrap();
    client.send_key(down(Key::A)).unwrap();
    client.heartbeat().unwrap();
    client.stop_control().unwrap();
    harness.wait_for_release(Key::LeftControl, Duration::from_secs(1));
    assert!(client.send_key(down(Key::B)).is_err());
    client.heartbeat().unwrap();
    client.disconnect().unwrap();
}

#[test]
fn abrupt_disconnect_releases_keys_and_allows_new_controller() {
    let harness = Harness::start(true);
    let mut client = harness.client();
    client.start_control(TOKEN.into()).unwrap();
    client.send_key(down(Key::LeftAlt)).unwrap();
    client.heartbeat().unwrap();
    drop(client);
    harness.wait_for_release(Key::LeftAlt, Duration::from_secs(1));
    let mut next = harness.client();
    next.start_control(TOKEN.into()).unwrap();
    next.stop_control().unwrap();
}

#[test]
fn only_one_controller_can_hold_the_lease() {
    let harness = Harness::start(true);
    let mut first = harness.client();
    first.start_control(TOKEN.into()).unwrap();
    let mut second = harness.client();
    assert!(second.start_control(TOKEN.into()).is_err());
    first.stop_control().unwrap();
    let mut third = harness.client();
    third.start_control(TOKEN.into()).unwrap();
    third.stop_control().unwrap();
}

#[test]
fn idle_controller_watchdog_releases_keys() {
    let harness = Harness::start(true);
    let mut client = harness.client();
    client.start_control(TOKEN.into()).unwrap();
    client.send_key(down(Key::LeftShift)).unwrap();
    client.heartbeat().unwrap();
    harness.wait_for_release(Key::LeftShift, Duration::from_millis(3500));
    assert!(client.heartbeat().is_err());
}

#[test]
fn receiver_shutdown_joins_workers_and_releases_before_return() {
    let mut harness = Harness::start(true);
    let mut client = harness.client();
    client.start_control(TOKEN.into()).unwrap();
    client.send_key(down(Key::A)).unwrap();
    client.heartbeat().unwrap();
    harness.stop();
    assert!(harness.records.lock().unwrap().contains(&(Key::A, false)));
}

fn raw_client(harness: &Harness) -> TcpStream {
    let mut stream = TcpStream::connect(harness.address).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(3)))
        .unwrap();
    write_message(
        &mut stream,
        &Message::Hello {
            version: PROTOCOL_VERSION,
            device: Device {
                name: "Raw".into(),
                os: "macos".into(),
            },
            text: "Hello".into(),
        },
    )
    .unwrap();
    assert!(matches!(
        read_message(&mut stream).unwrap(),
        Some(Message::HelloAck { .. })
    ));
    stream
}

#[test]
fn forged_update_without_authorized_session_is_rejected() {
    let h = Harness::start(true);
    let mut stream = raw_client(&h);
    write_message(
        &mut stream,
        &Message::UpdateControl {
            options: personal_device_link::input::options::ControlOptions::ALL,
        },
    )
    .unwrap();
    assert!(matches!(
        read_message(&mut stream).unwrap(),
        Some(Message::Error { .. })
    ));
    assert!(!h.access.as_ref().unwrap().is_active());
    assert!(h.trace.lock().unwrap().is_empty());
}

#[test]
fn forged_keyboard_message_before_authorization_is_rejected() {
    let harness = Harness::start(true);
    let mut stream = raw_client(&harness);
    write_message(
        &mut stream,
        &Message::Keyboard {
            key: Key::A,
            pressed: true,
            repeat: false,
        },
    )
    .unwrap();
    assert!(matches!(
        read_message(&mut stream).unwrap(),
        Some(Message::Error { .. })
    ));
    assert!(harness.records.lock().unwrap().is_empty());
}

#[test]
fn partial_frame_cannot_extend_control_deadline() {
    let harness = Harness::start(true);
    let mut stream = raw_client(&harness);
    write_message(
        &mut stream,
        &Message::ControlRequest {
            options: personal_device_link::input::options::ControlOptions::ALL,
            token: TOKEN.into(),
        },
    )
    .unwrap();
    assert!(matches!(
        read_message(&mut stream).unwrap(),
        Some(Message::ControlStarted {})
    ));
    write_message(
        &mut stream,
        &Message::Keyboard {
            key: Key::LeftControl,
            pressed: true,
            repeat: false,
        },
    )
    .unwrap();
    write_message(&mut stream, &Message::Ping { sequence: 1 }).unwrap();
    read_message(&mut stream).unwrap();
    stream.write_all(&[0]).unwrap();
    thread::sleep(Duration::from_millis(700));
    stream.write_all(&[0]).unwrap();
    thread::sleep(Duration::from_millis(700));
    stream.write_all(&[0]).unwrap();
    harness.wait_for_release(Key::LeftControl, Duration::from_millis(1300));
}

#[test]
fn invalid_control_message_releases_already_pressed_keys() {
    let harness = Harness::start(true);
    let mut client = harness.client();
    client.start_control(TOKEN.into()).unwrap();
    client.send_key(down(Key::LeftShift)).unwrap();
    client.heartbeat().unwrap();
    client
        .send_key(KeyEvent {
            key: Key::B,
            pressed: true,
            repeat: true,
        })
        .unwrap();
    harness.wait_for_release(Key::LeftShift, Duration::from_secs(1));
}

#[test]
fn failed_sink_creation_does_not_leak_exclusive_lease() {
    let attempts = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let counted = Arc::clone(&attempts);
    let records = Arc::new(Mutex::new(Vec::new()));
    let access = ControlAccess::new(
        TOKEN.into(),
        Box::new(move || {
            if counted.fetch_add(1, Ordering::AcqRel) == 0 {
                return Err(io::Error::other("injected setup failure"));
            }
            Ok(Box::new(Recorder(Arc::clone(&records), Trace::default())))
        }),
    )
    .unwrap();
    assert!(access.acquire(TOKEN).is_err());
    assert!(access.acquire(TOKEN).is_ok());
}

fn mouse_button(button: Button, pressed: bool) -> PointerEvent {
    PointerEvent::Button { button, pressed }
}

impl Harness {
    fn wait_for_button_release(&self, button: Button, timeout: Duration) {
        let deadline = Instant::now() + timeout;
        while Instant::now() < deadline {
            if self
                .trace
                .lock()
                .unwrap()
                .contains(&InputEvent::Pointer(mouse_button(button, false)))
            {
                return;
            }
            thread::sleep(Duration::from_millis(10));
        }
        panic!("mouse button was not released within deadline");
    }
}

#[test]
fn tcp_preserves_move_double_click_drag_scroll_and_keyboard_order() {
    let harness = Harness::start(true);
    let mut client = harness.client();
    client.start_control(TOKEN.into()).unwrap();
    client.send_key(down(Key::LeftShift)).unwrap();
    let events = [
        PointerEvent::Move { dx: 12, dy: -3 },
        mouse_button(Button::Left, true),
        mouse_button(Button::Left, false),
        mouse_button(Button::Left, true),
        mouse_button(Button::Left, false),
        mouse_button(Button::Left, true),
        PointerEvent::Move { dx: 10, dy: 5 },
        PointerEvent::Move { dx: -4, dy: 0 },
        mouse_button(Button::Left, false),
        mouse_button(Button::Right, true),
        mouse_button(Button::Right, false),
        PointerEvent::Scroll {
            horizontal: -120,
            vertical: 50,
        },
    ];
    for event in events {
        client.send_pointer(event).unwrap();
    }
    client.stop_control().unwrap();
    let mut expected = vec![InputEvent::Key(down(Key::LeftShift))];
    expected.extend(events.into_iter().map(InputEvent::Pointer));
    expected.push(InputEvent::Key(KeyEvent {
        key: Key::LeftShift,
        pressed: false,
        repeat: false,
    }));
    assert_eq!(*harness.trace.lock().unwrap(), expected);
    assert!(client
        .send_pointer(mouse_button(Button::Left, true))
        .is_err());
    client.heartbeat().unwrap();
}

#[test]
fn stop_during_drag_releases_all_buttons_and_modifier() {
    let harness = Harness::start(true);
    let mut client = harness.client();
    client.start_control(TOKEN.into()).unwrap();
    client.send_key(down(Key::LeftControl)).unwrap();
    for button in [Button::Left, Button::Right, Button::Middle] {
        client.send_pointer(mouse_button(button, true)).unwrap();
    }
    client.stop_control().unwrap();
    for button in [Button::Left, Button::Right, Button::Middle] {
        harness.wait_for_button_release(button, Duration::from_secs(1));
    }
    let trace = harness.trace.lock().unwrap();
    assert_eq!(
        trace.last(),
        Some(&InputEvent::Key(KeyEvent {
            key: Key::LeftControl,
            pressed: false,
            repeat: false
        }))
    );
}

#[test]
fn disconnect_watchdog_and_receiver_shutdown_release_drag() {
    for mode in 0..3 {
        let mut harness = Harness::start(true);
        let mut client = harness.client();
        client.start_control(TOKEN.into()).unwrap();
        client
            .send_pointer(mouse_button(Button::Left, true))
            .unwrap();
        client.heartbeat().unwrap();
        match mode {
            0 => drop(client),
            1 => {
                harness.wait_for_button_release(Button::Left, Duration::from_millis(3500));
            }
            _ => harness.stop(),
        }
        harness.wait_for_button_release(Button::Left, Duration::from_secs(1));
    }
}

#[test]
fn forged_pointer_before_authorization_never_reaches_sink() {
    let harness = Harness::start(true);
    let mut stream = raw_client(&harness);
    write_message(
        &mut stream,
        &Message::Pointer {
            event: mouse_button(Button::Left, true),
        },
    )
    .unwrap();
    assert!(matches!(
        read_message(&mut stream).unwrap(),
        Some(Message::Error { .. })
    ));
    assert!(harness.trace.lock().unwrap().is_empty());
}

#[test]
fn invalid_pointer_frame_ends_session_and_releases_drag() {
    let harness = Harness::start(true);
    let mut stream = raw_client(&harness);
    write_message(
        &mut stream,
        &Message::ControlRequest {
            options: personal_device_link::input::options::ControlOptions::ALL,
            token: TOKEN.into(),
        },
    )
    .unwrap();
    assert_eq!(
        read_message(&mut stream).unwrap(),
        Some(Message::ControlStarted {})
    );
    write_message(
        &mut stream,
        &Message::Pointer {
            event: mouse_button(Button::Left, true),
        },
    )
    .unwrap();
    write_message(
        &mut stream,
        &Message::Pointer {
            event: PointerEvent::Move {
                dx: i32::MAX,
                dy: 0,
            },
        },
    )
    .unwrap();
    harness.wait_for_button_release(Button::Left, Duration::from_secs(1));
    assert_eq!(harness.trace.lock().unwrap().len(), 2);
    // Release also relinquishes ownership for the next controller.
    let mut next = harness.client();
    next.start_control(TOKEN.into()).unwrap();
    next.stop_control().unwrap();
}
