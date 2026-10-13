mod cli;

use cli::{Command, Target, HELP};
use personal_device_link::{
    discovery::mdns::{Discovery, NearbyDevice},
    network::{
        client::Client, control::ControlAccess, protocol::Device, server, HEARTBEAT_INTERVAL,
    },
};
use std::{
    env, io,
    net::TcpListener,
    process::ExitCode,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    thread,
    time::{Duration, Instant},
};

fn print_peer(peer: &NearbyDevice) {
    let addresses = peer
        .addresses
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>()
        .join(", ");
    println!(
        "{} | {} | {} | {}\n  ID: {}",
        if peer.online { "Online" } else { "Offline" },
        peer.device.name,
        peer.device.os,
        addresses,
        peer.id
    );
}

fn scan(
    discovery: &mut Discovery,
    duration: Duration,
    running: &AtomicBool,
    print: bool,
) -> io::Result<()> {
    let deadline = Instant::now() + duration;
    while running.load(Ordering::Acquire) && Instant::now() < deadline {
        if let Some(peer) = discovery.poll(Duration::from_millis(100))? {
            if print {
                print_peer(&peer);
            }
        }
    }
    Ok(())
}

fn interrupted() -> io::Error {
    io::Error::new(io::ErrorKind::Interrupted, "cancelled")
}

fn connect(
    target: Target,
    device: Device,
    seconds: u64,
    running: &AtomicBool,
) -> io::Result<Client> {
    match target {
        Target::Address(address) => Client::connect(address, device),
        Target::Device(selector) => {
            println!("Looking for {selector:?} ({seconds} seconds)...");
            let mut discovery = Discovery::start(None)?;
            scan(&mut discovery, Duration::from_secs(seconds), running, false)?;
            if !running.load(Ordering::Acquire) {
                return Err(interrupted());
            }
            let peer = discovery.table().select(&selector)?;
            print_peer(&peer);
            drop(discovery);
            let mut failures = Vec::new();
            for address in &peer.addresses {
                if !running.load(Ordering::Acquire) {
                    return Err(interrupted());
                }
                match Client::connect_with_timeout(*address, device.clone(), Duration::from_secs(3))
                {
                    Ok(client) if client.peer == peer.device => return Ok(client),
                    Ok(client) => {
                        let _ = client.disconnect();
                        failures.push(format!("{address}: device metadata changed"));
                    }
                    Err(e) => failures.push(format!("{address}: {e}")),
                }
            }
            Err(io::Error::new(
                io::ErrorKind::ConnectionRefused,
                format!(
                    "discovered device could not be reached: {}",
                    failures.join("; ")
                ),
            ))
        }
    }
}

fn run() -> io::Result<()> {
    let command = cli::parse(env::args().skip(1).collect())?;
    if command == Command::Help {
        println!("{HELP}");
        return Ok(());
    }
    let running = Arc::new(AtomicBool::new(true));
    let signal = Arc::clone(&running);
    ctrlc::set_handler(move || {
        signal.store(false, Ordering::Release);
    })
    .map_err(io::Error::other)?;
    match command {
        Command::Help => unreachable!(),
        Command::Listen {
            bind,
            name,
            no_discovery,
            allow_control,
        } => {
            if allow_control && !cfg!(any(windows, target_os = "macos")) {
                return Err(io::Error::new(
                    io::ErrorKind::Unsupported,
                    "input receiving is not implemented for this OS",
                ));
            }
            let device = Device::local(name)?;
            let listener = TcpListener::bind(bind)?;
            let bound = listener.local_addr()?;
            let control = if allow_control {
                let token = uuid::Uuid::new_v4().simple().to_string();
                println!("Input control code: {token}");
                println!("Enter this code on the controlling device. It changes each time this receiver starts.");
                Some(Arc::new(ControlAccess::new(
                    token,
                    Box::new(personal_device_link::platform::input_sink),
                )?))
            } else {
                None
            };
            println!("{} ({}) listening on {bound}", device.name, device.os);
            let discovery = if no_discovery || bound.ip().is_loopback() || bound.is_ipv6() {
                println!("mDNS disabled for this listener; connect by IP.");
                None
            } else {
                match Discovery::start(Some((&device, bound))) {
                    Ok(discovery) => {
                        println!("mDNS enabled. Other devices can use discover or connect --device {:?}.", device.name);
                        Some(discovery)
                    }
                    Err(e) => {
                        eprintln!("Discovery unavailable: {e}. TCP is still available by IP.");
                        None
                    }
                }
            };
            let worker = discovery.map(|mut discovery| {
                let running = Arc::clone(&running);
                thread::spawn(move || {
                    while running.load(Ordering::Acquire) {
                        match discovery.poll(Duration::from_millis(100)) {
                            Ok(Some(peer)) => print_peer(&peer),
                            Ok(None) => {}
                            Err(e) => {
                                eprintln!("Discovery stopped: {e}. TCP is still available by IP.");
                                break;
                            }
                        }
                    }
                })
            });
            println!("Waiting for Hello. Press Ctrl+C to stop.");
            let result = server::serve_with_control(listener, device, &running, control);
            running.store(false, Ordering::Release);
            if let Some(worker) = worker {
                let _ = worker.join();
            }
            result?;
        }
        Command::Discover { seconds, watch } => {
            let mut discovery = Discovery::start(None)?;
            println!("Nearby Devices — listening for local mDNS announcements...");
            if watch {
                println!("Watching Online/Offline changes. Press Ctrl+C to stop.");
                while running.load(Ordering::Acquire) {
                    if let Some(peer) = discovery.poll(Duration::from_millis(100))? {
                        print_peer(&peer);
                    }
                }
            } else {
                scan(
                    &mut discovery,
                    Duration::from_secs(seconds),
                    &running,
                    false,
                )?;
                let peers = discovery.table().snapshot();
                if peers.is_empty() {
                    println!("No devices found. Start listen on the other device; check same LAN, UDP 5353 and local-network permission.");
                } else {
                    for peer in peers {
                        print_peer(&peer);
                    }
                }
            }
        }
        Command::Connect {
            target,
            name,
            once,
            seconds,
        } => {
            let mut client = connect(target, Device::local(name)?, seconds, &running)?;
            println!(
                "Hello acknowledged by {} ({})",
                client.peer.name, client.peer.os
            );
            while running.load(Ordering::Acquire) {
                let elapsed = client.heartbeat()?;
                println!("Heartbeat OK — {:.2} ms", elapsed.as_secs_f64() * 1000.0);
                if once {
                    break;
                }
                let deadline = Instant::now() + HEARTBEAT_INTERVAL;
                while running.load(Ordering::Acquire) && Instant::now() < deadline {
                    thread::sleep(Duration::from_millis(100));
                }
            }
            client.disconnect()?;
        }
        Command::Control {
            target,
            name,
            seconds,
        } => {
            control_input(target, name, seconds, running)?;
        }
    }
    Ok(())
}

fn control_input(
    target: Target,
    name: String,
    seconds: u64,
    running: Arc<AtomicBool>,
) -> io::Result<()> {
    #[cfg(any(windows, target_os = "macos"))]
    {
        use std::io::Write;
        #[cfg(target_os = "macos")]
        personal_device_link::platform::macos::input_capture::check_permission()?;
        print!("Enter the code shown on the receiver: ");
        io::stdout().flush()?;
        let mut token = String::new();
        io::stdin().read_line(&mut token)?;
        if !running.load(Ordering::Acquire) {
            return Err(interrupted());
        }
        println!("Release all modifier keys and mouse buttons. Starting keyboard + pointer control in 3 seconds...");
        let deadline = Instant::now() + Duration::from_secs(3);
        while running.load(Ordering::Acquire) && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(100));
        }
        if !running.load(Ordering::Acquire) {
            return Err(interrupted());
        }
        let mut client = connect(target, Device::local(name)?, seconds, &running)?;
        client.start_control(token.trim().to_owned())?;
        let result = personal_device_link::network::control::run_input_control(client, running);
        println!("Input control stopped. Keyboard and pointer are local again.");
        result
    }
    #[cfg(not(any(windows, target_os = "macos")))]
    {
        let _ = (target, name, seconds, running);
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "input capture is not implemented for this OS",
        ))
    }
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) if error.kind() == io::ErrorKind::Interrupted => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("Error: {error}\nUse --help for usage.");
            ExitCode::FAILURE
        }
    }
}
