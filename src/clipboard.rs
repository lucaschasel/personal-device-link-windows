//! Authenticated, opt-in clipboard exchange on its own TCP connection.
//! Native clipboard access only happens inside an authorized session.
#[cfg(any(windows, target_os = "macos"))]
pub mod native;
pub mod wire;

use crate::network::{
    configure_stream,
    protocol::{write_message, Device, Message},
};
use serde::Serialize;
use sha2::{Digest, Sha256};
use std::{
    io,
    net::{Shutdown, SocketAddr, TcpStream},
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    thread,
    time::Duration,
};

#[derive(Clone, PartialEq, Eq)]
pub enum Content {
    Text(String),
    Image {
        width: u32,
        height: u32,
        rgba: Vec<u8>,
    },
    Files(Vec<PathBuf>),
}
pub trait Backend: Send {
    fn read(&mut self) -> io::Result<Option<Content>>;
    fn write(&mut self, content: &Content) -> io::Result<()>;
}
pub type Factory = dyn Fn() -> io::Result<Box<dyn Backend>> + Send + Sync;
pub fn lock<T>(value: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    value.lock().unwrap_or_else(|e| e.into_inner())
}
fn fail(message: &str) -> io::Error {
    io::Error::other(message)
}

fn fingerprint(content: &Option<Content>) -> io::Result<Vec<u8>> {
    let mut h = Sha256::new();
    match content {
        None => h.update([0]),
        Some(Content::Text(text)) => {
            h.update([1]);
            h.update(text.as_bytes());
        }
        Some(Content::Image {
            width,
            height,
            rgba,
        }) => {
            h.update([2]);
            h.update(width.to_be_bytes());
            h.update(height.to_be_bytes());
            h.update(rgba);
        }
        Some(Content::Files(paths)) => {
            h.update([3]);
            for path in paths {
                h.update(path.to_string_lossy().as_bytes());
                h.update([0]);
                let meta = std::fs::symlink_metadata(path)?;
                h.update(meta.len().to_be_bytes());
                h.update(format!("{:?}", meta.modified()?).as_bytes());
            }
        }
    }
    Ok(h.finalize().to_vec())
}
struct Tracker {
    backend: Box<dyn Backend>,
    last: Vec<u8>,
}
impl Tracker {
    fn new(mut backend: Box<dyn Backend>) -> io::Result<Self> {
        // Do not send content copied before the user established this session.
        let last = fingerprint(&backend.read()?)?;
        Ok(Self { backend, last })
    }
    fn poll(&mut self, force: bool) -> io::Result<Option<Content>> {
        let content = self.backend.read()?;
        let hash = fingerprint(&content)?;
        if hash == self.last && !force {
            return Ok(None);
        }
        self.last = hash;
        Ok(content)
    }
    fn apply(&mut self, content: &Content) -> io::Result<bool> {
        // A local copy made while a file was arriving takes precedence.
        if fingerprint(&self.backend.read()?)? != self.last {
            return Ok(false);
        }
        self.backend.write(content)?;
        self.last = fingerprint(&self.backend.read()?)?;
        Ok(true)
    }
}

#[derive(Clone, Serialize)]
pub struct View {
    pub connections: usize,
    pub enabled: bool,
    pub code: Option<String>,
    pub phase: String,
    pub peer: String,
    pub error: Option<String>,
    pub sent: u64,
    pub received: u64,
    pub activity: String,
    pub cache_path: String,
}
struct Session {
    id: String,
    running: Arc<AtomicBool>,
    socket: Option<TcpStream>,
}
struct State {
    view: View,
    session: Option<Session>,
    closing: bool,
    epoch: u64,
}
pub struct Clipboard {
    consent: Option<Arc<crate::consent::Consent>>,
    state: Mutex<State>,
    send_current: AtomicBool,
    factory: Box<Factory>,
    cache: wire::Cache,
    workers: Mutex<Vec<thread::JoinHandle<()>>>,
}
struct Lease {
    owner: Arc<Clipboard>,
    id: String,
}
pub struct Connection(Arc<Clipboard>);
impl Drop for Connection {
    fn drop(&mut self) {
        let mut state = lock(&self.0.state);
        state.view.connections = state.view.connections.saturating_sub(1);
    }
}
impl Drop for Lease {
    fn drop(&mut self) {
        let mut state = lock(&self.owner.state);
        if state.session.as_ref().is_some_and(|s| s.id == self.id) {
            state.session = None;
            if self.owner.consent.is_some() {
                state.view.enabled = false;
                state.view.code = None;
            }
            state.view.phase = if state.view.enabled { "ready" } else { "off" }.into();
            state.view.peer.clear();
        }
    }
}
impl Clipboard {
    pub fn new(factory: Box<Factory>, cache: PathBuf) -> Self {
        Self {
            consent: None,
            state: Mutex::new(State {
                view: View {
                    connections: 0,
                    enabled: false,
                    code: None,
                    phase: "off".into(),
                    peer: String::new(),
                    error: None,
                    sent: 0,
                    received: 0,
                    activity: String::new(),
                    cache_path: cache.display().to_string(),
                },
                session: None,
                closing: false,
                epoch: 0,
            }),
            factory,
            send_current: AtomicBool::new(false),
            cache: wire::Cache::new(cache),
            workers: Mutex::new(vec![]),
        }
    }
    pub fn with_consent(mut self, consent: Arc<crate::consent::Consent>) -> Self {
        self.consent = Some(consent);
        self
    }
    pub fn snapshot(&self) -> View {
        lock(&self.state).view.clone()
    }
    pub fn connected(self: &Arc<Self>) -> Connection {
        lock(&self.state).view.connections += 1;
        Connection(self.clone())
    }
    pub fn enable(&self) -> io::Result<()> {
        let mut state = lock(&self.state);
        if state.closing {
            return Err(fail("应用正在退出"));
        }
        if state.session.is_some() {
            return Err(fail("请先停止当前剪贴板会话"));
        }
        if !state.view.enabled {
            state.view.enabled = true;
            state.view.code = if self.consent.is_none() {
                Some(uuid::Uuid::new_v4().simple().to_string())
            } else {
                None
            };
            state.view.phase = "ready".into();
            state.view.error = None;
        }
        Ok(())
    }
    pub fn disable(&self) {
        let mut state = lock(&self.state);
        state.epoch = state.epoch.wrapping_add(1);
        state.view.enabled = false;
        state.view.code = None;
        state.view.phase = if state.session.is_some() {
            "stopping"
        } else {
            "off"
        }
        .into();
        if let Some(s) = &state.session {
            s.running.store(false, Ordering::Release);
            if let Some(socket) = &s.socket {
                let _ = socket.shutdown(Shutdown::Both);
            }
        }
    }
    pub fn send_current(&self) -> io::Result<()> {
        let state = lock(&self.state);
        if !state.view.enabled || state.view.phase != "syncing" {
            return Err(fail("请先建立已获同意的剪贴板共享会话"));
        }
        self.send_current.store(true, Ordering::Release);
        Ok(())
    }
    pub fn shutdown(&self) {
        lock(&self.state).closing = true;
        self.disable();
    }
    pub fn join(&self) {
        self.shutdown();
        for worker in lock(&self.workers).drain(..) {
            let _ = worker.join();
        }
    }
    fn reserve(
        self: &Arc<Self>,
        token: Option<&str>,
        peer: &Device,
    ) -> io::Result<(Lease, Arc<AtomicBool>)> {
        self.reserve_inner(token, peer, None)
    }
    fn reserve_inner(
        self: &Arc<Self>,
        token: Option<&str>,
        peer: &Device,
        approved_epoch: Option<u64>,
    ) -> io::Result<(Lease, Arc<AtomicBool>)> {
        peer.validate()?;
        let mut state = lock(&self.state);
        if let Some(epoch) = approved_epoch {
            if state.closing || state.epoch != epoch || state.session.is_some() {
                return Err(fail("剪贴板请求已取消或已有同步会话"));
            }
            state.view.enabled = true;
            state.view.code = None;
        }
        if !state.view.enabled || state.closing {
            return Err(fail("对方尚未开启剪贴板共享"));
        }
        if let Some(token) = token {
            if state.view.code.as_deref() != Some(token) {
                return Err(fail("剪贴板配对码错误"));
            }
        }
        if state.session.is_some() {
            return Err(fail("剪贴板正在与另一台设备同步"));
        }
        let id = uuid::Uuid::new_v4().to_string();
        let running = Arc::new(AtomicBool::new(true));
        self.send_current.store(false, Ordering::Release);
        state.session = Some(Session {
            id: id.clone(),
            running: running.clone(),
            socket: None,
        });
        state.view.phase = "connecting".into();
        state.view.peer = peer.name.clone();
        state.view.error = None;
        Ok((
            Lease {
                owner: self.clone(),
                id,
            },
            running,
        ))
    }
    fn attach(&self, stream: &TcpStream, id: &str) -> io::Result<()> {
        configure_stream(stream)?;
        stream.set_read_timeout(Some(Duration::from_secs(10)))?;
        stream.set_write_timeout(Some(Duration::from_secs(10)))?;
        let mut state = lock(&self.state);
        let session = state
            .session
            .as_mut()
            .filter(|s| s.id == id && s.running.load(Ordering::Acquire))
            .ok_or_else(|| fail("剪贴板同步已取消"))?;
        session.socket = Some(stream.try_clone()?);
        Ok(())
    }
    fn active(&self, running: &AtomicBool) -> io::Result<()> {
        if running.load(Ordering::Acquire) && lock(&self.state).view.enabled {
            Ok(())
        } else {
            Err(fail("剪贴板同步已停止"))
        }
    }
    fn report(&self, result: &io::Result<()>, running: &AtomicBool) {
        if running.load(Ordering::Acquire) {
            if let Err(e) = result {
                lock(&self.state).view.error = Some(e.to_string());
            }
        }
    }
    fn count(&self, sent: bool, content: &Content) {
        let mut s = lock(&self.state);
        if sent {
            s.view.sent += 1;
        } else {
            s.view.received += 1;
        }
        s.view.activity = match content {
            Content::Text(_) => "文本",
            Content::Image { .. } => "图片",
            Content::Files(_) => "文件",
        }
        .into();
    }
    fn apply(
        &self,
        tracker: &mut Tracker,
        content: &Content,
        running: &AtomicBool,
    ) -> io::Result<bool> {
        let state = lock(&self.state);
        if !state.view.enabled || !running.load(Ordering::Acquire) {
            return Err(fail("剪贴板同步已停止"));
        }
        tracker.apply(content)
    }
    pub fn connect(
        self: &Arc<Self>,
        address: SocketAddr,
        code: String,
        local: Device,
        expected: Device,
    ) -> io::Result<()> {
        if !code.is_empty() && (code.len() != 32 || !code.is_ascii()) {
            return Err(fail("请输入对方完整的剪贴板配对码"));
        }
        let (lease, running) = self.reserve(None, &expected)?;
        let owner = self.clone();
        let worker = thread::Builder::new()
            .name("clipboard-sync".into())
            .spawn(move || {
                let result = (|| {
                    let mut stream = TcpStream::connect_timeout(&address, Duration::from_secs(2))?;
                    owner.attach(&stream, &lease.id)?;
                    write_message(
                        &mut stream,
                        &Message::ClipboardHello {
                            version: if code.is_empty() { 2 } else { 1 },
                            token: code,
                            device: local,
                        },
                    )?;
                    match crate::network::read_with_deadline(
                        &mut stream,
                        crate::consent::WAIT_TIMEOUT,
                    )? {
                        Some(Message::ClipboardReady { device }) if device == expected => (),
                        Some(Message::Error { message }) => return Err(fail(&message)),
                        _ => return Err(fail("剪贴板握手不匹配，请确认两端均为新版")),
                    }
                    stream.set_read_timeout(Some(Duration::from_secs(10)))?;
                    owner.active(&running)?;
                    let mut tracker = Tracker::new((owner.factory)()?)?;
                    lock(&owner.state).view.phase = "syncing".into();
                    while running.load(Ordering::Acquire) {
                        owner.active(&running)?;
                        let outgoing =
                            tracker.poll(owner.send_current.swap(false, Ordering::AcqRel))?;
                        wire::send(&mut stream, outgoing.as_ref(), &running)?;
                        if let Some(c) = &outgoing {
                            owner.count(true, c);
                        }
                        let incoming = wire::receive(&mut stream, &owner.cache, &running)?;
                        owner.active(&running)?;
                        if let Some(c) = incoming {
                            if owner.apply(&mut tracker, &c, &running)? {
                                owner.count(false, &c);
                            }
                        }
                        for _ in 0..10 {
                            if !running.load(Ordering::Acquire) {
                                break;
                            }
                            thread::sleep(Duration::from_millis(40));
                        }
                    }
                    Ok(())
                })();
                owner.report(&result, &running);
                drop(lease);
            })?;
        let mut workers = lock(&self.workers);
        let mut i = 0;
        while i < workers.len() {
            if workers[i].is_finished() {
                let _ = workers.swap_remove(i).join();
            } else {
                i += 1;
            }
        }
        workers.push(worker);
        Ok(())
    }
    pub fn serve(
        self: &Arc<Self>,
        mut stream: TcpStream,
        version: u16,
        token: String,
        peer: Device,
        local: &Device,
    ) -> io::Result<()> {
        let reservation = (|| {
            if let Some(consent) = &self.consent {
                if version != 2 {
                    return Err(fail("剪贴板授权协议已更新，请升级两端"));
                }
                let epoch = {
                    let state = lock(&self.state);
                    if state.closing || state.session.is_some() {
                        return Err(fail("剪贴板已有会话或正在退出"));
                    }
                    state.epoch
                };
                consent.request(&stream, &peer, "clipboard", None, || {
                    let state = lock(&self.state);
                    !state.closing && state.epoch == epoch && state.session.is_none()
                })?;
                self.reserve_inner(None, &peer, Some(epoch))
            } else if version == 1 {
                self.reserve(Some(&token), &peer)
            } else {
                Err(fail("剪贴板协议版本不兼容"))
            }
        })();
        let (lease, running) = match reservation {
            Ok(r) => r,
            Err(e) => {
                write_message(
                    &mut stream,
                    &Message::Error {
                        message: e.to_string(),
                    },
                )?;
                return Err(e);
            }
        };
        let result = (|| {
            self.attach(&stream, &lease.id)?;
            self.active(&running)?;
            let mut tracker = Tracker::new((self.factory)()?)?;
            write_message(
                &mut stream,
                &Message::ClipboardReady {
                    device: local.clone(),
                },
            )?;
            lock(&self.state).view.phase = "syncing".into();
            while running.load(Ordering::Acquire) {
                let incoming = wire::receive(&mut stream, &self.cache, &running)?;
                self.active(&running)?;
                // Copies made on the receiver during this exchange win conflicts.
                let outgoing = tracker.poll(self.send_current.swap(false, Ordering::AcqRel))?;
                if outgoing.is_none() {
                    if let Some(c) = incoming {
                        if self.apply(&mut tracker, &c, &running)? {
                            self.count(false, &c);
                        }
                    }
                }
                wire::send(&mut stream, outgoing.as_ref(), &running)?;
                if let Some(c) = &outgoing {
                    self.count(true, c);
                }
            }
            Ok(())
        })();
        self.report(&result, &running);
        drop(lease);
        result
    }
}
