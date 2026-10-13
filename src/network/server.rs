use super::{
    configure_stream,
    control::{ControlAccess, CONTROL_TIMEOUT},
    protocol::*,
};
use crate::input::{keyboard::KeyEvent, session::InputSession};
use std::{
    collections::BTreeMap,
    io,
    net::{Shutdown, TcpListener, TcpStream},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    thread,
    time::Duration,
};

const MAX_CONNECTIONS: usize = 16;
type Connections = Arc<Mutex<BTreeMap<u64, TcpStream>>>;

struct ConnectionSlot {
    id: u64,
    connections: Connections,
}
impl Drop for ConnectionSlot {
    fn drop(&mut self) {
        self.connections
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&self.id);
    }
}

pub fn serve(listener: TcpListener, device: Device) -> io::Result<()> {
    serve_until(listener, device, &AtomicBool::new(true))
}

pub fn serve_until(listener: TcpListener, device: Device, running: &AtomicBool) -> io::Result<()> {
    serve_with_control(listener, device, running, None)
}

/// Shutdown closes sockets and joins every session before returning, so its
/// InputSession destructor can release keys before the process exits.
pub fn serve_with_control(
    listener: TcpListener,
    device: Device,
    running: &AtomicBool,
    control: Option<Arc<ControlAccess>>,
) -> io::Result<()> {
    serve_with_transfers(listener, device, running, control, None)
}

pub fn serve_with_transfers(
    listener: TcpListener,
    device: Device,
    running: &AtomicBool,
    control: Option<Arc<ControlAccess>>,
    transfers: Option<Arc<crate::transfer::Transfers>>,
) -> io::Result<()> {
    serve_with_services(listener, device, running, control, transfers, None)
}
pub fn serve_with_services(
    listener: TcpListener,
    device: Device,
    running: &AtomicBool,
    control: Option<Arc<ControlAccess>>,
    transfers: Option<Arc<crate::transfer::Transfers>>,
    clipboard: Option<Arc<crate::clipboard::Clipboard>>,
) -> io::Result<()> {
    device.validate()?;
    listener.set_nonblocking(true)?;
    let connections: Connections = Arc::new(Mutex::new(BTreeMap::new()));
    let mut workers: Vec<thread::JoinHandle<()>> = Vec::new();
    let mut id = 0_u64;
    let result = (|| {
        while running.load(Ordering::Acquire) {
            let mut index = 0;
            while index < workers.len() {
                if workers[index].is_finished() {
                    let _ = workers.swap_remove(index).join();
                } else {
                    index += 1;
                }
            }
            let (stream, peer) = match listener.accept() {
                Ok(connection) => connection,
                Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                    thread::sleep(Duration::from_millis(20));
                    continue;
                }
                Err(e) => return Err(e),
            };
            stream.set_nonblocking(false)?;
            let mut active = connections.lock().unwrap_or_else(|e| e.into_inner());
            if active.len() >= MAX_CONNECTIONS {
                eprintln!("Connection limit reached; closing new connection");
                continue;
            }
            id = id.wrapping_add(1);
            active.insert(id, stream.try_clone()?);
            drop(active);
            let slot = ConnectionSlot {
                id,
                connections: Arc::clone(&connections),
            };
            let local = device.clone();
            let access = control.clone();
            let transfers = transfers.clone();
            let clipboard = clipboard.clone();
            match thread::Builder::new()
                .name(format!("peer-{peer}"))
                .spawn(move || {
                    let _slot = slot;
                    if let Err(e) = handle_with_transfers(
                        stream,
                        &local,
                        access.as_deref(),
                        transfers.as_deref(),
                        clipboard.as_ref(),
                    ) {
                        eprintln!("[{peer}] disconnected: {e}");
                    }
                }) {
                Ok(worker) => workers.push(worker),
                Err(e) => eprintln!("Could not start connection worker: {e}"),
            }
        }
        Ok(())
    })();
    for stream in connections
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .values()
    {
        let _ = stream.shutdown(Shutdown::Both);
    }
    for worker in workers {
        let _ = worker.join();
    }
    result
}

fn reject(stream: &mut TcpStream, reason: &str) -> io::Result<()> {
    write_message(
        stream,
        &Message::Error {
            message: reason.to_owned(),
        },
    )?;
    Err(invalid_data(reason))
}

pub fn handle_connection(stream: TcpStream, local: &Device) -> io::Result<()> {
    handle_with_control(stream, local, None)
}

pub fn handle_with_control(
    stream: TcpStream,
    local: &Device,
    access: Option<&ControlAccess>,
) -> io::Result<()> {
    handle_with_transfers(stream, local, access, None, None)
}

fn handle_with_transfers(
    mut stream: TcpStream,
    local: &Device,
    access: Option<&ControlAccess>,
    transfers: Option<&crate::transfer::Transfers>,
    clipboard: Option<&Arc<crate::clipboard::Clipboard>>,
) -> io::Result<()> {
    configure_stream(&stream)?;
    let peer = stream.peer_addr()?;
    let remote = match super::read_with_deadline(&mut stream, super::IO_TIMEOUT)? {
        Some(Message::ClipboardHello {
            version,
            token,
            device,
        }) => {
            return match clipboard {
                Some(clipboard) => clipboard.serve(stream, version, token, device, local),
                None => reject(&mut stream, "对方版本不支持剪贴板同步"),
            };
        }
        Some(Message::FileOffer {
            version,
            device,
            name,
            size,
        }) => {
            return match transfers {
                Some(transfers) => transfers.receive(stream, version, device, name, size),
                None => reject(
                    &mut stream,
                    "file transfer requires the updated desktop app",
                ),
            };
        }
        Some(Message::Hello {
            version,
            device,
            text,
        }) => {
            if version != PROTOCOL_VERSION {
                return reject(&mut stream, "unsupported protocol version");
            }
            if device.validate().is_err() || text != "Hello" {
                return reject(&mut stream, "invalid hello payload");
            }
            println!("[{peer}] Hello from {} ({})", device.name, device.os);
            write_message(
                &mut stream,
                &Message::HelloAck {
                    version: PROTOCOL_VERSION,
                    device: local.clone(),
                    text: "Hello".to_owned(),
                    capabilities: Capabilities {
                        keyboard: access.is_some(),
                        pointer: access.is_some(),
                        files: transfers.is_some(),
                    },
                },
            )?;
            device
        }
        None => return Ok(()),
        _ => return reject(&mut stream, "first message must be hello"),
    };
    let _connection = clipboard.map(|clipboard| clipboard.connected());
    // Destruction order is intentional: release keys/buttons BEFORE releasing the lease.
    let mut lease: Option<super::control::ControlLease> = None;
    let mut session: Option<InputSession> = None;
    let mut controls = crate::input::options::ControlOptions::default();
    loop {
        let message = if session.is_some() {
            super::read_with_deadline(&mut stream, CONTROL_TIMEOUT)?
        } else {
            read_message(&mut stream)?
        };
        if lease.as_ref().is_some_and(|lease| !lease.is_valid()) {
            // Release input before writing a rejection to a potentially stalled peer.
            drop(session.take());
            drop(lease.take());
            return reject(&mut stream, "对方已停止控制接收");
        }
        match message {
            Some(Message::Ping { sequence }) => {
                write_message(&mut stream, &Message::Pong { sequence })?
            }
            Some(Message::ControlRequest { token, options }) => {
                options.validate()?;
                if session.is_some() {
                    return reject(&mut stream, "control already started");
                }
                let Some(access) = access else {
                    return reject(&mut stream, "receiver has not enabled input control");
                };
                match access.acquire_request(&stream, &remote, &token, options) {
                    Ok((new_lease, sink)) => {
                        lease = Some(new_lease);
                        session = Some(InputSession::new(sink));
                        controls = options;
                        stream.set_write_timeout(Some(CONTROL_TIMEOUT))?;
                        write_message(&mut stream, &Message::ControlStarted {})?;
                        println!("[{peer}] Keyboard and pointer control started");
                    }
                    Err(e) => return reject(&mut stream, &e.to_string()),
                }
            }
            Some(Message::UpdateControl { options }) => {
                let Some(session) = session.as_mut() else {
                    return reject(&mut stream, "control update requires authorized control");
                };
                session.release_all()?;
                if (options.keyboard && !controls.keyboard)
                    || (options.pointer && !controls.pointer)
                    || (options.keyboard && options.shortcut_mapping != controls.shortcut_mapping)
                {
                    if let (Some(access), Some(lease)) = (access, lease.as_ref()) {
                        if let Err(e) = access.approve_update(&stream, &remote, options, lease) {
                            return reject(&mut stream, &e.to_string());
                        }
                    }
                }
                controls = options;
                write_message(&mut stream, &Message::ControlUpdated { options })?;
            }
            Some(Message::Keyboard {
                key,
                pressed,
                repeat,
            }) => {
                if !controls.keyboard {
                    return reject(&mut stream, "keyboard control was not selected");
                }
                let Some(session) = session.as_mut() else {
                    return reject(&mut stream, "keyboard input requires authorized control");
                };
                session.apply(KeyEvent {
                    key,
                    pressed,
                    repeat,
                })?;
            }
            Some(Message::Pointer { event }) => {
                if !controls.pointer {
                    return reject(&mut stream, "pointer control was not selected");
                }
                let Some(session) = session.as_mut() else {
                    return reject(&mut stream, "pointer input requires authorized control");
                };
                session.apply_pointer(event)?;
            }
            Some(Message::StopControl {}) => {
                if let Some(mut session) = session.take() {
                    session.release_all()?;
                }
                drop(lease.take());
                configure_stream(&stream)?;
                write_message(&mut stream, &Message::ControlStopped {})?;
                println!("[{peer}] Input control stopped; keys and buttons released");
            }
            Some(Message::Goodbye { .. }) | None => {
                println!("[{peer}] disconnected");
                return Ok(());
            }
            _ => return reject(&mut stream, "unexpected message after handshake"),
        }
    }
}
