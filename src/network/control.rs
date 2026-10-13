use crate::input::queue::{InputEvent, InputQueue};
use crate::input::session::InputSink;
use std::{
    io,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};

pub const CONTROL_TIMEOUT: Duration = Duration::from_secs(2);
pub const CONTROL_HEARTBEAT: Duration = Duration::from_millis(500);
pub type SinkFactory = dyn Fn() -> io::Result<Box<dyn InputSink>> + Send + Sync;

/// A fresh receiver token plus an exclusive lease. Never published through mDNS.
pub struct ControlAccess {
    authorization: Mutex<Option<(String, Arc<AtomicBool>)>>,
    occupied: Arc<AtomicBool>,
    factory: Box<SinkFactory>,
    gate: Option<Arc<AtomicBool>>,
    consent: Option<Arc<crate::consent::Consent>>,
}

pub struct ControlLease(
    Arc<AtomicBool>,
    Option<Arc<AtomicBool>>,
    Option<Arc<AtomicBool>>,
);
impl ControlLease {
    pub fn is_valid(&self) -> bool {
        self.2
            .as_ref()
            .is_none_or(|enabled| enabled.load(Ordering::Acquire))
    }
    pub fn reserve(flag: Arc<AtomicBool>) -> io::Result<Self> {
        flag.compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .map_err(|_| io::Error::other("本机正在控制或被控制，请先停止当前控制再切换方向"))?;
        Ok(Self(flag, None, None))
    }
}
impl Drop for ControlLease {
    fn drop(&mut self) {
        self.0.store(false, Ordering::Release);
        if let Some(gate) = &self.1 {
            gate.store(false, Ordering::Release);
        }
    }
}

impl ControlAccess {
    pub fn with_consent(mut self, consent: Arc<crate::consent::Consent>) -> Self {
        self.consent = Some(consent);
        self
    }
    pub fn approve_update(
        &self,
        stream: &std::net::TcpStream,
        peer: &super::protocol::Device,
        options: crate::input::options::ControlOptions,
        lease: &ControlLease,
    ) -> io::Result<()> {
        if let Some(consent) = &self.consent {
            consent.request(stream, peer, "control_update", Some(options), || {
                lease.is_valid()
            })?;
        }
        Ok(())
    }
    pub fn acquire_request(
        &self,
        stream: &std::net::TcpStream,
        peer: &super::protocol::Device,
        token: &str,
        options: crate::input::options::ControlOptions,
    ) -> io::Result<(ControlLease, Box<dyn InputSink>)> {
        if let Some(consent) = &self.consent {
            let (token, enabled) = self
                .authorization
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .clone()
                .ok_or_else(|| io::Error::other("对方已关闭控制请求"))?;
            if self.is_active() {
                return Err(io::Error::other("对方正在被控制"));
            }
            consent.request(stream, peer, "control", Some(options), || {
                enabled.load(Ordering::Acquire)
            })?;
            // Recheck the exact authorization generation after the user responds.
            if !enabled.load(Ordering::Acquire) {
                return Err(io::Error::other("控制请求已取消"));
            }
            self.acquire_authorized(&token)
        } else {
            self.acquire(token)
        }
    }
    pub fn with_gate(mut self, gate: Arc<AtomicBool>) -> Self {
        self.gate = Some(gate);
        self
    }
    pub fn is_active(&self) -> bool {
        self.occupied.load(Ordering::Acquire)
    }
    pub fn new(token: String, factory: Box<SinkFactory>) -> io::Result<Self> {
        let access = Self::disabled(factory);
        access.enable(token)?;
        Ok(access)
    }
    pub fn disabled(factory: Box<SinkFactory>) -> Self {
        Self {
            authorization: Mutex::new(None),
            occupied: Arc::new(AtomicBool::new(false)),
            factory,
            gate: None,
            consent: None,
        }
    }
    pub fn enable(&self, token: String) -> io::Result<()> {
        if token.len() < 32 || token.len() > 128 || !token.is_ascii() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "invalid control token",
            ));
        }
        let mut authorization = self.authorization.lock().unwrap_or_else(|e| e.into_inner());
        if authorization.is_some() || self.is_active() {
            return Err(io::Error::other("控制接收正在运行或停止"));
        }
        *authorization = Some((token, Arc::new(AtomicBool::new(true))));
        Ok(())
    }
    pub fn disable(&self) {
        if let Some((_, enabled)) = self
            .authorization
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take()
        {
            enabled.store(false, Ordering::Release);
        }
    }

    pub fn acquire(&self, token: &str) -> io::Result<(ControlLease, Box<dyn InputSink>)> {
        if self.consent.is_some() {
            return Err(io::Error::other("必须由接收端确认本次控制请求"));
        }
        self.acquire_authorized(token)
    }
    fn acquire_authorized(&self, token: &str) -> io::Result<(ControlLease, Box<dyn InputSink>)> {
        let authorization = self.authorization.lock().unwrap_or_else(|e| e.into_inner());
        let Some((expected, enabled)) = authorization.as_ref() else {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "对方尚未开启控制接收",
            ));
        };
        if token != expected {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "incorrect control code",
            ));
        }
        self.occupied
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .map_err(|_| {
                io::Error::new(
                    io::ErrorKind::PermissionDenied,
                    "another controller is active",
                )
            })?;
        let mut lease = ControlLease(Arc::clone(&self.occupied), None, Some(Arc::clone(enabled)));
        if let Some(gate) = &self.gate {
            gate.compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
                .map_err(|_| io::Error::other("对方正在主动控制其他设备，请先停止再切换方向"))?;
            lease.1 = Some(Arc::clone(gate));
        }
        let sink = (self.factory)()?;
        Ok((lease, sink))
    }
}

/// Shared by CLI and GUI. Capture has its own main-thread lifetime; this loop
/// only forwards events. The caller stops capture and completes stop_control.
pub fn forward_input(
    client: &mut super::client::Client,
    queue: &InputQueue,
    running: &AtomicBool,
) -> io::Result<()> {
    forward_input_while(client, queue, running, || true)
}

pub fn forward_input_while(
    client: &mut super::client::Client,
    queue: &InputQueue,
    running: &AtomicBool,
    mut keep_running: impl FnMut() -> bool,
) -> io::Result<()> {
    let mut heartbeat = std::time::Instant::now();
    while running.load(Ordering::Acquire) && keep_running() {
        if let Some(event) = queue.receive(Duration::from_millis(10))? {
            if !running.load(Ordering::Acquire) {
                break;
            }
            match event {
                InputEvent::Key(key) => client.send_key(key)?,
                InputEvent::Pointer(pointer) => client.send_pointer(pointer)?,
            }
        }
        if heartbeat.elapsed() >= CONTROL_HEARTBEAT {
            client.heartbeat()?;
            heartbeat = std::time::Instant::now();
        }
    }
    Ok(())
}

/// Bounded sender queue; capture callbacks never block on the network.
pub fn run_input_control(
    mut client: super::client::Client,
    running: Arc<AtomicBool>,
) -> io::Result<()> {
    #[cfg(any(windows, target_os = "macos"))]
    {
        use std::thread;
        let queue = Arc::new(InputQueue::new(512));
        let receiver = Arc::clone(&queue);
        let worker_running = Arc::clone(&running);
        let config = crate::input::options::CaptureConfig {
            options: crate::input::options::ControlOptions::ALL,
            target_os: client.peer.os.clone(),
        };
        let worker = thread::spawn(move || {
            let result = forward_input(&mut client, &receiver, &worker_running);
            // Any worker failure immediately returns input handling to macOS.
            worker_running.store(false, Ordering::Release);
            let cleanup = client.stop_control();
            let disconnect = client.disconnect();
            result.and(cleanup).and(disconnect)
        });
        #[cfg(target_os = "macos")]
        let capture_result = crate::platform::macos::input_capture::run_with_config(
            queue,
            Arc::clone(&running),
            config,
        );
        #[cfg(windows)]
        let capture_result = (|| {
            let capture = crate::platform::windows::input_capture::Capture::start(
                queue,
                Arc::clone(&running),
                config,
            )?;
            println!("Keyboard and pointer active. Emergency stop: Ctrl + Alt + Esc.");
            while running.load(Ordering::Acquire) {
                thread::sleep(Duration::from_millis(20));
            }
            capture.finish()
        })();
        running.store(false, Ordering::Release);
        let network_result = worker
            .join()
            .map_err(|_| io::Error::other("input sender thread panicked"))?;
        capture_result.and(network_result)
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        let _ = (&mut client, running);
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "input capture requires macOS",
        ))
    }
}
