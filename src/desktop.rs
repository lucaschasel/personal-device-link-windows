//! Desktop lifecycle without a dependency on a webview. All socket work stays
//! off the UI thread; CaptureDriver marshals native capture onto the main thread.
use crate::input::options::{CaptureConfig, ControlOptions};
use crate::{
    discovery::mdns::{Discovery, NearbyDevice},
    input::queue::InputQueue,
    network::{
        client::Client,
        control::{forward_input_while, ControlAccess},
        protocol::Device,
        server,
    },
};
use serde::Serialize;
use std::{
    io,
    net::{SocketAddr, TcpListener},
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Arc, Mutex,
    },
    thread,
    time::{Duration, Instant},
};

pub trait CaptureDriver: Send + Sync + 'static {
    fn permission(&self) -> io::Result<()>;
    fn start(
        &self,
        queue: Arc<InputQueue>,
        running: Arc<AtomicBool>,
        config: CaptureConfig,
    ) -> io::Result<()>;
    fn stop(&self) -> io::Result<()>;
}

#[derive(Clone, Serialize)]
pub struct PeerView {
    pub id: String,
    pub name: String,
    pub os: String,
    pub addresses: Vec<String>,
    pub online: bool,
}
impl From<&NearbyDevice> for PeerView {
    fn from(p: &NearbyDevice) -> Self {
        Self {
            id: p.id.clone(),
            name: p.device.name.clone(),
            os: p.device.os.clone(),
            addresses: p.addresses.iter().map(ToString::to_string).collect(),
            online: p.online,
        }
    }
}
#[derive(Clone, Serialize)]
pub struct ConnectionView {
    pub capabilities: crate::network::protocol::Capabilities,
    pub controls: ControlOptions,
    pub phase: String,
    pub name: String,
    pub address: String,
    pub os: String,
    pub error: Option<String>,
    pub latency_ms: Option<f64>,
}
impl Default for ConnectionView {
    fn default() -> Self {
        Self {
            capabilities: Default::default(),
            controls: Default::default(),
            phase: "idle".into(),
            name: String::new(),
            address: String::new(),
            os: String::new(),
            error: None,
            latency_ms: None,
        }
    }
}
#[derive(Clone, Serialize)]
pub struct ReceiverView {
    pub phase: String,
    pub address: String,
    pub code: Option<String>,
    pub active: bool,
    pub error: Option<String>,
}
impl Default for ReceiverView {
    fn default() -> Self {
        Self {
            phase: "off".into(),
            address: String::new(),
            code: None,
            active: false,
            error: None,
        }
    }
}
#[derive(Clone, Serialize)]
pub struct NetworkView {
    pub phase: String,
    pub address: String,
    pub error: Option<String>,
}
impl Default for NetworkView {
    fn default() -> Self {
        Self {
            phase: "off".into(),
            address: String::new(),
            error: None,
        }
    }
}
#[derive(Clone, Serialize)]
pub struct Snapshot {
    pub requests: Vec<crate::consent::Request>,
    pub clipboard: crate::clipboard::View,
    pub device: Device,
    pub peers: Vec<PeerView>,
    pub discovery_error: Option<String>,
    pub connection: ConnectionView,
    pub receiver: ReceiverView,
    pub network: NetworkView,
    pub closing: bool,
    pub transfers: Vec<crate::transfer::TransferView>,
}
struct Shared {
    waiting_consent: AtomicBool,
    socket: Mutex<Option<std::net::TcpStream>>,
    capture_running: Mutex<Option<Arc<AtomicBool>>>,
    control_gate: Arc<AtomicBool>,
    view: Mutex<Snapshot>,
    peers: Mutex<Vec<NearbyDevice>>,
    own_ids: Mutex<std::collections::BTreeMap<String, Instant>>,
    running: AtomicBool,
    driver: Arc<dyn CaptureDriver>,
}
impl Shared {
    fn view(&self, f: impl FnOnce(&mut Snapshot)) {
        f(&mut self.view.lock().unwrap_or_else(|e| e.into_inner()));
    }
}
struct Job {
    running: Arc<AtomicBool>,
    worker: thread::JoinHandle<()>,
}
struct ConnectionJob {
    job: Job,
    control: Arc<AtomicBool>,
    commands: mpsc::SyncSender<ControlCommand>,
}
enum ControlCommand {
    Start(String, ControlOptions),
    Update(ControlOptions),
}
struct NetworkJob {
    job: Job,
    access: Option<Arc<ControlAccess>>,
}

pub struct Desktop {
    pub consent: Arc<crate::consent::Consent>,
    pub clipboard: Arc<crate::clipboard::Clipboard>,
    pub transfers: Arc<crate::transfer::Transfers>,
    shared: Arc<Shared>,
    browser: Mutex<Option<thread::JoinHandle<()>>>,
    receiver: Mutex<Option<NetworkJob>>,
    connection: Mutex<Option<ConnectionJob>>,
}
fn error(message: &str) -> io::Error {
    io::Error::other(message)
}
fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}
fn pause(running: &AtomicBool, duration: Duration) {
    let end = Instant::now() + duration;
    while running.load(Ordering::Acquire) && Instant::now() < end {
        thread::sleep(Duration::from_millis(25));
    }
}

impl Desktop {
    pub fn new(device: Device, driver: Arc<dyn CaptureDriver>) -> io::Result<Self> {
        device.validate()?;
        let consent = Arc::new(crate::consent::Consent::default());
        let clipboard = Arc::new(
            crate::clipboard::Clipboard::new(
                Box::new(crate::platform::clipboard_backend),
                std::env::temp_dir()
                    .join("PersonalDeviceLink-clipboard")
                    .join(uuid::Uuid::new_v4().to_string()),
            )
            .with_consent(Arc::clone(&consent)),
        );
        Ok(Self {
            consent,
            clipboard: Arc::clone(&clipboard),
            transfers: Arc::new(crate::transfer::Transfers::new(device.clone())),
            shared: Arc::new(Shared {
                waiting_consent: AtomicBool::new(false),
                socket: Mutex::new(None),
                capture_running: Mutex::new(None),
                control_gate: Arc::new(AtomicBool::new(false)),
                view: Mutex::new(Snapshot {
                    requests: vec![],
                    clipboard: clipboard.snapshot(),
                    device,
                    peers: vec![],
                    discovery_error: None,
                    connection: ConnectionView::default(),
                    receiver: ReceiverView::default(),
                    network: NetworkView::default(),
                    closing: false,
                    transfers: vec![],
                }),
                peers: Mutex::new(vec![]),
                own_ids: Mutex::new(std::collections::BTreeMap::new()),
                running: AtomicBool::new(true),
                driver,
            }),
            browser: Mutex::new(None),
            receiver: Mutex::new(None),
            connection: Mutex::new(None),
        })
    }
    pub fn snapshot(&self) -> Snapshot {
        let mut snapshot = lock(&self.shared.view).clone();
        snapshot.requests = self.consent.snapshot();
        snapshot.transfers = self.transfers.snapshot();
        snapshot.clipboard = self.clipboard.snapshot();
        snapshot
    }
    pub fn connect_clipboard(&self, code: String) -> io::Result<()> {
        let view = self.snapshot();
        if !matches!(
            view.connection.phase.as_str(),
            "connected" | "controlling" | "updating"
        ) || view.closing
        {
            return Err(error("请先连接目标设备"));
        }
        self.clipboard.enable()?;
        self.clipboard.connect(
            view.connection
                .address
                .parse()
                .map_err(|_| error("目标地址无效"))?,
            code,
            view.device,
            Device {
                name: view.connection.name,
                os: view.connection.os,
            },
        )
    }
    pub fn send_file(&self, path: std::path::PathBuf) -> io::Result<String> {
        let view = self.snapshot();
        if !matches!(
            view.connection.phase.as_str(),
            "connected" | "controlling" | "updating"
        ) || view.closing
        {
            return Err(error("请先连接目标设备"));
        }
        self.transfers.send(
            view.connection
                .address
                .parse()
                .map_err(|_| error("目标地址无效"))?,
            Device {
                name: view.connection.name,
                os: view.connection.os,
            },
            path,
        )
    }
    pub fn start_discovery(&self) -> io::Result<()> {
        let mut job = lock(&self.browser);
        if job.is_some() {
            return Ok(());
        }
        let shared = Arc::clone(&self.shared);
        *job = Some(
            thread::Builder::new()
                .name("gui-discovery".into())
                .spawn(move || {
                    while shared.running.load(Ordering::Acquire) {
                        let result = (|| -> io::Result<()> {
                            let mut discovery = Discovery::start(None)?;
                            shared.view(|v| v.discovery_error = None);
                            while shared.running.load(Ordering::Acquire) {
                                discovery.poll(Duration::from_millis(100))?;
                                let own = {
                                    let mut ids = lock(&shared.own_ids);
                                    ids.retain(|_, seen| seen.elapsed() < Duration::from_secs(120));
                                    ids.clone()
                                };
                                let peers: Vec<_> = discovery
                                    .table()
                                    .snapshot()
                                    .into_iter()
                                    .filter(|p| !own.contains_key(&p.id))
                                    .collect();
                                shared
                                    .view(|v| v.peers = peers.iter().map(PeerView::from).collect());
                                *lock(&shared.peers) = peers;
                            }
                            Ok(())
                        })();
                        if let Err(e) = result {
                            lock(&shared.peers).clear();
                            shared.view(|v| {
                                v.discovery_error = Some(e.to_string());
                                v.peers.clear();
                            });
                            pause(&shared.running, Duration::from_secs(5));
                        }
                    }
                })?,
        );
        Ok(())
    }

    /// LAN visibility and file requests are independent of input authorization.
    pub fn start_network(&self, address: SocketAddr) -> io::Result<()> {
        let result = self.start_network_inner(address);
        if let Err(e) = &result {
            self.shared.view(|v| {
                v.network.phase = "off".into();
                v.network.error = Some(e.to_string());
            });
        }
        result
    }
    fn start_network_inner(&self, address: SocketAddr) -> io::Result<()> {
        if self.snapshot().closing {
            return Err(error("应用正在退出"));
        }
        let mut slot = lock(&self.receiver);
        if slot.as_ref().is_some_and(|j| !j.job.worker.is_finished()) {
            return Ok(());
        }
        if let Some(job) = slot.take() {
            let _ = job.job.worker.join();
        }
        let device = self.snapshot().device;
        let control = matches!(device.os.as_str(), "windows" | "macos");
        let listener = TcpListener::bind(address).map_err(|e| {
            if e.kind() == io::ErrorKind::AddrInUse {
                error("接收端口已被占用。请关闭旧版命令行接收程序或另一个本应用窗口，然后重试。")
            } else {
                e
            }
        })?;
        let bound = listener.local_addr()?;
        let access = control.then(|| {
            Arc::new(
                ControlAccess::new(
                    uuid::Uuid::new_v4().simple().to_string(),
                    Box::new(crate::platform::input_sink),
                )
                .expect("generated token")
                .with_consent(Arc::clone(&self.consent))
                .with_gate(Arc::clone(&self.shared.control_gate)),
            )
        });
        let worker_access = access.clone();
        let running = Arc::new(AtomicBool::new(true));
        let flag = Arc::clone(&running);
        let shared = Arc::clone(&self.shared);
        shared.view(|v| {
            v.network = NetworkView {
                phase: "starting".into(),
                address: bound.to_string(),
                error: None,
            };
            v.receiver = ReceiverView {
                phase: if access.is_some() { "listening" } else { "off" }.into(),
                address: bound.to_string(),
                ..Default::default()
            };
        });
        let transfers = Arc::clone(&self.transfers);
        let clipboard = Arc::clone(&self.clipboard);
        let worker = thread::Builder::new()
            .name("gui-receiver".into())
            .spawn(move || {
                let access = worker_access;
                let mut discovery = None;
                let mut retry = Instant::now();
                let server_flag = Arc::clone(&flag);
                let server_access = access.clone();
                let server_device = device.clone();
                let server = thread::spawn(move || {
                    server::serve_with_services(
                        listener,
                        server_device,
                        &server_flag,
                        server_access,
                        Some(transfers),
                        Some(clipboard),
                    )
                });
                shared.view(|v| {
                    if flag.load(Ordering::Acquire) {
                        v.network.phase = "listening".into();
                    }
                });
                while flag.load(Ordering::Acquire) && !server.is_finished() {
                    shared.view(|v| {
                        v.receiver.active = access.as_ref().is_some_and(|a| a.is_active());
                        if v.receiver.phase == "stopping" && !v.receiver.active {
                            v.receiver.phase = "off".into();
                        }
                    });
                    if discovery.is_none() && !bound.ip().is_loopback() && Instant::now() >= retry {
                        retry = Instant::now() + Duration::from_secs(5);
                        match Discovery::start(Some((&device, bound))) {
                            Ok(d) => {
                                lock(&shared.own_ids).insert(d.own_id().to_owned(), Instant::now());
                                discovery = Some(d);
                                shared.view(|v| v.network.error = None);
                            }
                            Err(e) => shared.view(|v| {
                                v.network.error =
                                    Some(format!("设备广播暂不可用，正在重试；可用 IP 连接：{e}"))
                            }),
                        }
                    }
                    if let Some(d) = discovery.as_mut() {
                        lock(&shared.own_ids).insert(d.own_id().to_owned(), Instant::now());
                        if let Err(e) = d.poll(Duration::from_millis(100)) {
                            shared.view(|v| {
                                v.network.error = Some(format!("设备广播中断，正在重试：{e}"))
                            });
                            discovery = None;
                        }
                    } else {
                        thread::sleep(Duration::from_millis(100));
                    }
                }
                flag.store(false, Ordering::Release);
                let result = server
                    .join()
                    .unwrap_or_else(|_| Err(error("接收线程异常退出")));
                drop(discovery);
                shared.view(|v| {
                    v.receiver = ReceiverView::default();
                    v.network = NetworkView::default();
                    if let Err(e) = result {
                        v.network.error = Some(e.to_string());
                    }
                });
            });
        match worker {
            Ok(worker) => {
                *slot = Some(NetworkJob {
                    job: Job { running, worker },
                    access,
                });
                Ok(())
            }
            Err(e) => {
                self.shared.view(|v| v.receiver = ReceiverView::default());
                Err(e)
            }
        }
    }
    /// Enable only input control, keeping the existing LAN listener and advertisement.
    pub fn start_receiver(&self, address: SocketAddr) -> io::Result<()> {
        self.start_network(address)?;
        if self.snapshot().receiver.phase == "listening" {
            return Ok(());
        }
        let slot = lock(&self.receiver);
        let access = slot
            .as_ref()
            .and_then(|j| j.access.as_ref())
            .ok_or_else(|| error("本机系统不支持输入控制"))?;
        let code = uuid::Uuid::new_v4().simple().to_string();
        access.enable(code.clone())?;
        self.shared.view(|v| {
            v.receiver.phase = "listening".into();
            v.receiver.code = None;
            v.receiver.error = None;
        });
        Ok(())
    }
    pub fn stop_receiver(&self) {
        if let Some(job) = lock(&self.receiver).as_ref() {
            if let Some(access) = &job.access {
                access.disable();
                self.shared.view(|v| {
                    v.receiver.phase = if access.is_active() {
                        "stopping"
                    } else {
                        "off"
                    }
                    .into();
                    v.receiver.code = None;
                });
            }
        }
    }

    pub fn connect(&self, id: Option<String>, address: Option<String>) -> io::Result<()> {
        let (addresses, expected) = match (id, address) {
            (Some(id), None) => {
                let peers = lock(&self.shared.peers);
                let peer = peers
                    .iter()
                    .find(|p| p.id == id && p.online)
                    .ok_or_else(|| error("设备已离线，请重新选择"))?;
                (peer.addresses.clone(), Some(peer.device.clone()))
            }
            (None, Some(address)) => (
                vec![address
                    .trim()
                    .parse::<SocketAddr>()
                    .map_err(|_| error("请输入 IP:端口，例如 192.168.1.20:45832"))?],
                None,
            ),
            _ => return Err(error("请选择设备或填写一个地址")),
        };
        if self.snapshot().closing {
            return Err(error("应用正在退出"));
        }
        let mut slot = lock(&self.connection);
        if slot.as_ref().is_some_and(|j| !j.job.worker.is_finished()) {
            return Err(error("请先断开当前连接"));
        }
        if let Some(job) = slot.take() {
            let _ = job.job.worker.join();
        }
        let running = Arc::new(AtomicBool::new(true));
        let control = Arc::new(AtomicBool::new(false));
        let (tx, rx) = mpsc::sync_channel(1);
        self.shared.view(|v| {
            v.connection = ConnectionView {
                phase: "connecting".into(),
                name: expected
                    .as_ref()
                    .map(|d| d.name.clone())
                    .unwrap_or_else(|| addresses[0].to_string()),
                ..Default::default()
            }
        });
        let shared = Arc::clone(&self.shared);
        let flag = Arc::clone(&running);
        let controlling = Arc::clone(&control);
        let worker = thread::Builder::new()
            .name("gui-connection".into())
            .spawn(move || {
                let result =
                    connection_worker(&shared, &addresses, expected, &flag, &controlling, rx);
                controlling.store(false, Ordering::Release);
                if let Some(socket) = lock(&shared.socket).take() {
                    let _ = socket.shutdown(std::net::Shutdown::Both);
                }
                shared.view(|v| {
                    v.connection.phase = "idle".into();
                    v.connection.controls = ControlOptions::default();
                    v.connection.latency_ms = None;
                    if let Err(e) = result {
                        if flag.load(Ordering::Acquire) && v.connection.error.is_none() {
                            v.connection.error = Some(e.to_string());
                        }
                    }
                });
            });
        match worker {
            Ok(worker) => {
                *slot = Some(ConnectionJob {
                    job: Job { running, worker },
                    control,
                    commands: tx,
                });
                Ok(())
            }
            Err(e) => {
                self.shared.view(|v| v.connection.phase = "idle".into());
                Err(e)
            }
        }
    }
    pub fn start_control(&self, code: String) -> io::Result<()> {
        self.start_control_with_options(code, ControlOptions::ALL)
    }
    pub fn start_control_with_options(
        &self,
        code: String,
        options: ControlOptions,
    ) -> io::Result<()> {
        options.validate()?;
        let code = code.trim().to_owned();
        if !code.is_empty() && (code.len() < 32 || code.len() > 128 || !code.is_ascii()) {
            return Err(error("请输入接收端显示的完整控制码"));
        }
        let slot = lock(&self.connection);
        let job = slot.as_ref().ok_or_else(|| error("请先连接目标设备"))?;
        let mut view = lock(&self.shared.view);
        if view.closing
            || !matches!(view.device.os.as_str(), "macos" | "windows")
            || (options.keyboard && !view.connection.capabilities.keyboard)
            || (options.pointer && !view.connection.capabilities.pointer)
            || view.connection.phase != "connected"
        {
            return Err(error("当前状态不能开始控制"));
        }
        job.control.store(true, Ordering::Release);
        if job
            .commands
            .try_send(ControlCommand::Start(code, options))
            .is_err()
        {
            job.control.store(false, Ordering::Release);
            return Err(error("控制请求正在处理"));
        }
        view.connection.phase = "preparing".into();
        view.connection.controls = options;
        view.connection.error = None;
        Ok(())
    }
    pub fn update_control(&self, options: ControlOptions) -> io::Result<()> {
        let slot = lock(&self.connection);
        let job = slot.as_ref().ok_or_else(|| error("请先开始控制"))?;
        let mut view = lock(&self.shared.view);
        if view.closing
            || view.connection.phase != "controlling"
            || !job.control.load(Ordering::Acquire)
        {
            return Err(error("当前状态不能切换控制功能"));
        }
        if (options.keyboard && !view.connection.capabilities.keyboard)
            || (options.pointer && !view.connection.capabilities.pointer)
        {
            return Err(error("对方不支持所选控制功能"));
        }
        if options == view.connection.controls {
            return Ok(());
        }
        job.commands
            .try_send(ControlCommand::Update(options))
            .map_err(|_| error("控制选项正在处理"))?;
        view.connection.phase = "updating".into();
        view.connection.error = None;
        Ok(())
    }
    pub fn stop_control(&self) {
        if let Some(job) = lock(&self.connection).as_ref() {
            job.control.store(false, Ordering::Release);
            if let Some(flag) = lock(&self.shared.capture_running).as_ref() {
                flag.store(false, Ordering::Release);
            }
            self.shared.view(|v| {
                if self.shared.waiting_consent.load(Ordering::Acquire) {
                    if let Some(socket) = lock(&self.shared.socket).as_ref() {
                        let _ = socket.shutdown(std::net::Shutdown::Both);
                    }
                }
                if matches!(
                    v.connection.phase.as_str(),
                    "preparing" | "controlling" | "updating"
                ) {
                    v.connection.phase = "stopping".into();
                }
            });
        }
    }
    pub fn disconnect(&self) {
        if let Some(socket) = lock(&self.shared.socket).as_ref() {
            let _ = socket.shutdown(std::net::Shutdown::Both);
        }
        self.clipboard.disable();
        self.stop_control();
        if let Some(job) = lock(&self.connection).as_ref() {
            if !job.job.worker.is_finished() {
                job.control.store(false, Ordering::Release);
                job.job.running.store(false, Ordering::Release);
                self.shared
                    .view(|v| v.connection.phase = "disconnecting".into());
            }
        }
    }
    pub fn request_shutdown(&self) {
        self.consent.shutdown();
        self.clipboard.shutdown();
        self.transfers.shutdown();
        self.shared.view(|v| v.closing = true);
        self.shared.running.store(false, Ordering::Release);
        self.stop_control();
        self.disconnect();
        self.stop_receiver();
        if let Some(job) = lock(&self.receiver).as_ref() {
            job.job.running.store(false, Ordering::Release);
        }
    }
    /// Must run off the UI thread: capture shutdown may need the UI main loop.
    pub fn join(&self) {
        self.request_shutdown();
        self.transfers.join();
        self.clipboard.join();
        if let Some(job) = lock(&self.connection).take() {
            let _ = job.job.worker.join();
        }
        if let Some(job) = lock(&self.receiver).take() {
            let _ = job.job.worker.join();
        }
        if let Some(job) = lock(&self.browser).take() {
            let _ = job.join();
        }
    }
}

fn connection_worker(
    shared: &Shared,
    addresses: &[SocketAddr],
    expected: Option<Device>,
    running: &AtomicBool,
    control: &Arc<AtomicBool>,
    commands: mpsc::Receiver<ControlCommand>,
) -> io::Result<()> {
    let local = lock(&shared.view).device.clone();
    let mut last_error = error("无法连接设备");
    let mut connected = None;
    for address in addresses {
        if !running.load(Ordering::Acquire) {
            return Ok(());
        }
        match Client::connect_with_timeout(*address, local.clone(), Duration::from_secs(2)) {
            Ok(client)
                if expected
                    .as_ref()
                    .is_none_or(|device| *device == client.peer) =>
            {
                connected = Some((client, *address));
                break;
            }
            Ok(client) => {
                let _ = client.disconnect();
                last_error = error("设备信息已改变，请重新发现");
            }
            Err(e) => last_error = e,
        }
    }
    let (mut client, address) = connected.ok_or(last_error)?;
    *lock(&shared.socket) = Some(client.cancel_socket()?);
    client.set_idle_timeout(Duration::from_secs(2))?;
    shared.view(|v| {
        if running.load(Ordering::Acquire) {
            v.connection.phase = "connected".into();
        }
        v.connection.name = client.peer.name.clone();
        v.connection.os = client.peer.os.clone();
        v.connection.capabilities = client.capabilities;
        v.connection.address = address.to_string();
    });
    let mut heartbeat = Instant::now() - Duration::from_secs(5);
    while running.load(Ordering::Acquire) {
        match commands.recv_timeout(Duration::from_millis(50)) {
            Ok(ControlCommand::Start(code, options)) => {
                let preparation = (|| -> io::Result<()> {
                    let _role = crate::network::control::ControlLease::reserve(Arc::clone(
                        &shared.control_gate,
                    ))?;
                    if !control.load(Ordering::Acquire) {
                        return Ok(());
                    }
                    shared.driver.permission()?;
                    pause(control, Duration::from_secs(3));
                    if !control.load(Ordering::Acquire) {
                        return Ok(());
                    }
                    shared.waiting_consent.store(true, Ordering::Release);
                    let approved = if control.load(Ordering::Acquire) {
                        client.start_control_with_options(code, options)
                    } else {
                        Ok(())
                    };
                    shared.waiting_consent.store(false, Ordering::Release);
                    approved?;
                    let forwarded =
                        active_control(shared, &mut client, control, &commands, options);
                    control.store(false, Ordering::Release);
                    let stopped = client.stop_control();
                    forwarded.and(stopped)
                })();
                control.store(false, Ordering::Release);
                shared.view(|v| {
                    v.connection.controls = ControlOptions::default();
                    if running.load(Ordering::Acquire) {
                        v.connection.phase = "connected".into();
                    }
                    if let Err(e) = preparation {
                        v.connection.error = Some(e.to_string());
                    }
                });
                // Also detects rejected control requests that closed the socket.
                heartbeat = Instant::now() - Duration::from_secs(5);
            }
            // A cancelled transition must never become a new control session.
            Ok(ControlCommand::Update(_)) => {}
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
        if running.load(Ordering::Acquire) && heartbeat.elapsed() >= Duration::from_secs(2) {
            let elapsed = client.heartbeat()?;
            shared.view(|v| v.connection.latency_ms = Some(elapsed.as_secs_f64() * 1000.0));
            heartbeat = Instant::now();
        }
    }
    client.disconnect()
}

fn active_control(
    shared: &Shared,
    client: &mut Client,
    control: &AtomicBool,
    commands: &mpsc::Receiver<ControlCommand>,
    mut options: ControlOptions,
) -> io::Result<()> {
    while control.load(Ordering::Acquire) {
        // Each capture owns its flag and queue. Stopping an old native capture
        // cannot cancel a new one, and queued events never cross a transition.
        let queue = Arc::new(InputQueue::new(512));
        let capture_running = Arc::new(AtomicBool::new(true));
        *lock(&shared.capture_running) = Some(Arc::clone(&capture_running));
        if !control.load(Ordering::Acquire) {
            capture_running.store(false, Ordering::Release);
            lock(&shared.capture_running).take();
            return Ok(());
        }
        let enabled = options.keyboard || options.pointer;
        if enabled {
            if let Err(e) = shared.driver.start(
                Arc::clone(&queue),
                Arc::clone(&capture_running),
                CaptureConfig {
                    options,
                    target_os: client.peer.os.clone(),
                },
            ) {
                capture_running.store(false, Ordering::Release);
                let _ = shared.driver.stop();
                lock(&shared.capture_running).take();
                return Err(e);
            }
        }
        shared.view(|v| {
            if control.load(Ordering::Acquire) && v.connection.phase != "disconnecting" {
                v.connection.phase = "controlling".into();
                v.connection.controls = options;
            }
        });
        let mut next = None;
        let forwarded = forward_input_while(client, &queue, &capture_running, || {
            if !control.load(Ordering::Acquire) {
                return false;
            }
            if let Ok(ControlCommand::Update(options)) = commands.try_recv() {
                next = Some(options);
                return false;
            }
            true
        });
        capture_running.store(false, Ordering::Release);
        let cleanup = if enabled {
            shared.driver.stop()
        } else {
            Ok(())
        };
        lock(&shared.capture_running).take();
        forwarded.and(cleanup)?;
        if !control.load(Ordering::Acquire) {
            return Ok(());
        }
        let Some(new_options) = next else {
            return Ok(());
        };
        // The receiver releases keys/buttons and acknowledges before fresh
        // capture starts. Its authorization lease stays held, even when paused.
        shared.waiting_consent.store(true, Ordering::Release);
        let updated = if control.load(Ordering::Acquire) {
            client.update_control(new_options)
        } else {
            Ok(())
        };
        shared.waiting_consent.store(false, Ordering::Release);
        updated?;
        options = new_options;
        if options.keyboard || options.pointer {
            let until = Instant::now() + Duration::from_secs(3);
            while control.load(Ordering::Acquire) && Instant::now() < until {
                client.heartbeat()?;
                pause(control, Duration::from_millis(400));
            }
        }
    }
    Ok(())
}
