use personal_device_link::input::options::ControlOptions;
use personal_device_link::{
    desktop::{CaptureDriver, Desktop, Snapshot},
    input::{
        keyboard::{Key, KeyEvent},
        pointer::{Button, PointerEvent},
        queue::{InputEvent, InputQueue},
        session::InputSink,
    },
    network::{control::ControlAccess, protocol::Device, server},
};
use std::{
    io,
    net::{SocketAddr, TcpListener},
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc, Mutex,
    },
    thread,
    time::{Duration, Instant},
};
const CODE: &str = "0123456789abcdef0123456789abcdef";

#[test]
fn connection_and_empty_options_never_start_capture() {
    let h = Harness::new();
    h.connect();
    thread::sleep(Duration::from_millis(200));
    assert_eq!(h.capture.started.load(Ordering::Acquire), 0);
    assert!(h.trace.lock().unwrap().is_empty());
    assert_eq!(
        h.desktop.snapshot().connection.controls,
        ControlOptions::default()
    );
    assert!(h
        .desktop
        .start_control_with_options(CODE.into(), ControlOptions::default())
        .is_err());
    assert_eq!(h.desktop.snapshot().connection.phase, "connected");
}

#[test]
fn windows_to_mac_keyboard_only_keeps_pointer_out_of_session() {
    let h = Harness::with_os("windows", "macos");
    h.connect();
    h.desktop
        .start_control_with_options(
            CODE.into(),
            ControlOptions {
                keyboard: true,
                pointer: false,
                shortcut_mapping: true,
            },
        )
        .unwrap();
    h.wait(|s| s.connection.phase == "controlling");
    h.desktop.stop_control();
    h.wait(|s| s.connection.phase == "connected");
    let trace = h.trace.lock().unwrap();
    assert!(trace
        .iter()
        .all(|event| matches!(event, InputEvent::Key(_))));
    assert!(trace.contains(&InputEvent::Key(KeyEvent {
        key: Key::LeftShift,
        pressed: false,
        repeat: false
    })));
    assert_eq!(
        h.desktop.snapshot().connection.controls,
        ControlOptions::default()
    );
}

#[test]
fn pointer_only_session_does_not_forward_keyboard_and_releases_drag() {
    let h = Harness::new();
    h.connect();
    h.desktop
        .start_control_with_options(
            CODE.into(),
            ControlOptions {
                keyboard: false,
                pointer: true,
                shortcut_mapping: false,
            },
        )
        .unwrap();
    h.wait(|s| s.connection.phase == "controlling");
    h.desktop.stop_control();
    h.wait(|s| s.connection.phase == "connected");
    let trace = h.trace.lock().unwrap();
    assert!(trace
        .iter()
        .all(|event| matches!(event, InputEvent::Pointer(_))));
    assert!(trace.contains(&InputEvent::Pointer(PointerEvent::Button {
        button: Button::Left,
        pressed: false
    })));
}

#[test]
fn same_os_pairs_use_the_same_explicit_control_flow() {
    for os in ["windows", "macos"] {
        let h = Harness::with_os(os, os);
        h.connect();
        h.active();
        h.desktop.stop_control();
        h.wait(|s| s.connection.phase == "connected");
        h.released();
    }
}
type Trace = Arc<Mutex<Vec<InputEvent>>>;
struct Sink(Trace);
impl InputSink for Sink {
    fn send(&mut self, key: Key, pressed: bool) -> io::Result<()> {
        self.0.lock().unwrap().push(InputEvent::Key(KeyEvent {
            key,
            pressed,
            repeat: false,
        }));
        Ok(())
    }
    fn pointer(&mut self, event: PointerEvent) -> io::Result<()> {
        self.0.lock().unwrap().push(InputEvent::Pointer(event));
        Ok(())
    }
}
#[derive(Default)]
struct Capture {
    configs: Mutex<Vec<personal_device_link::input::options::CaptureConfig>>,
    queue: Mutex<Option<Arc<InputQueue>>>,
    permission_denied: AtomicBool,
    fail_start: AtomicBool,
    started: AtomicUsize,
    stopped: AtomicUsize,
    stop_barrier: Mutex<Option<(std::sync::mpsc::Sender<()>, std::sync::mpsc::Receiver<()>)>>,
    flag: Mutex<Option<Arc<AtomicBool>>>,
}
impl CaptureDriver for Capture {
    fn permission(&self) -> io::Result<()> {
        if self.permission_denied.load(Ordering::Acquire) {
            Err(io::Error::other("test permission denied"))
        } else {
            Ok(())
        }
    }
    fn start(
        &self,
        queue: Arc<InputQueue>,
        running: Arc<AtomicBool>,
        config: personal_device_link::input::options::CaptureConfig,
    ) -> io::Result<()> {
        if self.fail_start.load(Ordering::Acquire) {
            return Err(io::Error::other("test capture failed"));
        }
        *self.flag.lock().unwrap() = Some(running);
        *self.queue.lock().unwrap() = Some(Arc::clone(&queue));
        self.configs.lock().unwrap().push(config.clone());
        self.started.fetch_add(1, Ordering::AcqRel);
        if config.options.keyboard {
            queue.push(InputEvent::Key(KeyEvent {
                key: Key::LeftShift,
                pressed: true,
                repeat: false,
            }))?;
        }
        if config.options.pointer {
            queue.push(InputEvent::Pointer(PointerEvent::Button {
                button: Button::Left,
                pressed: true,
            }))?;
        }
        Ok(())
    }
    fn stop(&self) -> io::Result<()> {
        self.stopped.fetch_add(1, Ordering::AcqRel);
        if let Some((entered, release)) = self.stop_barrier.lock().unwrap().take() {
            entered.send(()).map_err(io::Error::other)?;
            release
                .recv_timeout(Duration::from_secs(6))
                .map_err(io::Error::other)?;
        }
        if let Some(flag) = self.flag.lock().unwrap().take() {
            flag.store(false, Ordering::Release);
        }
        Ok(())
    }
}
struct Harness {
    desktop: Desktop,
    capture: Arc<Capture>,
    address: SocketAddr,
    trace: Trace,
    running: Arc<AtomicBool>,
    worker: Option<thread::JoinHandle<io::Result<()>>>,
}
impl Harness {
    fn new() -> Self {
        Self::with_os("macos", "windows")
    }
    fn with_os(source_os: &str, target_os: &str) -> Self {
        Self::with_consent(source_os, target_os, None)
    }
    fn with_consent(
        source_os: &str,
        target_os: &str,
        consent: Option<Arc<personal_device_link::consent::Consent>>,
    ) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let running = Arc::new(AtomicBool::new(true));
        let flag = running.clone();
        let trace = Trace::default();
        let recorded = trace.clone();
        let access = ControlAccess::new(
            CODE.into(),
            Box::new(move || Ok(Box::new(Sink(recorded.clone())))),
        )
        .unwrap();
        let access = Arc::new(if let Some(consent) = consent {
            access.with_consent(consent)
        } else {
            access
        });
        let target_os = target_os.to_owned();
        let worker = thread::spawn(move || {
            server::serve_with_control(
                listener,
                Device {
                    name: "Test Windows".into(),
                    os: target_os,
                },
                &flag,
                Some(access),
            )
        });
        let capture = Arc::new(Capture::default());
        let desktop = Desktop::new(
            Device {
                name: "Test Mac".into(),
                os: source_os.into(),
            },
            capture.clone(),
        )
        .unwrap();
        Self {
            desktop,
            capture,
            address,
            trace,
            running,
            worker: Some(worker),
        }
    }
    fn connect(&self) {
        self.desktop
            .connect(None, Some(self.address.to_string()))
            .unwrap();
        self.wait(|s| s.connection.phase == "connected" && s.connection.latency_ms.is_some());
    }
    fn wait(&self, predicate: impl Fn(&Snapshot) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(6);
        while Instant::now() < deadline {
            if predicate(&self.desktop.snapshot()) {
                return;
            }
            thread::sleep(Duration::from_millis(10));
        }
        panic!(
            "GUI state deadline: {}",
            self.desktop.snapshot().connection.phase
        );
    }
    fn active(&self) {
        self.desktop.start_control(CODE.into()).unwrap();
        self.wait(|s| s.connection.phase == "controlling");
        let deadline = Instant::now() + Duration::from_secs(2);
        while self.trace.lock().unwrap().len() < 2 && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(10));
        }
        assert!(self.trace.lock().unwrap().len() >= 2);
    }
    fn released(&self) {
        let trace = self.trace.lock().unwrap();
        assert!(trace.contains(&InputEvent::Pointer(PointerEvent::Button {
            button: Button::Left,
            pressed: false
        })));
        assert!(trace.contains(&InputEvent::Key(KeyEvent {
            key: Key::LeftShift,
            pressed: false,
            repeat: false
        })));
    }
}
impl Drop for Harness {
    fn drop(&mut self) {
        self.desktop.join();
        self.running.store(false, Ordering::Release);
        if let Some(worker) = self.worker.take() {
            worker.join().unwrap().unwrap();
        }
    }
}

#[test]
fn active_options_switch_pause_and_resume_without_reauthorization_or_stale_input() {
    let h = Harness::new();
    h.connect();
    h.active();
    let old_queue = h.capture.queue.lock().unwrap().as_ref().unwrap().clone();
    let keyboard = ControlOptions {
        keyboard: true,
        pointer: false,
        shortcut_mapping: false,
    };
    h.desktop.update_control(keyboard).unwrap();
    assert!(h.desktop.update_control(ControlOptions::ALL).is_err());
    h.wait(|s| s.connection.phase == "controlling" && s.connection.controls == keyboard);
    h.released();
    old_queue
        .push(InputEvent::Key(KeyEvent {
            key: Key::A,
            pressed: true,
            repeat: false,
        }))
        .unwrap();
    h.desktop.update_control(ControlOptions::default()).unwrap();
    h.wait(|s| {
        s.connection.phase == "controlling" && s.connection.controls == ControlOptions::default()
    });
    assert!(h.capture.flag.lock().unwrap().is_none());
    assert_eq!(h.capture.started.load(Ordering::Acquire), 2);
    // A paused authorization stays alive beyond the receiver's input watchdog.
    thread::sleep(Duration::from_millis(2300));
    assert_eq!(h.desktop.snapshot().connection.phase, "controlling");
    let pointer = ControlOptions {
        keyboard: false,
        pointer: true,
        shortcut_mapping: false,
    };
    h.desktop.update_control(pointer).unwrap();
    h.wait(|s| s.connection.phase == "controlling" && s.connection.controls == pointer);
    h.desktop.stop_control();
    h.wait(|s| s.connection.phase == "connected");
    let configs = h.capture.configs.lock().unwrap();
    assert_eq!(
        configs.iter().map(|c| c.options).collect::<Vec<_>>(),
        vec![ControlOptions::ALL, keyboard, pointer]
    );
    assert!(h
        .trace
        .lock()
        .unwrap()
        .iter()
        .all(|e| !matches!(e, InputEvent::Key(KeyEvent { key: Key::A, .. }))));
}

#[test]
fn cancel_switch_never_restarts_capture_and_allows_fresh_control() {
    let h = Harness::new();
    h.connect();
    assert!(h.desktop.update_control(ControlOptions::ALL).is_err());
    h.active();
    // Hold native cleanup so cancellation happens before a permission request.
    // Cancelling after that request may intentionally close the connection.
    let (entered_tx, entered_rx) = std::sync::mpsc::channel();
    let (release_tx, release_rx) = std::sync::mpsc::channel();
    *h.capture.stop_barrier.lock().unwrap() = Some((entered_tx, release_rx));
    h.desktop
        .update_control(ControlOptions {
            keyboard: true,
            pointer: false,
            shortcut_mapping: true,
        })
        .unwrap();
    entered_rx.recv_timeout(Duration::from_secs(6)).unwrap();
    h.desktop.stop_control();
    release_tx.send(()).unwrap();
    h.wait(|s| s.connection.phase == "connected");
    assert_eq!(h.capture.started.load(Ordering::Acquire), 1);
    h.released();
    h.active();
}

#[test]
fn failed_capture_during_switch_stops_remote_and_keeps_plain_connection() {
    let h = Harness::new();
    h.connect();
    h.active();
    h.capture.fail_start.store(true, Ordering::Release);
    h.desktop
        .update_control(ControlOptions {
            keyboard: true,
            pointer: false,
            shortcut_mapping: false,
        })
        .unwrap();
    h.wait(|s| s.connection.phase == "connected" && s.connection.error.is_some());
    assert!(h
        .desktop
        .snapshot()
        .connection
        .error
        .unwrap()
        .contains("capture failed"));
    h.released();
    assert_eq!(
        h.desktop.snapshot().connection.controls,
        ControlOptions::default()
    );
}

#[test]
fn connects_reports_latency_rejects_duplicates_and_disconnects() {
    let h = Harness::new();
    h.connect();
    assert_eq!(h.desktop.snapshot().connection.name, "Test Windows");
    assert!(h
        .desktop
        .connect(None, Some(h.address.to_string()))
        .is_err());
    h.desktop.disconnect();
    h.wait(|s| s.connection.phase == "idle");
    assert!(h.desktop.snapshot().connection.error.is_none());
}
#[test]
fn invalid_address_and_offline_device_are_rejected_without_starting_job() {
    let h = Harness::new();
    assert!(h
        .desktop
        .connect(None, Some("not an address".into()))
        .is_err());
    assert!(h.desktop.connect(Some("missing".into()), None).is_err());
    assert_eq!(h.desktop.snapshot().connection.phase, "idle");
}
#[test]
fn cancel_preparation_never_installs_capture() {
    let h = Harness::new();
    h.connect();
    h.desktop.start_control(CODE.into()).unwrap();
    h.desktop.stop_control();
    h.wait(|s| s.connection.phase == "connected");
    assert_eq!(h.capture.started.load(Ordering::Acquire), 0);
    assert!(h.trace.lock().unwrap().is_empty());
}
#[test]
fn permission_denial_keeps_connection_and_reports_error() {
    let h = Harness::new();
    h.connect();
    h.capture.permission_denied.store(true, Ordering::Release);
    h.desktop.start_control(CODE.into()).unwrap();
    h.wait(|s| s.connection.phase == "connected" && s.connection.error.is_some());
    assert!(h
        .desktop
        .snapshot()
        .connection
        .error
        .unwrap()
        .contains("permission denied"));
    assert_eq!(h.capture.started.load(Ordering::Acquire), 0);
}
#[test]
fn stop_button_and_emergency_release_input_and_allow_restarting() {
    let h = Harness::new();
    h.connect();
    h.active();
    h.desktop.stop_control();
    h.wait(|s| s.connection.phase == "connected");
    h.released();
    h.active();
    h.capture
        .flag
        .lock()
        .unwrap()
        .as_ref()
        .unwrap()
        .store(false, Ordering::Release);
    h.wait(|s| s.connection.phase == "connected");
    h.released();
    assert_eq!(h.capture.stopped.load(Ordering::Acquire), 2);
}
#[test]
fn shutdown_waits_for_capture_cleanup_and_remote_release() {
    let h = Harness::new();
    h.connect();
    h.active();
    h.desktop.join();
    assert!(h.desktop.snapshot().closing);
    h.released();
    assert_eq!(h.capture.stopped.load(Ordering::Acquire), 1);
    assert!(h.desktop.start_control(CODE.into()).is_err());
}
#[test]
fn failed_capture_releases_lease_and_does_not_claim_active() {
    let h = Harness::new();
    h.connect();
    h.capture.fail_start.store(true, Ordering::Release);
    h.desktop.start_control(CODE.into()).unwrap();
    h.wait(|s| s.connection.phase == "connected" && s.connection.error.is_some());
    assert!(h
        .desktop
        .snapshot()
        .connection
        .error
        .unwrap()
        .contains("capture failed"));
    assert!(h.trace.lock().unwrap().is_empty());
}
#[test]
fn wrong_code_reports_rejection_and_returns_to_idle() {
    let h = Harness::new();
    h.connect();
    h.desktop
        .start_control("ffffffffffffffffffffffffffffffff".into())
        .unwrap();
    h.wait(|s| s.connection.phase == "idle" && s.connection.error.is_some());
    assert!(h
        .desktop
        .snapshot()
        .connection
        .error
        .unwrap()
        .contains("incorrect control code"));
}
#[test]
fn background_network_connects_without_enabling_control_and_survives_toggle() {
    let capture = Arc::new(Capture::default());
    let desktop = Desktop::new(
        Device::local("Visible without control".into()).unwrap(),
        capture.clone(),
    )
    .unwrap();
    desktop
        .start_network("127.0.0.1:0".parse().unwrap())
        .unwrap();
    let address = desktop.snapshot().network.address;
    let mut client = personal_device_link::network::client::Client::connect(
        address.parse().unwrap(),
        Device::local("Observer".into()).unwrap(),
    )
    .unwrap();
    client.heartbeat().unwrap();
    assert!(client.capabilities.files);
    assert_eq!(desktop.snapshot().receiver.phase, "listening");
    assert!(desktop.snapshot().receiver.code.is_none());
    desktop.stop_receiver();
    assert!(client
        .start_control(CODE.into())
        .unwrap_err()
        .to_string()
        .contains("关闭控制请求"));
    let mut ordinary = personal_device_link::network::client::Client::connect(
        address.parse().unwrap(),
        Device::local("Ordinary".into()).unwrap(),
    )
    .unwrap();
    desktop
        .start_receiver("127.0.0.1:0".parse().unwrap())
        .unwrap();
    desktop.stop_receiver();
    ordinary.heartbeat().unwrap();
    assert_eq!(desktop.snapshot().network.address, address);
    assert_eq!(capture.started.load(Ordering::Acquire), 0);
    desktop.join();
}

#[test]
fn network_bind_error_is_visible_and_retry_clears_it() {
    let busy = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = busy.local_addr().unwrap();
    let desktop = Desktop::new(
        Device::local("Retry test".into()).unwrap(),
        Arc::new(Capture::default()),
    )
    .unwrap();
    assert!(desktop.start_network(address).is_err());
    assert!(desktop.snapshot().network.error.unwrap().contains("占用"));
    drop(busy);
    desktop.start_network(address).unwrap();
    assert!(desktop.snapshot().network.error.is_none());
    assert!(desktop.snapshot().receiver.code.is_none());
    desktop.join();
}

#[test]
fn receiver_stop_and_restart_has_no_code_or_stale_state() {
    let desktop = Desktop::new(
        Device {
            name: "Test Windows".into(),
            os: "windows".into(),
        },
        Arc::new(Capture::default()),
    )
    .unwrap();
    desktop
        .start_receiver("127.0.0.1:0".parse().unwrap())
        .unwrap();
    assert_eq!(desktop.snapshot().receiver.phase, "listening");
    assert!(desktop.snapshot().receiver.code.is_none());
    desktop.stop_receiver();
    let deadline = Instant::now() + Duration::from_secs(2);
    while desktop.snapshot().receiver.phase != "off" && Instant::now() < deadline {
        thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(desktop.snapshot().receiver.phase, "off");
    desktop
        .start_receiver("127.0.0.1:0".parse().unwrap())
        .unwrap();
    assert_eq!(desktop.snapshot().receiver.phase, "listening");
    assert!(desktop.snapshot().receiver.code.is_none());
    desktop.join();
    assert!(desktop.snapshot().receiver.code.is_none());
}

#[test]
fn desktop_cancels_pending_consent_without_capture_or_stale_popup() {
    let consent = Arc::new(personal_device_link::consent::Consent::default());
    let h = Harness::with_consent("windows", "macos", Some(consent.clone()));
    h.connect();
    h.desktop.start_control(String::new()).unwrap();
    h.wait(|_| !consent.snapshot().is_empty());
    assert_eq!(h.capture.started.load(Ordering::Acquire), 0);
    h.desktop.stop_control();
    h.wait(|s| s.connection.phase == "idle" && consent.snapshot().is_empty());
    assert_eq!(h.capture.started.load(Ordering::Acquire), 0);
}
#[test]
fn desktop_pauses_capture_during_permission_expansion_and_can_withdraw() {
    let consent = Arc::new(personal_device_link::consent::Consent::default());
    let h = Harness::with_consent("macos", "windows", Some(consent.clone()));
    h.connect();
    h.desktop
        .start_control_with_options(
            String::new(),
            ControlOptions {
                keyboard: true,
                pointer: false,
                shortcut_mapping: true,
            },
        )
        .unwrap();
    h.wait(|_| !consent.snapshot().is_empty());
    consent.decide(&consent.snapshot()[0].id, true).unwrap();
    h.wait(|s| s.connection.phase == "controlling");
    h.desktop.update_control(ControlOptions::ALL).unwrap();
    h.wait(|_| {
        consent
            .snapshot()
            .iter()
            .any(|r| r.kind == "control_update")
    });
    assert_eq!(h.capture.started.load(Ordering::Acquire), 1);
    assert_eq!(h.capture.stopped.load(Ordering::Acquire), 1);
    h.desktop.stop_control();
    h.wait(|s| s.connection.phase == "idle" && consent.snapshot().is_empty());
    assert_eq!(h.capture.started.load(Ordering::Acquire), 1);
}
