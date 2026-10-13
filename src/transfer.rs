//! One independent TCP stream per file. Memory is bounded by CHUNK_BYTES, never
//! the advertised size. A destination is created only after local acceptance.
use crate::network::protocol::{invalid_data, write_message, Device, Message};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File, OpenOptions},
    io::{self, Read, Write},
    net::{Shutdown, SocketAddr, TcpStream},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    thread,
    time::{Duration, Instant},
};

pub const FILE_VERSION: u16 = 1;
pub const CHUNK_BYTES: usize = 1024 * 1024;
pub const MAX_FILE_BYTES: u64 = 1024 * 1024 * 1024 * 1024;
const MAX_ACTIVE: usize = 4;
const HISTORY: usize = 50;
const WAIT: Duration = Duration::from_secs(120);
const IO_TIMEOUT: Duration = Duration::from_secs(15);

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}
fn error(s: impl Into<String>) -> io::Error {
    io::Error::other(s.into())
}
fn terminal(phase: &str) -> bool {
    matches!(phase, "complete" | "failed" | "cancelled" | "rejected")
}
pub fn validate_name(name: &str) -> io::Result<()> {
    let stem = name.split('.').next().unwrap_or("").to_ascii_uppercase();
    if name.is_empty()
        || name.len() > 255
        || name == "."
        || name == ".."
        || name.ends_with([' ', '.'])
        || name
            .chars()
            .any(|c| c.is_control() || "\\/:*?\"<>|".contains(c))
        || matches!(stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || (stem.len() == 4
            && (stem.starts_with("COM") || stem.starts_with("LPT"))
            && stem.as_bytes()[3].is_ascii_digit())
    {
        return Err(invalid_data("文件名不兼容 Windows/macOS，请重命名后发送"));
    }
    Ok(())
}

#[derive(Clone, Serialize)]
pub struct TransferView {
    pub id: String,
    pub direction: String,
    pub peer: String,
    pub peer_os: String,
    pub transport: String,
    pub address: String,
    pub name: String,
    pub size: u64,
    pub transferred: u64,
    pub bytes_per_second: f64,
    pub phase: String,
    pub error: Option<String>,
    pub path: Option<String>,
    pub sha256: Option<String>,
}
struct State {
    view: TransferView,
    decision: Option<Option<PathBuf>>,
    started: Option<Instant>,
}
struct Entry {
    state: Mutex<State>,
    cancel: AtomicBool,
    socket: Mutex<Option<TcpStream>>,
}
impl Entry {
    fn phase(&self, phase: &str) {
        let mut state = lock(&self.state);
        if !terminal(&state.view.phase) {
            state.view.phase = phase.into();
        }
    }
    fn check(&self) -> io::Result<()> {
        if self.cancel.load(Ordering::Acquire) {
            Err(error("传输已取消"))
        } else {
            Ok(())
        }
    }
    fn socket(&self, stream: &TcpStream) -> io::Result<()> {
        let mut slot = lock(&self.socket);
        self.check()?;
        *slot = Some(stream.try_clone()?);
        Ok(())
    }
    fn progress(&self, count: u64) {
        let mut state = lock(&self.state);
        let elapsed = state
            .started
            .get_or_insert_with(Instant::now)
            .elapsed()
            .as_secs_f64();
        state.view.transferred = count;
        state.view.bytes_per_second = count as f64 / elapsed.max(0.001);
    }
    fn begin(&self) {
        let mut state = lock(&self.state);
        state.started = Some(Instant::now());
        if !terminal(&state.view.phase) {
            state.view.phase = "transferring".into();
        }
    }
    fn finish(&self, result: io::Result<()>) {
        if let Err(e) = result {
            let mut state = lock(&self.state);
            if !terminal(&state.view.phase) {
                state.view.phase = if self.cancel.load(Ordering::Acquire) {
                    "cancelled"
                } else {
                    "failed"
                }
                .into();
                state.view.error = Some(e.to_string());
            }
        }
        if let Some(socket) = lock(&self.socket).take() {
            let _ = socket.shutdown(Shutdown::Both);
        }
    }
}

pub struct Transfers {
    local: Device,
    entries: Mutex<Vec<Arc<Entry>>>,
    workers: Mutex<Vec<thread::JoinHandle<()>>>,
    closing: AtomicBool,
}
impl Transfers {
    pub fn new(local: Device) -> Self {
        Self {
            local,
            entries: Mutex::new(vec![]),
            workers: Mutex::new(vec![]),
            closing: AtomicBool::new(false),
        }
    }
    pub fn snapshot(&self) -> Vec<TransferView> {
        lock(&self.entries)
            .iter()
            .rev()
            .map(|entry| lock(&entry.state).view.clone())
            .collect()
    }
    fn entry(&self, id: &str) -> io::Result<Arc<Entry>> {
        lock(&self.entries)
            .iter()
            .find(|e| lock(&e.state).view.id == id)
            .cloned()
            .ok_or_else(|| error("传输记录已不存在"))
    }
    fn add(
        &self,
        direction: &str,
        peer: Device,
        address: SocketAddr,
        name: String,
        size: u64,
    ) -> io::Result<Arc<Entry>> {
        validate_name(&name)?;
        if size > MAX_FILE_BYTES {
            return Err(error("单文件不能超过 1 TiB"));
        }
        let mut entries = lock(&self.entries);
        if self.closing.load(Ordering::Acquire) {
            return Err(error("应用正在退出"));
        }
        if entries
            .iter()
            .filter(|e| !terminal(&lock(&e.state).view.phase))
            .count()
            >= MAX_ACTIVE
        {
            return Err(error("最多同时处理 4 个文件，请等待或取消已有传输"));
        }
        while entries.len() >= HISTORY {
            if let Some(index) = entries
                .iter()
                .position(|e| terminal(&lock(&e.state).view.phase))
            {
                entries.remove(index);
            } else {
                break;
            }
        }
        let entry = Arc::new(Entry {
            state: Mutex::new(State {
                view: TransferView {
                    id: uuid::Uuid::new_v4().to_string(),
                    direction: direction.into(),
                    peer: peer.name,
                    peer_os: peer.os,
                    transport: crate::consent::transport(address).into(),
                    address: address.to_string(),
                    name,
                    size,
                    transferred: 0,
                    bytes_per_second: 0.0,
                    phase: if direction == "incoming" {
                        "pending"
                    } else {
                        "waiting"
                    }
                    .into(),
                    error: None,
                    path: None,
                    sha256: None,
                },
                decision: None,
                started: None,
            }),
            cancel: AtomicBool::new(false),
            socket: Mutex::new(None),
        });
        entries.push(Arc::clone(&entry));
        Ok(entry)
    }
    pub fn pending_name(&self, id: &str) -> io::Result<String> {
        let entry = self.entry(id)?;
        let state = lock(&entry.state);
        if state.view.phase != "pending" || state.decision.is_some() {
            return Err(error("该请求已处理或过期"));
        }
        Ok(state.view.name.clone())
    }
    /// None rejects. A path is supplied exclusively by a local save dialog.
    pub fn decide(&self, id: &str, path: Option<PathBuf>) -> io::Result<()> {
        if let Some(path) = &path {
            validate_destination(path)?;
        }
        let entry = self.entry(id)?;
        let mut state = lock(&entry.state);
        entry.check()?;
        if state.view.phase != "pending" || state.decision.is_some() {
            return Err(error("该请求已处理或过期"));
        }
        state.decision = Some(path);
        Ok(())
    }
    pub fn cancel(&self, id: &str) -> io::Result<()> {
        let entry = self.entry(id)?;
        let mut state = lock(&entry.state);
        if terminal(&state.view.phase) {
            return Ok(());
        }
        entry.cancel.store(true, Ordering::Release);
        state.view.phase = "cancelled".into();
        drop(state);
        if let Some(socket) = lock(&entry.socket).as_ref() {
            let _ = socket.shutdown(Shutdown::Both);
        }
        Ok(())
    }
    pub fn shutdown(&self) {
        self.closing.store(true, Ordering::Release);
        for view in self.snapshot() {
            let _ = self.cancel(&view.id);
        }
    }
    pub fn join(&self) {
        self.shutdown();
        for job in lock(&self.workers).drain(..) {
            let _ = job.join();
        }
    }
    pub fn send(
        self: &Arc<Self>,
        address: SocketAddr,
        peer: Device,
        path: PathBuf,
    ) -> io::Result<String> {
        let file = File::open(&path)?;
        let metadata = file.metadata()?;
        if !metadata.is_file() {
            return Err(error("只支持普通单文件，暂不支持文件夹"));
        }
        let name = path
            .file_name()
            .and_then(|s| s.to_str())
            .ok_or_else(|| error("文件名必须为 UTF-8"))?
            .to_owned();
        let mut workers = lock(&self.workers);
        let mut index = 0;
        while index < workers.len() {
            if workers[index].is_finished() {
                let _ = workers.swap_remove(index).join();
            } else {
                index += 1;
            }
        }
        let entry = self.add("outgoing", peer.clone(), address, name, metadata.len())?;
        let id = lock(&entry.state).view.id.clone();
        let local = self.local.clone();
        let job_entry = Arc::clone(&entry);
        match thread::Builder::new()
            .name("file-send".into())
            .spawn(move || {
                let result = send_file(&job_entry, address, local, peer, file);
                job_entry.finish(result);
            }) {
            Ok(worker) => workers.push(worker),
            Err(e) => {
                entry.finish(Err(error(e.to_string())));
                return Err(e);
            }
        }
        Ok(id)
    }
    /// Invoked by the bounded listener worker on a dedicated connection.
    pub fn receive(
        &self,
        mut stream: TcpStream,
        version: u16,
        peer: Device,
        name: String,
        size: u64,
    ) -> io::Result<()> {
        configure(&stream)?;
        peer.validate()?;
        if version != FILE_VERSION {
            return Err(error("文件协议不兼容，请同时更新两端"));
        }
        let entry = match self.add("incoming", peer, stream.peer_addr()?, name, size) {
            Ok(entry) => entry,
            Err(e) => {
                let _ = write_wire(
                    &mut stream,
                    &Wire::Failure {
                        message: e.to_string(),
                    },
                );
                return Err(e);
            }
        };
        let result = (|| {
            entry.socket(&stream)?;
            receive_file(&entry, &mut stream, &self.local)
        })();
        if let Err(e) = &result {
            let _ = write_wire(
                &mut stream,
                &Wire::Failure {
                    message: e.to_string().chars().take(512).collect(),
                },
            );
        }
        entry.finish(result);
        Ok(())
    }
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
enum Wire {
    Pending { device: Device },
    Accept {},
    Reject {},
    Finish { sha256: String },
    Complete { sha256: String },
    Failure { message: String },
}
fn configure(stream: &TcpStream) -> io::Result<()> {
    stream.set_nodelay(true)?;
    stream.set_read_timeout(Some(IO_TIMEOUT))?;
    stream.set_write_timeout(Some(IO_TIMEOUT))
}
fn write_wire(stream: &mut TcpStream, message: &Wire) -> io::Result<()> {
    let body = serde_json::to_vec(message).map_err(|e| error(e.to_string()))?;
    if body.len() > 8192 {
        return Err(invalid_data("文件控制消息过长"));
    }
    stream.write_all(&(body.len() as u32).to_be_bytes())?;
    stream.write_all(&body)
}
fn read_wire(stream: &mut TcpStream) -> io::Result<Wire> {
    // Total frame deadline prevents a peer drip-feeding a JSON frame forever.
    struct Reader<'a> {
        stream: &'a mut TcpStream,
        until: Instant,
    }
    impl Read for Reader<'_> {
        fn read(&mut self, b: &mut [u8]) -> io::Result<usize> {
            let left = self.until.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return Err(error("文件控制消息超时"));
            }
            self.stream.set_read_timeout(Some(left))?;
            self.stream.read(b)
        }
    }
    let mut reader = Reader {
        stream,
        until: Instant::now() + IO_TIMEOUT,
    };
    let mut header = [0; 4];
    reader.read_exact(&mut header)?;
    let length = u32::from_be_bytes(header) as usize;
    if length == 0 || length > 8192 {
        return Err(invalid_data("文件控制消息长度非法"));
    }
    let mut body = vec![0; length];
    reader.read_exact(&mut body)?;
    serde_json::from_slice(&body).map_err(|e| invalid_data(e.to_string()))
}
fn send_file(
    entry: &Entry,
    address: SocketAddr,
    local: Device,
    peer: Device,
    mut file: File,
) -> io::Result<()> {
    entry.check()?;
    let mut stream = TcpStream::connect_timeout(&address, Duration::from_secs(3))?;
    configure(&stream)?;
    entry.socket(&stream)?;
    let view = lock(&entry.state).view.clone();
    let modified = file.metadata()?.modified().ok();
    write_message(
        &mut stream,
        &Message::FileOffer {
            version: FILE_VERSION,
            device: local,
            name: view.name,
            size: view.size,
        },
    )?;
    let deadline = Instant::now() + WAIT;
    let mut confirmed = false;
    loop {
        entry.check()?;
        if Instant::now() >= deadline {
            return Err(error("等待接收确认超时"));
        }
        match read_wire(&mut stream)? {
            Wire::Pending { device } if device == peer => confirmed = true,
            Wire::Accept {} if confirmed => break,
            Wire::Reject {} => {
                entry.phase("rejected");
                return Ok(());
            }
            Wire::Failure { message } => return Err(error(format!("对方拒绝传输：{message}"))),
            _ => return Err(error("目标信息或文件协议不匹配，请更新两端并重新连接")),
        }
    }
    entry.begin();
    let mut buffer = vec![0; CHUNK_BYTES];
    let mut hash = Sha256::new();
    let mut sent = 0;
    while sent < view.size {
        entry.check()?;
        let length = (view.size - sent).min(CHUNK_BYTES as u64) as usize;
        file.read_exact(&mut buffer[..length])?;
        hash.update(&buffer[..length]);
        stream.write_all(&(length as u32).to_be_bytes())?;
        stream.write_all(&buffer[..length])?;
        sent += length as u64;
        entry.progress(sent);
    }
    let mut extra = [0];
    if file.read(&mut extra)? != 0
        || file.metadata()?.len() != view.size
        || file.metadata()?.modified().ok() != modified
    {
        return Err(error("源文件在传输期间发生变化，请停止修改后重试"));
    }
    entry.check()?;
    entry.phase("verifying");
    let digest = format!("{:x}", hash.finalize());
    stream.write_all(&0_u32.to_be_bytes())?;
    write_wire(
        &mut stream,
        &Wire::Finish {
            sha256: digest.clone(),
        },
    )?;
    match read_wire(&mut stream)? {
        Wire::Complete { sha256 } if sha256 == digest => {
            let mut state = lock(&entry.state);
            entry.check()?;
            state.view.phase = "complete".into();
            state.view.sha256 = Some(digest);
            Ok(())
        }
        Wire::Failure { message } => Err(error(format!("接收失败：{message}"))),
        _ => Err(error("未收到有效校验确认；请核对对方传输记录")),
    }
}

fn validate_destination(path: &Path) -> io::Result<()> {
    if !path.is_absolute() || path.file_name().is_none() || !path.parent().is_some_and(Path::is_dir)
    {
        return Err(error("请选择现有目录中的完整文件路径"));
    }
    match fs::symlink_metadata(path) {
        Ok(_) => Err(error("目标文件已存在，请选择新名称；不会覆盖已有文件")),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
    }
}
struct PartialFile {
    path: PathBuf,
    file: Option<File>,
}
impl PartialFile {
    fn create(destination: &Path) -> io::Result<Self> {
        validate_destination(destination)?;
        let path = destination
            .parent()
            .unwrap()
            .join(format!(".pdl-{}.part", uuid::Uuid::new_v4()));
        let file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&path)?;
        Ok(Self {
            path,
            file: Some(file),
        })
    }
    fn sync(&mut self) -> io::Result<()> {
        self.file.as_ref().unwrap().sync_all()?;
        drop(self.file.take());
        Ok(())
    }
    fn commit(&self, destination: &Path) -> io::Result<()> {
        // Atomic no-clobber publication on NTFS/APFS. Never rename over a file
        // that appeared while receiving. The temporary file is on the same volume.
        fs::hard_link(&self.path, destination).map_err(|e| {
            error(format!(
                "保存失败（目录需支持硬链接，且目标不能已存在）：{e}"
            ))
        })
    }
}
impl Drop for PartialFile {
    fn drop(&mut self) {
        drop(self.file.take());
        let _ = fs::remove_file(&self.path);
    }
}
fn receive_file(entry: &Entry, stream: &mut TcpStream, local: &Device) -> io::Result<()> {
    let until = Instant::now() + WAIT;
    let mut heartbeat = Instant::now() - Duration::from_secs(2);
    let destination = loop {
        entry.check()?;
        if Instant::now() >= until {
            return Err(error("接收确认已超时"));
        }
        if heartbeat.elapsed() >= Duration::from_secs(1) {
            write_wire(
                stream,
                &Wire::Pending {
                    device: local.clone(),
                },
            )?;
            heartbeat = Instant::now();
        }
        let decision = {
            let mut state = lock(&entry.state);
            let decision = state.decision.take();
            if decision.is_some() && !terminal(&state.view.phase) {
                state.view.phase = "preparing".into();
            }
            decision
        };
        if let Some(decision) = decision {
            match decision {
                Some(path) => break path,
                None => {
                    entry.phase("rejected");
                    write_wire(stream, &Wire::Reject {})?;
                    return Ok(());
                }
            }
        }
        thread::sleep(Duration::from_millis(40));
    };
    let mut output = PartialFile::create(&destination)?;
    entry.check()?;
    entry.begin();
    lock(&entry.state).view.path = Some(destination.to_string_lossy().into_owned());
    write_wire(stream, &Wire::Accept {})?;
    configure(stream)?;
    let size = lock(&entry.state).view.size;
    let mut buffer = vec![0; CHUNK_BYTES];
    let mut hash = Sha256::new();
    let mut received = 0;
    loop {
        entry.check()?;
        let mut header = [0; 4];
        stream.read_exact(&mut header)?;
        let length = u32::from_be_bytes(header) as usize;
        if length == 0 {
            break;
        }
        if length > CHUNK_BYTES || length as u64 > size.saturating_sub(received) {
            return Err(invalid_data("文件数据块超出声明大小"));
        }
        stream.read_exact(&mut buffer[..length])?;
        entry.check()?;
        output.file.as_mut().unwrap().write_all(&buffer[..length])?;
        hash.update(&buffer[..length]);
        received += length as u64;
        entry.progress(received);
    }
    if received != size {
        return Err(invalid_data("文件不完整"));
    }
    entry.phase("verifying");
    let digest = format!("{:x}", hash.finalize());
    match read_wire(stream)? {
        Wire::Finish { sha256 } if sha256 == digest => {}
        _ => return Err(invalid_data("SHA-256 校验失败，未保存文件")),
    }
    // Slow disk flushing must not hold the snapshot/cancel lock used by the UI.
    entry.check()?;
    output.sync()?;
    {
        let mut state = lock(&entry.state);
        entry.check()?;
        output.commit(&destination)?;
        state.view.sha256 = Some(digest.clone());
        state.view.phase = "complete".into();
    }
    write_wire(stream, &Wire::Complete { sha256: digest })
}
