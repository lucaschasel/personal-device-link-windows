use personal_device_link::{
    clipboard::{wire, Backend, Clipboard, Content},
    network::{
        protocol::{read_message, write_message, Device, Message},
        server,
    },
};
use std::{
    fs,
    io::{self, Write},
    net::{TcpListener, TcpStream},
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc, Mutex,
    },
    thread,
    time::{Duration, Instant},
};

struct Temp(PathBuf);
impl Temp {
    fn new() -> Self {
        let p = std::env::temp_dir().join(format!("pdl-clipboard-test-{}", uuid::Uuid::new_v4()));
        fs::create_dir(&p).unwrap();
        Self(p)
    }
}
impl Drop for Temp {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}
fn wait(mut predicate: impl FnMut() -> bool) {
    let end = Instant::now() + Duration::from_secs(5);
    while !predicate() {
        assert!(Instant::now() < end, "clipboard test timed out");
        thread::sleep(Duration::from_millis(10));
    }
}
fn flag() -> AtomicBool {
    AtomicBool::new(true)
}
fn packet(json: &str) -> Vec<u8> {
    let mut v = (json.len() as u32).to_be_bytes().to_vec();
    v.extend_from_slice(json.as_bytes());
    v
}

#[test]
fn text_and_image_roundtrip_with_integrity_and_size_limits() {
    let temp = Temp::new();
    let cache = wire::Cache::new(temp.0.join("cache"));
    for content in [
        Content::Text("中文\nhttps://example.test/ 🙂".into()),
        Content::Image {
            width: 2,
            height: 1,
            rgba: vec![255, 0, 0, 255, 0, 255, 0, 128],
        },
    ] {
        let mut bytes = vec![];
        wire::send(&mut bytes, Some(&content), &flag()).unwrap();
        assert!(
            wire::receive(&mut bytes.as_slice(), &cache, &flag())
                .unwrap()
                .as_ref()
                == Some(&content)
        );
        let last = bytes.len() - 1;
        bytes[last] ^= 1;
        assert!(wire::receive(&mut bytes.as_slice(), &cache, &flag()).is_err());
    }
    for json in [
        r#"{"kind":"Text","size":1048577}"#,
        r#"{"kind":"Image","width":4294967295,"height":4294967295}"#,
        r#"{"kind":"Files","files":[{"name":"../bad","size":0}]}"#,
        r#"{"kind":"Files","files":[{"name":"a","size":0},{"name":"A","size":0}]}"#,
        r#"{"kind":"Files","files":[{"name":"a","size":268435457}]}"#,
    ] {
        assert!(wire::receive(&mut packet(json).as_slice(), &cache, &flag()).is_err());
    }
    assert!(!temp.0.join("cache").exists());
}

#[test]
fn copied_files_stream_to_new_cache_paths_and_partial_failure_cleans_up() {
    let temp = Temp::new();
    let source = temp.0.join("中文.bin");
    fs::write(&source, vec![42; 2 * 1024 * 1024]).unwrap();
    let content = Content::Files(vec![source.clone()]);
    let mut bytes = vec![];
    wire::send(&mut bytes, Some(&content), &flag()).unwrap();
    let cache = wire::Cache::new(temp.0.join("received"));
    let Some(Content::Files(paths)) =
        wire::receive(&mut bytes.as_slice(), &cache, &flag()).unwrap()
    else {
        panic!("missing files")
    };
    assert_ne!(paths[0], source);
    assert_eq!(fs::read(&paths[0]).unwrap(), fs::read(&source).unwrap());
    let broken = wire::Cache::new(temp.0.join("broken"));
    assert!(wire::receive(&mut &bytes[..bytes.len() - 10], &broken, &flag()).is_err());
    assert_eq!(fs::read_dir(temp.0.join("broken")).unwrap().count(), 0);
    assert!(wire::send(
        &mut vec![],
        Some(&Content::Files(vec![temp.0.clone()])),
        &flag()
    )
    .is_err());
    assert!(wire::send(&mut vec![], Some(&content), &AtomicBool::new(false)).is_err());
}

#[derive(Clone, Default)]
struct Fake {
    content: Arc<Mutex<Option<Content>>>,
    reads: Arc<AtomicUsize>,
    writes: Arc<AtomicUsize>,
}
impl Backend for Fake {
    fn read(&mut self) -> io::Result<Option<Content>> {
        self.reads.fetch_add(1, Ordering::AcqRel);
        Ok(self.content.lock().unwrap().clone())
    }
    fn write(&mut self, c: &Content) -> io::Result<()> {
        *self.content.lock().unwrap() = Some(c.clone());
        self.writes.fetch_add(1, Ordering::AcqRel);
        Ok(())
    }
}
struct Host {
    manager: Arc<Clipboard>,
    fake: Fake,
    device: Device,
    address: std::net::SocketAddr,
    run: Arc<AtomicBool>,
    worker: Option<thread::JoinHandle<io::Result<()>>>,
    _temp: Temp,
}
impl Host {
    fn new(name: &str) -> Self {
        let temp = Temp::new();
        let fake = Fake::default();
        let f = fake.clone();
        let manager = Arc::new(Clipboard::new(
            Box::new(move || Ok(Box::new(f.clone()))),
            temp.0.join("cache"),
        ));
        let device = Device {
            name: name.into(),
            os: "windows".into(),
        };
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let run = Arc::new(AtomicBool::new(true));
        let flag = run.clone();
        let m = manager.clone();
        let d = device.clone();
        let worker = thread::spawn(move || {
            server::serve_with_services(listener, d, &flag, None, None, Some(m))
        });
        Self {
            manager,
            fake,
            device,
            address,
            run,
            worker: Some(worker),
            _temp: temp,
        }
    }
    fn connect(&self, other: &Self) {
        self.manager.enable().unwrap();
        other.manager.enable().unwrap();
        self.manager
            .connect(
                other.address,
                other.manager.snapshot().code.unwrap(),
                self.device.clone(),
                other.device.clone(),
            )
            .unwrap();
        wait(|| {
            self.manager.snapshot().phase == "syncing"
                && other.manager.snapshot().phase == "syncing"
        });
    }
}
impl Drop for Host {
    fn drop(&mut self) {
        self.manager.shutdown();
        self.run.store(false, Ordering::Release);
        self.worker.take().unwrap().join().unwrap().unwrap();
        self.manager.join();
    }
}

#[test]
fn opt_in_pairing_synchronizes_text_images_and_files_without_echo_or_old_content() {
    let a = Host::new("Mac");
    let b = Host::new("PC");
    *a.fake.content.lock().unwrap() = Some(Content::Text("private before pairing".into()));
    assert_eq!(a.fake.reads.load(Ordering::Acquire), 0);
    a.connect(&b);
    thread::sleep(Duration::from_millis(500));
    assert_eq!(b.fake.writes.load(Ordering::Acquire), 0);
    *a.fake.content.lock().unwrap() = Some(Content::Text("新复制文本".into()));
    wait(|| *b.fake.content.lock().unwrap() == Some(Content::Text("新复制文本".into())));
    thread::sleep(Duration::from_millis(600));
    assert_eq!(a.fake.writes.load(Ordering::Acquire), 0);
    let image = Content::Image {
        width: 1,
        height: 1,
        rgba: vec![9, 8, 7, 255],
    };
    *b.fake.content.lock().unwrap() = Some(image.clone());
    wait(|| *a.fake.content.lock().unwrap() == Some(image.clone()));
    let source = a._temp.0.join("copy.txt");
    fs::write(&source, "copied file").unwrap();
    *a.fake.content.lock().unwrap() = Some(Content::Files(vec![source.clone()]));
    wait(|| matches!(&*b.fake.content.lock().unwrap(), Some(Content::Files(_))));
    let Some(Content::Files(paths)) = b.fake.content.lock().unwrap().clone() else {
        unreachable!()
    };
    assert_ne!(source, paths[0]);
    assert_eq!(fs::read(&paths[0]).unwrap(), b"copied file");
    assert!(source.exists());
    a.manager.disable();
    wait(|| a.manager.snapshot().phase == "off");
    let writes = b.fake.writes.load(Ordering::Acquire);
    *a.fake.content.lock().unwrap() = Some(Content::Text("after stop".into()));
    thread::sleep(Duration::from_millis(600));
    assert_eq!(b.fake.writes.load(Ordering::Acquire), writes);
    assert!(a.manager.snapshot().code.is_none());
}

#[test]
fn ordinary_connection_exposes_options_without_enabling_or_reading_clipboard() {
    let h = Host::new("receiver");
    let mut stream = TcpStream::connect(h.address).unwrap();
    write_message(
        &mut stream,
        &Message::Hello {
            version: personal_device_link::network::protocol::PROTOCOL_VERSION,
            device: Device {
                name: "sender".into(),
                os: "windows".into(),
            },
            text: "Hello".into(),
        },
    )
    .unwrap();
    assert!(matches!(
        read_message(&mut stream).unwrap(),
        Some(Message::HelloAck { .. })
    ));
    wait(|| h.manager.snapshot().connections == 1);
    assert!(!h.manager.snapshot().enabled);
    assert_eq!(h.fake.reads.load(Ordering::Acquire), 0);
    drop(stream);
    wait(|| h.manager.snapshot().connections == 0);
    assert_eq!(h.fake.reads.load(Ordering::Acquire), 0);
}

#[test]
fn explicit_send_can_share_preexisting_content_and_repeat_without_echo() {
    let a = Host::new("A");
    let b = Host::new("B");
    assert!(a.manager.send_current().is_err());
    *a.fake.content.lock().unwrap() = Some(Content::Text("already copied".into()));
    a.connect(&b);
    thread::sleep(Duration::from_millis(500));
    assert_eq!(b.fake.writes.load(Ordering::Acquire), 0);
    a.manager.send_current().unwrap();
    wait(|| b.fake.writes.load(Ordering::Acquire) == 1);
    a.manager.send_current().unwrap();
    wait(|| b.fake.writes.load(Ordering::Acquire) == 2);
    assert_eq!(a.fake.writes.load(Ordering::Acquire), 0);
    b.manager.send_current().unwrap();
    wait(|| a.fake.writes.load(Ordering::Acquire) == 1);
    a.manager.disable();
    assert!(a.manager.send_current().is_err());
}

#[test]
fn disabled_or_wrong_code_never_reads_clipboard_and_pairing_is_exclusive() {
    let a = Host::new("A");
    let b = Host::new("B");
    a.manager.enable().unwrap();
    a.manager
        .connect(
            b.address,
            "0".repeat(32),
            a.device.clone(),
            b.device.clone(),
        )
        .unwrap();
    wait(|| {
        let v = a.manager.snapshot();
        v.error.is_some() && v.phase == "ready"
    });
    assert_eq!(a.fake.reads.load(Ordering::Acquire), 0);
    assert_eq!(b.fake.reads.load(Ordering::Acquire), 0);
    b.manager.enable().unwrap();
    a.manager
        .connect(
            b.address,
            "0".repeat(32),
            a.device.clone(),
            b.device.clone(),
        )
        .unwrap();
    wait(|| {
        let v = a.manager.snapshot();
        v.error.is_some() && v.phase == "ready"
    });
    assert_eq!(b.fake.reads.load(Ordering::Acquire), 0);
    a.connect(&b);
    assert!(b
        .manager
        .connect(
            a.address,
            a.manager.snapshot().code.unwrap(),
            b.device.clone(),
            a.device.clone()
        )
        .is_err());
}

#[test]
fn revocation_interrupts_partial_clipboard_payload_without_writing_it() {
    let h = Host::new("receiver");
    h.manager.enable().unwrap();
    let mut stream = TcpStream::connect(h.address).unwrap();
    write_message(
        &mut stream,
        &Message::ClipboardHello {
            version: 1,
            token: h.manager.snapshot().code.unwrap(),
            device: Device {
                name: "sender".into(),
                os: "macos".into(),
            },
        },
    )
    .unwrap();
    assert!(matches!(
        read_message(&mut stream).unwrap(),
        Some(Message::ClipboardReady { .. })
    ));
    stream
        .write_all(&packet(r#"{"kind":"Text","size":10000}"#))
        .unwrap();
    stream.write_all(b"partial").unwrap();
    h.manager.disable();
    wait(|| h.manager.snapshot().phase == "off");
    assert_eq!(h.fake.writes.load(Ordering::Acquire), 0);
}
