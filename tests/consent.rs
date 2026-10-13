use personal_device_link::{
    clipboard::Clipboard,
    consent::Consent,
    input::{
        keyboard::{Key, KeyEvent},
        options::ControlOptions,
        session::InputSink,
    },
    network::{
        client::Client,
        control::ControlAccess,
        protocol::{read_message, Device, Message},
        server,
    },
};
use std::{
    io,
    net::{Shutdown, TcpListener, TcpStream},
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc, Mutex,
    },
    thread,
    time::{Duration, Instant},
};
fn device() -> Device {
    Device {
        name: "测试设备".into(),
        os: "windows".into(),
    }
}
fn wait(mut predicate: impl FnMut() -> bool) {
    let end = Instant::now() + Duration::from_secs(5);
    while !predicate() {
        assert!(Instant::now() < end, "approval test timed out");
        thread::sleep(Duration::from_millis(10));
    }
}
fn pair() -> (TcpStream, TcpStream) {
    let l = TcpListener::bind("127.0.0.1:0").unwrap();
    let c = TcpStream::connect(l.local_addr().unwrap()).unwrap();
    (c, l.accept().unwrap().0)
}
struct Sink(Arc<Mutex<Vec<(Key, bool)>>>);
impl InputSink for Sink {
    fn send(&mut self, key: Key, down: bool) -> io::Result<()> {
        self.0.lock().unwrap().push((key, down));
        Ok(())
    }
}
fn access(
    consent: Arc<Consent>,
    calls: Arc<AtomicUsize>,
    trace: Arc<Mutex<Vec<(Key, bool)>>>,
) -> Arc<ControlAccess> {
    Arc::new(
        ControlAccess::new(
            "0123456789abcdef0123456789abcdef".into(),
            Box::new(move || {
                calls.fetch_add(1, Ordering::SeqCst);
                Ok(Box::new(Sink(trace.clone())))
            }),
        )
        .unwrap()
        .with_consent(consent),
    )
}
#[test]
fn control_code_cannot_bypass_popup_and_rejection_never_opens_input() {
    let consent = Arc::new(Consent::default());
    let calls = Arc::new(AtomicUsize::new(0));
    let access = access(consent.clone(), calls.clone(), Arc::default());
    assert!(access.acquire("0123456789abcdef0123456789abcdef").is_err());
    let (_client, stream) = pair();
    let worker = thread::spawn(move || {
        access
            .acquire_request(
                &stream,
                &device(),
                "0123456789abcdef0123456789abcdef",
                ControlOptions::ALL,
            )
            .is_ok()
    });
    wait(|| !consent.snapshot().is_empty());
    let r = consent.snapshot().remove(0);
    assert_eq!(r.device, device());
    assert_eq!(r.options, Some(ControlOptions::ALL));
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    consent.decide(&r.id, false).unwrap();
    assert!(consent.decide(&r.id, true).is_err());
    assert!(!worker.join().unwrap());
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    assert!(consent.snapshot().is_empty());
}
#[test]
fn disconnect_and_receiver_disable_cancel_pending_control() {
    for disconnect in [true, false] {
        let consent = Arc::new(Consent::default());
        let calls = Arc::new(AtomicUsize::new(0));
        let access = access(consent.clone(), calls.clone(), Arc::default());
        let receiver = access.clone();
        let (client, stream) = pair();
        let worker = thread::spawn(move || {
            receiver
                .acquire_request(&stream, &device(), "", ControlOptions::ALL)
                .is_ok()
        });
        wait(|| !consent.snapshot().is_empty());
        if disconnect {
            client.shutdown(Shutdown::Both).unwrap();
        } else {
            access.disable();
        }
        assert!(!worker.join().unwrap());
        assert!(consent.snapshot().is_empty());
        assert_eq!(calls.load(Ordering::SeqCst), 0);
    }
}
#[test]
fn added_input_permission_requires_fresh_confirmation_and_releases_held_keys() {
    let consent = Arc::new(Consent::default());
    let calls = Arc::new(AtomicUsize::new(0));
    let trace = Arc::new(Mutex::new(vec![]));
    let access = access(consent.clone(), calls.clone(), trace.clone());
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let receiver = thread::spawn(move || {
        server::handle_with_control(listener.accept().unwrap().0, &device(), Some(&access))
    });
    let mut client = Client::connect(address, device()).unwrap();
    let controller = thread::spawn(move || {
        client
            .start_control_with_options(
                String::new(),
                ControlOptions {
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
        client.heartbeat().unwrap();
        client.update_control(ControlOptions::ALL).is_err()
    });
    wait(|| !consent.snapshot().is_empty());
    assert_eq!(calls.load(Ordering::SeqCst), 0);
    consent.decide(&consent.snapshot()[0].id, true).unwrap();
    wait(|| {
        consent
            .snapshot()
            .iter()
            .any(|r| r.kind == "control_update")
    });
    assert_eq!(calls.load(Ordering::SeqCst), 1);
    assert_eq!(
        *trace.lock().unwrap(),
        vec![(Key::LeftShift, true), (Key::LeftShift, false)]
    );
    consent.decide(&consent.snapshot()[0].id, false).unwrap();
    assert!(controller.join().unwrap());
    assert!(receiver.join().unwrap().is_err());
}
#[test]
fn clipboard_requires_approval_even_when_previously_enabled_and_cancels_safely() {
    for allow in [false, true] {
        let consent = Arc::new(Consent::default());
        let calls = Arc::new(AtomicUsize::new(0));
        let count = calls.clone();
        // Factory deliberately fails after approval: no system clipboard is touched.
        let clipboard = Arc::new(
            Clipboard::new(
                Box::new(move || {
                    count.fetch_add(1, Ordering::SeqCst);
                    Err(io::Error::other("mock backend"))
                }),
                std::env::temp_dir().join("unused-consent-test"),
            )
            .with_consent(consent.clone()),
        );
        clipboard.enable().unwrap();
        assert!(clipboard.snapshot().code.is_none());
        let (mut client, stream) = pair();
        let owner = clipboard.clone();
        let worker =
            thread::spawn(move || owner.serve(stream, 2, String::new(), device(), &device()));
        wait(|| !consent.snapshot().is_empty());
        assert_eq!(calls.load(Ordering::SeqCst), 0);
        consent.decide(&consent.snapshot()[0].id, allow).unwrap();
        assert!(worker.join().unwrap().is_err());
        if !allow {
            assert!(matches!(
                read_message(&mut client).unwrap(),
                Some(Message::Error { .. })
            ));
        }
        assert_eq!(calls.load(Ordering::SeqCst), usize::from(allow));
        assert!(consent.snapshot().is_empty());
    }
}
#[test]
fn approved_clipboard_cannot_start_after_local_disable() {
    let consent = Arc::new(Consent::default());
    let calls = Arc::new(AtomicUsize::new(0));
    let count = calls.clone();
    let clipboard = Arc::new(
        Clipboard::new(
            Box::new(move || {
                count.fetch_add(1, Ordering::SeqCst);
                Err(io::Error::other("mock"))
            }),
            std::env::temp_dir().join("unused-consent-test"),
        )
        .with_consent(consent.clone()),
    );
    let (_client, stream) = pair();
    let owner = clipboard.clone();
    let worker = thread::spawn(move || owner.serve(stream, 2, String::new(), device(), &device()));
    wait(|| !consent.snapshot().is_empty());
    clipboard.disable();
    if let Some(request) = consent.snapshot().first() {
        let _ = consent.decide(&request.id, true);
    }
    assert!(worker.join().unwrap().is_err());
    assert_eq!(calls.load(Ordering::SeqCst), 0);
}
#[test]
fn shutdown_and_pipelined_input_never_grant_a_pending_request() {
    use std::io::Write;
    for shutdown in [true, false] {
        let consent = Arc::new(Consent::default());
        let owner = consent.clone();
        let (mut client, stream) = pair();
        let valid = AtomicBool::new(true);
        let worker = thread::spawn(move || {
            owner.request(
                &stream,
                &device(),
                "control",
                Some(ControlOptions::ALL),
                || valid.load(Ordering::Acquire),
            )
        });
        wait(|| !consent.snapshot().is_empty());
        if shutdown {
            consent.shutdown();
        } else {
            client.write_all(&[1]).unwrap();
        }
        assert!(worker.join().unwrap().is_err());
        assert!(consent.snapshot().is_empty());
    }
}

#[test]
fn approved_clipboard_works_without_codes_or_receiver_pre_enabling() {
    use personal_device_link::clipboard::{Backend, Content};
    struct Memory(Arc<Mutex<Option<Content>>>);
    impl Backend for Memory {
        fn read(&mut self) -> io::Result<Option<Content>> {
            Ok(self.0.lock().unwrap().clone())
        }
        fn write(&mut self, value: &Content) -> io::Result<()> {
            *self.0.lock().unwrap() = Some(value.clone());
            Ok(())
        }
    }
    let consent = Arc::new(Consent::default());
    let left = Arc::new(Mutex::new(None));
    let right = Arc::new(Mutex::new(None));
    let a = left.clone();
    let b = right.clone();
    let sender = Arc::new(
        Clipboard::new(
            Box::new(move || Ok(Box::new(Memory(a.clone())))),
            std::env::temp_dir().join("unused-consent-left"),
        )
        .with_consent(Arc::new(Consent::default())),
    );
    let receiver = Arc::new(
        Clipboard::new(
            Box::new(move || Ok(Box::new(Memory(b.clone())))),
            std::env::temp_dir().join("unused-consent-right"),
        )
        .with_consent(consent.clone()),
    );
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let owner = receiver.clone();
    let worker = thread::spawn(move || {
        let mut stream = listener.accept().unwrap().0;
        let Some(Message::ClipboardHello {
            version,
            token,
            device: peer,
        }) = read_message(&mut stream).unwrap()
        else {
            panic!("hello missing")
        };
        owner.serve(stream, version, token, peer, &device())
    });
    sender.enable().unwrap();
    sender
        .connect(address, String::new(), device(), device())
        .unwrap();
    wait(|| !consent.snapshot().is_empty());
    assert!(!receiver.snapshot().enabled);
    consent.decide(&consent.snapshot()[0].id, true).unwrap();
    wait(|| sender.snapshot().phase == "syncing" && receiver.snapshot().phase == "syncing");
    *left.lock().unwrap() = Some(Content::Text("同意后同步".into()));
    wait(|| *right.lock().unwrap() == Some(Content::Text("同意后同步".into())));
    *right.lock().unwrap() = Some(Content::Text("反向同步".into()));
    wait(|| *left.lock().unwrap() == Some(Content::Text("反向同步".into())));
    sender.disable();
    sender.join();
    let _ = worker.join().unwrap();
    assert!(!sender.snapshot().enabled);
    assert!(!receiver.snapshot().enabled);
}
