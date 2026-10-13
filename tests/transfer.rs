use personal_device_link::{
    network::{
        client::Client,
        protocol::{write_message, Device, Message},
        server,
    },
    transfer::{validate_name, TransferView, Transfers, CHUNK_BYTES, FILE_VERSION, MAX_FILE_BYTES},
};
use sha2::{Digest, Sha256};
use std::{
    alloc::{GlobalAlloc, Layout, System},
    fs::{self, File},
    io::{Read, Write},
    net::{SocketAddr, TcpListener, TcpStream},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc,
    },
    thread,
    time::{Duration, Instant},
};

// The isolated 1 GiB test also checks live Rust heap growth and largest
// allocation. No UI or input injection is involved.
struct Meter;
static LIVE: AtomicUsize = AtomicUsize::new(0);
static PEAK: AtomicUsize = AtomicUsize::new(0);
static LARGEST: AtomicUsize = AtomicUsize::new(0);
unsafe impl GlobalAlloc for Meter {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let p = unsafe { System.alloc(layout) };
        if !p.is_null() {
            let live = LIVE.fetch_add(layout.size(), Ordering::Relaxed) + layout.size();
            PEAK.fetch_max(live, Ordering::Relaxed);
            LARGEST.fetch_max(layout.size(), Ordering::Relaxed);
        }
        p
    }
    unsafe fn dealloc(&self, p: *mut u8, layout: Layout) {
        LIVE.fetch_sub(layout.size(), Ordering::Relaxed);
        unsafe { System.dealloc(p, layout) }
    }
}
#[global_allocator]
static ALLOCATOR: Meter = Meter;

struct Temp(PathBuf);
impl Temp {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!("pdl-transfer-test-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
    fn path(&self, name: &str) -> PathBuf {
        self.0.join(name)
    }
    fn partials(&self) -> usize {
        fs::read_dir(&self.0)
            .unwrap()
            .filter_map(Result::ok)
            .filter(|e| e.file_name().to_string_lossy().ends_with(".part"))
            .count()
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        // Only our known flat temporary directory; never recursively remove a supplied path.
        for entry in fs::read_dir(&self.0).unwrap() {
            let _ = fs::remove_file(entry.unwrap().path());
        }
        let _ = fs::remove_dir(&self.0);
    }
}
fn device(name: &str, os: &str) -> Device {
    Device {
        name: name.into(),
        os: os.into(),
    }
}
struct Host {
    manager: Arc<Transfers>,
    device: Device,
    address: SocketAddr,
    flag: Arc<AtomicBool>,
    worker: Option<thread::JoinHandle<()>>,
}
impl Host {
    fn new(device: Device) -> Self {
        Self::with_access(device, None)
    }
    fn with_access(
        device: Device,
        access: Option<Arc<personal_device_link::network::control::ControlAccess>>,
    ) -> Self {
        let manager = Arc::new(Transfers::new(device.clone()));
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let flag = Arc::new(AtomicBool::new(true));
        let (run, local, transfers) = (Arc::clone(&flag), device.clone(), Arc::clone(&manager));
        let worker = thread::spawn(move || {
            server::serve_with_transfers(listener, local, &run, access, Some(transfers)).unwrap()
        });
        Self {
            manager,
            device,
            address,
            flag,
            worker: Some(worker),
        }
    }
    fn wait(&self, predicate: impl Fn(&TransferView) -> bool) -> TransferView {
        let until = Instant::now() + Duration::from_secs(180);
        loop {
            if let Some(view) = self.manager.snapshot().into_iter().find(&predicate) {
                return view;
            }
            assert!(
                Instant::now() < until,
                "timed out: {:?}",
                self.manager
                    .snapshot()
                    .iter()
                    .map(|v| (&v.phase, &v.error))
                    .collect::<Vec<_>>()
            );
            thread::sleep(Duration::from_millis(10));
        }
    }
    fn raw(&self, name: &str, size: u64) -> TcpStream {
        let mut stream = TcpStream::connect(self.address).unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        write_message(
            &mut stream,
            &Message::FileOffer {
                version: FILE_VERSION,
                device: device("Mac", "macos"),
                name: name.into(),
                size,
            },
        )
        .unwrap();
        stream
    }
}
impl Drop for Host {
    fn drop(&mut self) {
        self.manager.shutdown();
        self.flag.store(false, Ordering::Release);
        self.worker.take().unwrap().join().unwrap();
        self.manager.join();
    }
}
fn fixture(path: &Path, size: usize) -> String {
    let mut file = File::create(path).unwrap();
    let data: Vec<_> = (0..CHUNK_BYTES)
        .map(|i| (i.wrapping_mul(31) % 251) as u8)
        .collect();
    let mut hash = Sha256::new();
    let mut remaining = size;
    while remaining > 0 {
        let n = remaining.min(data.len());
        file.write_all(&data[..n]).unwrap();
        hash.update(&data[..n]);
        remaining -= n;
    }
    format!("{:x}", hash.finalize())
}
fn file_hash(path: &Path) -> String {
    let mut f = File::open(path).unwrap();
    let mut h = Sha256::new();
    let mut b = vec![0; CHUNK_BYTES];
    loop {
        let n = f.read(&mut b).unwrap();
        if n == 0 {
            break;
        }
        h.update(&b[..n]);
    }
    format!("{:x}", h.finalize())
}
fn send_roundtrip(size: usize, reversed: bool) {
    let temp = Temp::new();
    let (a, b) = if reversed {
        (device("PC", "windows"), device("Mac", "macos"))
    } else {
        (device("Mac", "macos"), device("PC", "windows"))
    };
    let sender = Host::new(a);
    let receiver = Host::new(b);
    let source = temp.path("源文件.bin");
    let destination = temp.path("收到.bin");
    let hash = fixture(&source, size);
    let id = sender
        .manager
        .send(receiver.address, receiver.device.clone(), source)
        .unwrap();
    let request = receiver.wait(|v| v.phase == "pending");
    assert!(!destination.exists());
    assert_eq!(temp.partials(), 0);
    // A separate control channel still answers while a file waits for approval.
    let mut control = Client::connect(receiver.address, sender.device.clone()).unwrap();
    control.heartbeat().unwrap();
    receiver
        .manager
        .decide(&request.id, Some(destination.clone()))
        .unwrap();
    let sent = sender.wait(|v| v.id == id && matches!(v.phase.as_str(), "complete" | "failed"));
    assert_eq!(sent.phase, "complete", "{:?}", sent.error);
    let received = receiver.wait(|v| v.phase == "complete");
    assert_eq!(sent.sha256.as_deref(), Some(hash.as_str()));
    assert_eq!(received.sha256, sent.sha256);
    assert_eq!(sent.transferred, size as u64);
    assert_eq!(received.transferred, size as u64);
    assert_eq!(file_hash(&destination), hash);
    control.heartbeat().unwrap();
    control.disconnect().unwrap();
    receiver.manager.join();
    assert_eq!(temp.partials(), 0);
}
#[test]
fn control_revocation_does_not_cancel_pending_file_and_file_needs_no_input_sink() {
    use personal_device_link::network::control::ControlAccess;
    let access = Arc::new(ControlAccess::disabled(Box::new(|| {
        panic!("file transfer must not create an input sink")
    })));
    let receiver = Host::with_access(device("PC", "windows"), Some(access.clone()));
    let sender = Host::new(device("Mac", "macos"));
    let temp = Temp::new();
    let source = temp.path("file.bin");
    let hash = fixture(&source, 1024);
    let id = sender
        .manager
        .send(receiver.address, receiver.device.clone(), source)
        .unwrap();
    let request = receiver.wait(|v| v.phase == "pending");
    access
        .enable("0123456789abcdef0123456789abcdef".into())
        .unwrap();
    access.disable();
    let destination = temp.path("received.bin");
    receiver
        .manager
        .decide(&request.id, Some(destination.clone()))
        .unwrap();
    let sent = sender.wait(|v| v.id == id && matches!(v.phase.as_str(), "complete" | "failed"));
    assert_eq!(sent.phase, "complete", "{:?}", sent.error);
    assert_eq!(file_hash(&destination), hash);
}

#[test]
fn one_kib_mac_to_windows() {
    send_roundtrip(1024, false);
}
#[test]
fn ten_mib_windows_to_mac() {
    send_roundtrip(10 * 1024 * 1024, true);
}
#[test]
fn empty_file_checksums_and_saves() {
    send_roundtrip(0, false);
}
#[test]
#[ignore = "explicit 1 GiB streaming and heap acceptance test; run alone in release mode"]
fn one_gib_bounded_memory() {
    let baseline = LIVE.load(Ordering::Relaxed);
    PEAK.store(baseline, Ordering::Relaxed);
    LARGEST.store(0, Ordering::Relaxed);
    send_roundtrip(1024 * 1024 * 1024, false);
    let growth = PEAK.load(Ordering::Relaxed).saturating_sub(baseline);
    let largest = LARGEST.load(Ordering::Relaxed);
    println!("1 GiB passed; peak live Rust heap growth={growth} bytes; largest allocation={largest} bytes");
    assert!(growth < 32 * 1024 * 1024);
    assert!(largest <= CHUNK_BYTES * 2);
}
#[test]
fn malicious_and_incompatible_names_rejected() {
    for name in [
        "",
        ".",
        "..",
        "../secret",
        "C:\\a.txt",
        "/etc/x",
        "a/b",
        "a:b",
        "CON.txt",
        "LPT1",
        "a\n.txt",
        "x.",
        "x ",
    ] {
        assert!(validate_name(name).is_err(), "{name:?}");
    }
    assert!(validate_name(&"a".repeat(256)).is_err());
    for name in ["中文 文件.txt", "archive.tar.gz", "hello.txt"] {
        validate_name(name).unwrap();
    }
}
#[test]
fn reject_never_writes_a_file() {
    let temp = Temp::new();
    let sender = Host::new(device("Mac", "macos"));
    let receiver = Host::new(device("PC", "windows"));
    let path = temp.path("sample");
    fixture(&path, 1024);
    sender
        .manager
        .send(receiver.address, receiver.device.clone(), path)
        .unwrap();
    let request = receiver.wait(|v| v.phase == "pending");
    receiver.manager.decide(&request.id, None).unwrap();
    sender.wait(|v| v.phase == "rejected");
    receiver.wait(|v| v.phase == "rejected");
    assert_eq!(fs::read_dir(&temp.0).unwrap().count(), 1);
}
fn wire(stream: &mut TcpStream, value: serde_json::Value) {
    let body = serde_json::to_vec(&value).unwrap();
    stream
        .write_all(&(body.len() as u32).to_be_bytes())
        .unwrap();
    stream.write_all(&body).unwrap();
}
fn read_wire(stream: &mut TcpStream) -> serde_json::Value {
    let mut length = [0; 4];
    stream.read_exact(&mut length).unwrap();
    let n = u32::from_be_bytes(length) as usize;
    assert!(n <= 8192);
    let mut body = vec![0; n];
    stream.read_exact(&mut body).unwrap();
    serde_json::from_slice(&body).unwrap()
}
fn accepted(receiver: &Host, stream: &mut TcpStream, destination: &Path) -> String {
    let req = receiver.wait(|v| v.phase == "pending");
    receiver
        .manager
        .decide(&req.id, Some(destination.to_owned()))
        .unwrap();
    loop {
        if read_wire(stream)["type"] == "accept" {
            break;
        }
    }
    req.id
}
#[test]
fn bad_hash_cleans_partial_and_never_publishes() {
    let temp = Temp::new();
    let receiver = Host::new(device("PC", "windows"));
    let target = temp.path("new.txt");
    let mut stream = receiver.raw("new.txt", 3);
    accepted(&receiver, &mut stream, &target);
    stream.write_all(&3_u32.to_be_bytes()).unwrap();
    stream.write_all(b"abc").unwrap();
    stream.write_all(&0_u32.to_be_bytes()).unwrap();
    wire(
        &mut stream,
        serde_json::json!({"type":"finish", "sha256":"0".repeat(64)}),
    );
    receiver.wait(|v| v.phase == "failed");
    assert!(!target.exists());
    assert_eq!(temp.partials(), 0);
}
#[test]
fn truncated_data_cleans_partial() {
    let temp = Temp::new();
    let receiver = Host::new(device("PC", "windows"));
    let target = temp.path("new.bin");
    let mut stream = receiver.raw("new.bin", 1024);
    accepted(&receiver, &mut stream, &target);
    stream.write_all(&1024_u32.to_be_bytes()).unwrap();
    stream.write_all(b"abc").unwrap();
    drop(stream);
    receiver.wait(|v| v.phase == "failed");
    assert!(!target.exists());
    assert_eq!(temp.partials(), 0);
}
#[test]
fn oversized_chunk_is_rejected_before_allocation() {
    let temp = Temp::new();
    let receiver = Host::new(device("PC", "windows"));
    let target = temp.path("new.bin");
    let mut stream = receiver.raw("new.bin", MAX_FILE_BYTES);
    accepted(&receiver, &mut stream, &target);
    stream.write_all(&u32::MAX.to_be_bytes()).unwrap();
    receiver.wait(|v| v.phase == "failed");
    assert!(!target.exists());
    assert_eq!(temp.partials(), 0);
}
#[test]
fn existing_destination_rejected_and_collision_at_commit_preserved() {
    let temp = Temp::new();
    let receiver = Host::new(device("PC", "windows"));
    let target = temp.path("new.bin");
    let mut stream = receiver.raw("new.bin", 0);
    let req = receiver.wait(|v| v.phase == "pending");
    fs::write(&target, b"keep").unwrap();
    assert!(receiver
        .manager
        .decide(&req.id, Some(target.clone()))
        .is_err());
    fs::remove_file(&target).unwrap();
    accepted(&receiver, &mut stream, &target);
    fs::write(&target, b"appeared during transfer").unwrap();
    stream.write_all(&0_u32.to_be_bytes()).unwrap();
    wire(
        &mut stream,
        serde_json::json!({"type":"finish", "sha256":format!("{:x}", Sha256::digest([]))}),
    );
    receiver.wait(|v| v.phase == "failed");
    assert_eq!(fs::read(&target).unwrap(), b"appeared during transfer");
    assert_eq!(temp.partials(), 0);
}
#[test]
fn cancel_receiver_interrupts_socket_and_cleans_partial() {
    let temp = Temp::new();
    let receiver = Host::new(device("PC", "windows"));
    let target = temp.path("new.bin");
    let mut stream = receiver.raw("new.bin", 1024 * 1024);
    let id = accepted(&receiver, &mut stream, &target);
    stream
        .write_all(&(CHUNK_BYTES as u32).to_be_bytes())
        .unwrap();
    stream.write_all(&[1; 32]).unwrap();
    receiver.manager.cancel(&id).unwrap();
    drop(receiver);
    assert!(!target.exists());
    assert_eq!(temp.partials(), 0);
}
#[test]
fn cancel_sender_while_waiting_and_reject_stale_decision() {
    let temp = Temp::new();
    let sender = Host::new(device("Mac", "macos"));
    let receiver = Host::new(device("PC", "windows"));
    let path = temp.path("sample");
    fixture(&path, 1024);
    let id = sender
        .manager
        .send(receiver.address, receiver.device.clone(), path)
        .unwrap();
    let req = receiver.wait(|v| v.phase == "pending");
    sender.manager.cancel(&id).unwrap();
    sender.wait(|v| v.phase == "cancelled");
    receiver.wait(|v| v.phase == "failed");
    assert!(receiver
        .manager
        .decide(&req.id, Some(temp.path("later")))
        .is_err());
    assert_eq!(temp.partials(), 0);
}
#[test]
fn shutdown_interrupts_pending_receives_and_sends() {
    let temp = Temp::new();
    let sender = Host::new(device("Mac", "macos"));
    let receiver = Host::new(device("PC", "windows"));
    let path = temp.path("sample");
    fixture(&path, 1024);
    sender
        .manager
        .send(receiver.address, receiver.device.clone(), path)
        .unwrap();
    receiver.wait(|v| v.phase == "pending");
    let start = Instant::now();
    drop(receiver);
    drop(sender);
    assert!(start.elapsed() < Duration::from_secs(3));
    assert_eq!(temp.partials(), 0);
}
#[test]
fn bounded_incoming_requests_and_invalid_size() {
    let receiver = Host::new(device("PC", "windows"));
    let mut streams = vec![];
    for i in 0..4 {
        streams.push(receiver.raw(&format!("{i}.bin"), 1));
        receiver.wait(|v| v.name == format!("{i}.bin"));
    }
    let mut fifth = receiver.raw("fifth.bin", 1);
    assert_eq!(read_wire(&mut fifth)["type"], "failure");
    assert_eq!(receiver.manager.snapshot().len(), 4);
    drop(streams);
    drop(receiver);
    let receiver = Host::new(device("PC", "windows"));
    let mut stream = receiver.raw("huge.bin", MAX_FILE_BYTES + 1);
    assert_eq!(read_wire(&mut stream)["type"], "failure");
    assert!(receiver.manager.snapshot().is_empty());
}

#[test]
fn source_changed_while_waiting_never_commits() {
    let temp = Temp::new();
    let sender = Host::new(device("Mac", "macos"));
    let receiver = Host::new(device("PC", "windows"));
    let path = temp.path("sample");
    let target = temp.path("received");
    fixture(&path, 1024);
    sender
        .manager
        .send(receiver.address, receiver.device.clone(), path.clone())
        .unwrap();
    let req = receiver.wait(|v| v.phase == "pending");
    fs::OpenOptions::new()
        .append(true)
        .open(path)
        .unwrap()
        .write_all(b"changed")
        .unwrap();
    receiver
        .manager
        .decide(&req.id, Some(target.clone()))
        .unwrap();
    let result = sender.wait(|v| v.phase == "failed");
    assert!(result.error.unwrap().contains("发生变化"));
    receiver.wait(|v| v.phase == "failed");
    assert!(!target.exists());
    assert_eq!(temp.partials(), 0);
}

#[test]
fn sender_cancel_interrupts_active_payload_write() {
    let temp = Temp::new();
    let path = temp.path("large");
    fixture(&path, 16 * CHUNK_BYTES);
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let (ready, wait) = std::sync::mpsc::channel();
    let receiver = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        assert!(matches!(
            personal_device_link::network::protocol::read_message(&mut stream).unwrap(),
            Some(Message::FileOffer { .. })
        ));
        wire(
            &mut stream,
            serde_json::json!({"type":"pending", "device":{"name":"PC","os":"windows"}}),
        );
        wire(&mut stream, serde_json::json!({"type":"accept"}));
        let mut first = [0; 36];
        stream.read_exact(&mut first).unwrap();
        ready.send(()).unwrap();
        let mut drain = [0; 8192];
        while let Ok(n) = stream.read(&mut drain) {
            if n == 0 {
                break;
            }
        }
    });
    let sender = Arc::new(Transfers::new(device("Mac", "macos")));
    let id = sender.send(address, device("PC", "windows"), path).unwrap();
    wait.recv_timeout(Duration::from_secs(5)).unwrap();
    let start = Instant::now();
    sender.cancel(&id).unwrap();
    sender.join();
    receiver.join().unwrap();
    assert!(start.elapsed() < Duration::from_secs(3));
    assert_eq!(sender.snapshot()[0].phase, "cancelled");
}

#[test]
fn input_and_heartbeats_continue_during_file_payload() {
    use personal_device_link::input::{
        keyboard::{Key, KeyEvent},
        session::InputSink,
    };
    use personal_device_link::network::control::ControlAccess;
    struct Sink(Arc<AtomicUsize>);
    impl InputSink for Sink {
        fn send(&mut self, _: Key, _: bool) -> std::io::Result<()> {
            self.0.fetch_add(1, Ordering::Relaxed);
            Ok(())
        }
    }
    let count = Arc::new(AtomicUsize::new(0));
    let factory_count = Arc::clone(&count);
    let code = "0123456789abcdef0123456789abcdef";
    let access = Arc::new(
        ControlAccess::new(
            code.into(),
            Box::new(move || Ok(Box::new(Sink(Arc::clone(&factory_count))))),
        )
        .unwrap(),
    );
    let receiver = Host::with_access(device("PC", "windows"), Some(access));
    let temp = Temp::new();
    let target = temp.path("parallel.bin");
    let mut control = Client::connect(receiver.address, device("Mac", "macos")).unwrap();
    control.start_control(code.into()).unwrap();
    let mut stream = receiver.raw("parallel.bin", 64 * 65536);
    accepted(&receiver, &mut stream, &target);
    let chunk = [7; 65536];
    let mut hash = Sha256::new();
    for _ in 0..64 {
        stream
            .write_all(&(chunk.len() as u32).to_be_bytes())
            .unwrap();
        stream.write_all(&chunk).unwrap();
        hash.update(chunk);
        control
            .send_key(KeyEvent {
                key: Key::A,
                pressed: true,
                repeat: false,
            })
            .unwrap();
        control
            .send_key(KeyEvent {
                key: Key::A,
                pressed: false,
                repeat: false,
            })
            .unwrap();
        control.heartbeat().unwrap();
    }
    stream.write_all(&0_u32.to_be_bytes()).unwrap();
    wire(
        &mut stream,
        serde_json::json!({"type":"finish", "sha256":format!("{:x}", hash.finalize())}),
    );
    assert_eq!(read_wire(&mut stream)["type"], "complete");
    control.stop_control().unwrap();
    control.disconnect().unwrap();
    assert_eq!(count.load(Ordering::Relaxed), 128);
    assert_eq!(fs::metadata(target).unwrap().len(), 64 * 65536);
}
