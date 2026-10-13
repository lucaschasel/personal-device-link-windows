//! One-shot desktop approvals. No native device access takes place here.
use crate::{clipboard::lock, input::options::ControlOptions, network::protocol::Device};
use serde::Serialize;
use std::{
    io,
    net::{SocketAddr, TcpStream},
    sync::Mutex,
    time::{Duration, Instant},
};

pub const WAIT_TIMEOUT: Duration = Duration::from_secs(70);
const REQUEST_TIMEOUT: Duration = Duration::from_secs(60);

pub fn transport(address: SocketAddr) -> &'static str {
    // The RFCOMM bridge connects locally; loopback alone cannot prove Bluetooth.
    if address.ip().is_loopback() {
        "蓝牙 / 本机桥接"
    } else {
        "局域网 / IP 直连"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn expired_request_is_removed_and_cannot_be_approved() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let _client = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        let stream = listener.accept().unwrap().0;
        let consent = Consent::default();
        let peer = Device {
            name: "timeout".into(),
            os: "windows".into(),
        };
        let error = consent
            .request_for(
                &stream,
                &peer,
                "control",
                Some(ControlOptions::ALL),
                || true,
                Duration::from_millis(1),
            )
            .unwrap_err();
        assert!(error.to_string().contains("过期"));
        assert!(consent.snapshot().is_empty());
        assert!(consent.decide("expired", true).is_err());
    }
}

#[derive(Clone, Serialize)]
pub struct Request {
    pub id: String,
    pub device: Device,
    pub address: String,
    pub transport: String,
    pub kind: String,
    pub options: Option<ControlOptions>,
}
struct Pending {
    request: Request,
    expires: Instant,
    decision: Option<bool>,
}
#[derive(Default)]
struct State {
    pending: Vec<Pending>,
    closing: bool,
}
#[derive(Default)]
pub struct Consent {
    state: Mutex<State>,
}
impl Consent {
    pub fn snapshot(&self) -> Vec<Request> {
        lock(&self.state)
            .pending
            .iter()
            .filter(|p| p.decision.is_none() && Instant::now() < p.expires)
            .map(|p| p.request.clone())
            .collect()
    }
    pub fn decide(&self, id: &str, allow: bool) -> io::Result<()> {
        let mut state = lock(&self.state);
        if state.closing {
            return Err(io::Error::other("应用正在退出"));
        }
        let pending = state
            .pending
            .iter_mut()
            .find(|p| p.request.id == id && p.decision.is_none() && Instant::now() < p.expires)
            .ok_or_else(|| io::Error::other("请求已取消或过期"))?;
        pending.decision = Some(allow);
        Ok(())
    }
    pub fn shutdown(&self) {
        lock(&self.state).closing = true;
    }
    pub fn request(
        &self,
        stream: &TcpStream,
        device: &Device,
        kind: &str,
        options: Option<ControlOptions>,
        valid: impl Fn() -> bool,
    ) -> io::Result<()> {
        self.request_for(stream, device, kind, options, valid, REQUEST_TIMEOUT)
    }
    fn request_for(
        &self,
        stream: &TcpStream,
        device: &Device,
        kind: &str,
        options: Option<ControlOptions>,
        valid: impl Fn() -> bool,
        timeout: Duration,
    ) -> io::Result<()> {
        device.validate()?;
        let address = stream.peer_addr()?;
        let id = uuid::Uuid::new_v4().to_string();
        let old_timeout = stream.read_timeout()?;
        stream.set_read_timeout(Some(Duration::from_millis(100)))?;
        let result = (|| {
            {
                let mut state = lock(&self.state);
                if state.closing || state.pending.len() >= 8 {
                    return Err(io::Error::other("暂时无法接受更多授权请求"));
                }
                state.pending.push(Pending {
                    request: Request {
                        id: id.clone(),
                        device: device.clone(),
                        address: address.to_string(),
                        transport: transport(address).into(),
                        kind: kind.into(),
                        options,
                    },
                    expires: Instant::now() + timeout,
                    decision: None,
                });
            }
            loop {
                // Detect cancellation and reject pipelined input before honoring a decision.
                match stream.peek(&mut [0]) {
                    Ok(0) => return Err(io::Error::other("发起方已断开")),
                    Ok(_) => return Err(io::Error::other("授权前收到额外数据，请重新请求")),
                    Err(e)
                        if matches!(
                            e.kind(),
                            io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                        ) => {}
                    Err(e) => return Err(e),
                }
                if !valid() {
                    return Err(io::Error::other("请求已取消"));
                }
                let state = lock(&self.state);
                if state.closing {
                    return Err(io::Error::other("应用正在退出"));
                }
                let pending = state
                    .pending
                    .iter()
                    .find(|p| p.request.id == id)
                    .ok_or_else(|| io::Error::other("请求已取消"))?;
                if Instant::now() >= pending.expires {
                    return Err(io::Error::other("对方未在 60 秒内确认，请求已过期"));
                }
                match pending.decision {
                    Some(true) => return Ok(()),
                    Some(false) => return Err(io::Error::other("对方拒绝了本次请求")),
                    None => (),
                }
            }
        })();
        lock(&self.state).pending.retain(|p| p.request.id != id);
        let reset = stream.set_read_timeout(old_timeout);
        result.and(reset)
    }
}
