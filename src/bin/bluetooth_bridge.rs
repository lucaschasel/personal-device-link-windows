//! Experimental bidirectional loopback TCP multiplexing over one RFCOMM stream.
use personal_device_link::bluetooth::{parse, Role};
use std::{
    collections::HashMap,
    io::{self, Read, Write},
    net::{Shutdown, TcpListener, TcpStream},
    sync::{
        atomic::{AtomicBool, AtomicU32, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};
type Output = Arc<Mutex<Box<dyn Write + Send>>>;
struct Connection {
    socket: TcpStream,
    local_eof: bool,
    remote_eof: bool,
}
type Clients = Arc<Mutex<HashMap<u32, Connection>>>;
const MAX: usize = 4096;
fn packet(out: &Output, kind: u8, id: u32, data: &[u8]) -> io::Result<()> {
    if data.len() > MAX {
        return Err(io::Error::other("oversized frame"));
    }
    let mut out = out.lock().unwrap();
    out.write_all(&[kind])?;
    out.write_all(&id.to_be_bytes())?;
    out.write_all(&(data.len() as u16).to_be_bytes())?;
    out.write_all(data)?;
    out.flush()
}
fn frame(input: &mut dyn Read) -> io::Result<(u8, u32, Vec<u8>)> {
    let mut h = [0; 7];
    input.read_exact(&mut h)?;
    let n = u16::from_be_bytes([h[5], h[6]]) as usize;
    if n > MAX || !(1..=4).contains(&h[0]) || (h[0] != 2 && n != 0) {
        return Err(io::Error::other("invalid bridge frame"));
    }
    let mut data = vec![0; n];
    input.read_exact(&mut data)?;
    Ok((h[0], u32::from_be_bytes(h[1..5].try_into().unwrap()), data))
}
fn remove(clients: &Clients, id: u32) {
    if let Some(s) = clients.lock().unwrap().remove(&id) {
        let _ = s.socket.shutdown(Shutdown::Both);
    }
}
fn eof(clients: &Clients, id: u32, local: bool) {
    let mut slots = clients.lock().unwrap();
    if let Some(c) = slots.get_mut(&id) {
        if local {
            c.local_eof = true;
        } else {
            c.remote_eof = true;
            let _ = c.socket.shutdown(Shutdown::Write);
        }
        if c.local_eof && c.remote_eof {
            slots.remove(&id);
        }
    }
}
fn pump(mut tcp: TcpStream, id: u32, out: Output, clients: Clients) {
    std::thread::spawn(move || {
        let mut b = [0; MAX];
        loop {
            match tcp.read(&mut b) {
                Ok(0) => {
                    let _ = packet(&out, 3, id, &[]);
                    eof(&clients, id, true);
                    return;
                }
                Ok(n) => {
                    if packet(&out, 2, id, &b[..n]).is_err() {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
        let _ = packet(&out, 4, id, &[]);
        remove(&clients, id);
    });
}
fn add(tcp: &TcpStream, clients: &Clients, id: u32) -> io::Result<()> {
    tcp.set_nonblocking(false)?;
    tcp.set_write_timeout(Some(Duration::from_secs(3)))?;
    let mut slots = clients.lock().unwrap();
    if slots.len() >= 8 || slots.contains_key(&id) {
        return Err(io::Error::other("too many connections or duplicate id"));
    }
    slots.insert(
        id,
        Connection {
            socket: tcp.try_clone()?,
            local_eof: false,
            remote_eof: false,
        },
    );
    Ok(())
}
fn bridge(
    mut input: Box<dyn Read + Send>,
    mut output: Box<dyn Write + Send>,
    odd: bool,
    bind: std::net::SocketAddr,
    target: std::net::SocketAddr,
) -> io::Result<()> {
    output.write_all(b"PDLBT001")?;
    output.flush()?;
    let mut hello = [0; 8];
    input.read_exact(&mut hello)?;
    if &hello != b"PDLBT001" {
        return Err(io::Error::other("not a PDL Bluetooth bridge"));
    }
    let listener = TcpListener::bind(bind)?;
    listener.set_nonblocking(true)?;
    let out: Output = Arc::new(Mutex::new(output));
    let clients: Clients = Arc::new(Mutex::new(HashMap::new()));
    let running = Arc::new(AtomicBool::new(true));
    let ids = AtomicU32::new(if odd { 1 } else { 2 });
    let (o, c, r) = (out.clone(), clients.clone(), running.clone());
    let accept = std::thread::spawn(move || {
        while r.load(Ordering::Acquire) {
            match listener.accept() {
                Ok((tcp, _)) => {
                    let id = ids.fetch_add(2, Ordering::Relaxed);
                    if id == 0 {
                        break;
                    }
                    if add(&tcp, &c, id).is_ok() {
                        if packet(&o, 1, id, &[]).is_ok() {
                            pump(tcp, id, o.clone(), c.clone());
                        } else {
                            remove(&c, id);
                            break;
                        }
                    }
                }
                Err(e) if e.kind() == io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(30))
                }
                Err(_) => break,
            }
        }
    });
    eprintln!(
        "蓝牙桥接已连接。两端应用均可手动连接 127.0.0.1:45833；不要创建 Wi-Fi。Ctrl+C 停止。"
    );
    println!("{}", serde_json::json!({"event":"ready"}));
    let result = (|| -> io::Result<()> {
        loop {
            let (kind, id, data) = frame(input.as_mut())?;
            if id == 0 {
                return Err(io::Error::other("invalid stream id"));
            }
            match kind {
                1 => {
                    if (id % 2 == 1) == odd {
                        return Err(io::Error::other("invalid stream direction"));
                    }
                    match TcpStream::connect_timeout(&target, Duration::from_secs(1)) {
                        Ok(tcp) => {
                            if add(&tcp, &clients, id).is_ok() {
                                pump(tcp, id, out.clone(), clients.clone());
                            } else {
                                packet(&out, 4, id, &[])?;
                            }
                        }
                        Err(_) => {
                            packet(&out, 4, id, &[])?;
                        }
                    }
                }
                2 => {
                    let tcp = clients
                        .lock()
                        .unwrap()
                        .get(&id)
                        .map(|c| c.socket.try_clone())
                        .transpose()?;
                    if let Some(mut tcp) = tcp {
                        if tcp.write_all(&data).is_err() {
                            remove(&clients, id);
                            packet(&out, 4, id, &[])?;
                        }
                    }
                }
                3 => eof(&clients, id, false),
                4 => remove(&clients, id),
                _ => unreachable!(),
            }
        }
    })();
    running.store(false, Ordering::Release);
    for (_, tcp) in clients.lock().unwrap().drain() {
        let _ = tcp.socket.shutdown(Shutdown::Both);
    }
    // A stalled radio write must not prevent process exit.
    drop(accept);
    result
}
fn main() {
    let managed = std::env::args().any(|a| a == "--managed");
    if managed {
        std::thread::spawn(|| {
            let _ = io::stdin().read_to_end(&mut Vec::new());
            std::process::exit(0);
        });
    }
    let args: Vec<_> = std::env::args().filter(|a| a != "--managed").collect();
    if args.len() == 1 || args[1] == "--help" {
        eprintln!("PDL Bluetooth bridge\n等待端（任一系统）: bluetooth_bridge --listen\n发起端（任一系统）: bluetooth_bridge --connect ADDRESS CHANNEL\n先在系统中完成两台电脑的蓝牙配对；双方选择不同角色。");
        return;
    }
    if let Err(e) = run(&args) {
        eprintln!("桥接停止：{e}");
        std::process::exit(1);
    }
}
#[cfg(windows)]
fn run(args: &[String]) -> io::Result<()> {
    use std::os::windows::io::FromRawSocket;
    use windows_sys::Win32::{Devices::Bluetooth::*, Networking::WinSock::*};
    let (role, peer_address) = parse(&args[1..]).map_err(io::Error::other)?;
    unsafe {
        let mut data: WSADATA = std::mem::zeroed();
        let error = WSAStartup(0x202, &mut data);
        if error != 0 {
            return Err(io::Error::from_raw_os_error(error));
        }
        let s = socket(AF_BTH as i32, SOCK_STREAM, BTHPROTO_RFCOMM as i32);
        if s == INVALID_SOCKET {
            let error = WSAGetLastError();
            WSACleanup();
            return Err(io::Error::from_raw_os_error(error));
        }
        let mut socket_consumed = false;
        let result = (|| {
            let yes = 1u32;
            for opt in [SO_BTH_AUTHENTICATE, SO_BTH_ENCRYPT] {
                if setsockopt(
                    s,
                    SOL_RFCOMM as i32,
                    opt as i32,
                    (&yes as *const u32).cast(),
                    4,
                ) == SOCKET_ERROR
                {
                    return Err(io::Error::from_raw_os_error(WSAGetLastError()));
                }
            }
            let mut addr: SOCKADDR_BTH = std::mem::zeroed();
            addr.addressFamily = AF_BTH;
            let peer = if let Some((address, channel)) = peer_address {
                addr.btAddr = u64::from_str_radix(&address, 16).map_err(io::Error::other)?;
                addr.port = u32::from(channel);
                if connect(
                    s,
                    (&addr as *const SOCKADDR_BTH).cast(),
                    std::mem::size_of_val(&addr) as i32,
                ) == SOCKET_ERROR
                {
                    return Err(io::Error::from_raw_os_error(WSAGetLastError()));
                }
                // Duplicate ownership is avoided: the connected socket is consumed below.
                s
            } else {
                let params = BLUETOOTH_FIND_RADIO_PARAMS {
                    dwSize: std::mem::size_of::<BLUETOOTH_FIND_RADIO_PARAMS>() as u32,
                };
                let mut radio = std::ptr::null_mut();
                let find = BluetoothFindFirstRadio(&params, &mut radio);
                if find.is_null() {
                    return Err(io::Error::other("没有可用蓝牙适配器，请先开启蓝牙"));
                }
                let mut info: BLUETOOTH_RADIO_INFO = std::mem::zeroed();
                info.dwSize = std::mem::size_of_val(&info) as u32;
                let code = BluetoothGetRadioInfo(radio, &mut info);
                windows_sys::Win32::Foundation::CloseHandle(radio);
                BluetoothFindRadioClose(find);
                if code != 0 {
                    return Err(io::Error::from_raw_os_error(code as i32));
                }
                addr.btAddr = info.address.Anonymous.ullLong;
                addr.port = u32::MAX;
                let mut size = std::mem::size_of_val(&addr) as i32;
                if bind(s, (&addr as *const SOCKADDR_BTH).cast(), size) == SOCKET_ERROR
                    || getsockname(s, (&mut addr as *mut SOCKADDR_BTH).cast(), &mut size)
                        == SOCKET_ERROR
                    || listen(s, 1) == SOCKET_ERROR
                {
                    return Err(io::Error::from_raw_os_error(WSAGetLastError()));
                }
                let (address, port) = (addr.btAddr, addr.port);
                println!(
                    "{}",
                    serde_json::json!({"event":"listening","address":format!("{address:012X}"),"channel":port})
                );
                eprintln!("本机蓝牙地址：{address:012X}，频道：{port}。请在对方设备选择主动连接并输入这两个参数。");
                let peer = accept(s, std::ptr::null_mut(), std::ptr::null_mut());
                if peer == INVALID_SOCKET {
                    return Err(io::Error::from_raw_os_error(WSAGetLastError()));
                }
                peer
            };
            println!("{}", serde_json::json!({"event":"negotiating"}));
            socket_consumed = role == Role::Connect;
            let stream = TcpStream::from_raw_socket(peer as _);
            bridge(
                Box::new(stream.try_clone()?),
                Box::new(stream),
                role.odd_stream_ids(),
                "127.0.0.1:45833".parse().unwrap(),
                "127.0.0.1:45832".parse().unwrap(),
            )
        })();
        if !socket_consumed {
            closesocket(s);
        }
        WSACleanup();
        result
    }
}
#[cfg(target_os = "macos")]
fn run(args: &[String]) -> io::Result<()> {
    use std::io::BufReader;
    use std::process::{Command, Stdio};
    let (role, peer) = parse(&args[1..]).map_err(io::Error::other)?;
    let (address, channel) = peer.unwrap_or_default();
    let arguments = role
        .arguments(&address, channel)
        .map_err(io::Error::other)?;
    let helper = std::env::current_exe()?.with_file_name("pdl-rfcomm-mac");
    let mut child = Command::new(helper)
        .args(arguments)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .spawn()?;
    let result = (|| {
        let mut input = BufReader::new(child.stdout.take().unwrap());
        // Native helper status is a bounded JSON line prefix, then only raw bytes.
        // Keep BufReader alive: it may already contain the remote bridge greeting.
        if role == Role::Listen {
            let event = native_status(&mut input, "listening")?;
            let address = event["address"].as_str().unwrap_or_default();
            let channel = event["channel"]
                .as_u64()
                .and_then(|n| u16::try_from(n).ok())
                .unwrap_or(0);
            personal_device_link::bluetooth::address(address, channel).map_err(io::Error::other)?;
            println!("{event}");
        }
        native_status(&mut input, "connected")?;
        println!("{}", serde_json::json!({"event":"negotiating"}));
        bridge(
            Box::new(input),
            Box::new(child.stdin.take().unwrap()),
            role.odd_stream_ids(),
            "127.0.0.1:45833".parse().unwrap(),
            "127.0.0.1:45832".parse().unwrap(),
        )
    })();
    let _ = child.kill();
    let _ = child.wait();
    result
}
#[cfg(any(target_os = "macos", test))]
fn native_status(
    input: &mut impl std::io::BufRead,
    expected: &str,
) -> io::Result<serde_json::Value> {
    use std::io::BufRead;
    let mut line = Vec::new();
    input.take(4096).read_until(b'\n', &mut line)?;
    if line.last() != Some(&b'\n') {
        return Err(io::Error::other(
            "native Bluetooth status missing or too long",
        ));
    }
    let event: serde_json::Value = serde_json::from_slice(&line).map_err(io::Error::other)?;
    if event["event"].as_str() != Some(expected) {
        return Err(io::Error::other("unexpected native Bluetooth status"));
    }
    Ok(event)
}
#[cfg(not(any(windows, target_os = "macos")))]
fn run(_: &[String]) -> io::Result<()> {
    Err(io::Error::other("unsupported platform"))
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn native_status_preserves_prefetched_radio_bytes_and_rejects_invalid_prefix() {
        use std::io::BufReader;
        let bytes = b"{\"event\":\"connected\"}\nPDLBT001";
        let mut input = BufReader::new(&bytes[..]);
        native_status(&mut input, "connected").unwrap();
        let mut hello = [0; 8];
        input.read_exact(&mut hello).unwrap();
        assert_eq!(&hello, b"PDLBT001");
        for bad in [
            vec![b'x'; 5000],
            b"{\"event\":\"ready\"}\n".to_vec(),
            b"{}".to_vec(),
        ] {
            assert!(native_status(&mut BufReader::new(&bad[..]), "connected").is_err());
        }
    }
    #[test]
    fn both_directions_carry_multiple_frames_and_stop_on_wire_disconnect() {
        for role in [Role::Listen, Role::Connect] {
            bidirectional_pair(role);
        }
    }
    fn bidirectional_pair(left_role: Role) {
        fn listener() -> TcpListener {
            TcpListener::bind("127.0.0.1:0").unwrap()
        }
        let service_a = listener();
        let service_b = listener();
        let (target_a, target_b) = (
            service_a.local_addr().unwrap(),
            service_b.local_addr().unwrap(),
        );
        let local_a = listener();
        let local_b = listener();
        let (bind_a, bind_b) = (local_a.local_addr().unwrap(), local_b.local_addr().unwrap());
        drop((local_a, local_b));
        let wire = listener();
        let a = TcpStream::connect(wire.local_addr().unwrap()).unwrap();
        let (b, _) = wire.accept().unwrap();
        let stop = a.try_clone().unwrap();
        let left = std::thread::spawn(move || {
            bridge(
                Box::new(a.try_clone().unwrap()),
                Box::new(a),
                left_role.odd_stream_ids(),
                bind_a,
                target_a,
            )
        });
        let right = std::thread::spawn(move || {
            bridge(
                Box::new(b.try_clone().unwrap()),
                Box::new(b),
                !left_role.odd_stream_ids(),
                bind_b,
                target_b,
            )
        });
        let echoes: Vec<_> = [service_a, service_b]
            .into_iter()
            .map(|server| {
                std::thread::spawn(move || {
                    let (mut s, _) = server.accept().unwrap();
                    s.set_read_timeout(Some(Duration::from_secs(5))).unwrap();
                    let mut bytes = vec![0; MAX * 3 + 17];
                    s.read_exact(&mut bytes).unwrap();
                    s.write_all(&bytes).unwrap();
                })
            })
            .collect();
        let clients: Vec<_> = [bind_a, bind_b]
            .into_iter()
            .map(|addr| {
                std::thread::spawn(move || {
                    let deadline = std::time::Instant::now() + Duration::from_secs(5);
                    let mut stream = loop {
                        if let Ok(s) = TcpStream::connect(addr) {
                            break s;
                        }
                        assert!(std::time::Instant::now() < deadline);
                        std::thread::sleep(Duration::from_millis(10));
                    };
                    stream
                        .set_read_timeout(Some(Duration::from_secs(5)))
                        .unwrap();
                    let bytes: Vec<_> = (0..MAX * 3 + 17).map(|i| (i % 251) as u8).collect();
                    stream.write_all(&bytes).unwrap();
                    stream.shutdown(Shutdown::Write).unwrap();
                    let mut got = vec![0; bytes.len()];
                    stream.read_exact(&mut got).unwrap();
                    assert_eq!(got, bytes);
                })
            })
            .collect();
        for client in clients {
            client.join().unwrap();
        }
        for echo in echoes {
            echo.join().unwrap();
        }
        stop.shutdown(Shutdown::Both).unwrap();
        assert!(left.join().unwrap().is_err());
        assert!(right.join().unwrap().is_err());
    }
    #[test]
    fn rejects_bad_size_and_control_payload() {
        for h in [
            [2, 0, 0, 0, 1, 0xff, 0xff],
            [1, 0, 0, 0, 1, 0, 1],
            [9, 0, 0, 0, 1, 0, 0],
        ] {
            assert!(frame(&mut &h[..]).is_err());
        }
    }
    #[test]
    fn decodes_fragmented_data() {
        let h = [2, 0, 0, 0, 7, 0, 3, 1, 2, 3];
        let (k, id, b) = frame(&mut &h[..]).unwrap();
        assert_eq!((k, id, b), (2, 7, vec![1, 2, 3]));
        assert!(frame(&mut &h[..8]).is_err());
    }
}
