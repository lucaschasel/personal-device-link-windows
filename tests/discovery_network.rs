//! Opt-in tests: require an active IPv4 LAN interface and multicast permission.
use personal_device_link::{
    discovery::mdns::Discovery,
    network::{client::Client, protocol::Device, server},
};
use std::{
    net::TcpListener,
    process::{Child, Command, Stdio},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    thread,
    time::{Duration, Instant},
};

fn wait_for(discovery: &mut Discovery, name: &str, online: bool, seconds: u64) {
    let deadline = Instant::now() + Duration::from_secs(seconds);
    while Instant::now() < deadline {
        if let Some(peer) = discovery.poll(Duration::from_millis(100)).unwrap() {
            if peer.device.name == name && peer.online == online {
                return;
            }
        }
    }
    panic!("did not observe {name} online={online}; check active LAN and multicast permission");
}

struct ChildGuard(Child);
impl Drop for ChildGuard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[test]
#[ignore = "requires LAN multicast; run explicitly on a development machine"]
fn real_mdns_resolve_tcp_connect_and_goodbye() {
    let name = format!("PDL-test-{}", uuid::Uuid::new_v4());
    let listener = TcpListener::bind("0.0.0.0:0").unwrap();
    let bound = listener.local_addr().unwrap();
    let device = Device::local(name.clone()).unwrap();
    let running = Arc::new(AtomicBool::new(true));
    let server_running = Arc::clone(&running);
    let server_device = device.clone();
    let worker =
        thread::spawn(move || server::serve_until(listener, server_device, &server_running));
    let result = std::panic::catch_unwind(|| {
        let advertiser = Discovery::start(Some((&device, bound))).unwrap();
        let mut browser = Discovery::start(None).unwrap();
        wait_for(&mut browser, &name, true, 15);
        let peer = browser.table().select(&name).unwrap();
        assert!(peer
            .addresses
            .iter()
            .all(|address| address.port() == bound.port()));
        let mut client = peer
            .addresses
            .iter()
            .find_map(|address| {
                Client::connect_with_timeout(
                    *address,
                    Device::local("mDNS-test-client".into()).unwrap(),
                    Duration::from_secs(1),
                )
                .ok()
            })
            .expect("resolved TCP endpoint must be reachable");
        client.heartbeat().unwrap();
        client.disconnect().unwrap();
        drop(advertiser);
        wait_for(&mut browser, &name, false, 10);
    });
    running.store(false, Ordering::Release);
    worker.join().unwrap().unwrap();
    if let Err(panic) = result {
        std::panic::resume_unwind(panic);
    }
}

#[test]
#[ignore = "requires LAN multicast; checks offline after abrupt process termination"]
fn abrupt_exit_becomes_offline_without_goodbye() {
    let name = format!("PDL-crash-{}", uuid::Uuid::new_v4());
    let mut browser = Discovery::start(None).unwrap();
    let mut child = ChildGuard(
        Command::new(env!("CARGO_BIN_EXE_personal-device-link"))
            .args(["listen", "--bind", "0.0.0.0:0", "--name", &name])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()
            .unwrap(),
    );
    wait_for(&mut browser, &name, true, 15);
    child.0.kill().unwrap();
    child.0.wait().unwrap();
    wait_for(&mut browser, &name, false, 40);
}

struct NoCapture;
impl personal_device_link::desktop::CaptureDriver for NoCapture {
    fn permission(&self) -> std::io::Result<()> {
        panic!("discovery must not request input permission")
    }
    fn start(
        &self,
        _: Arc<personal_device_link::input::queue::InputQueue>,
        _: Arc<AtomicBool>,
        _: personal_device_link::input::options::CaptureConfig,
    ) -> std::io::Result<()> {
        panic!("discovery must not capture input")
    }
    fn stop(&self) -> std::io::Result<()> {
        Ok(())
    }
}
struct DesktopGuard(personal_device_link::desktop::Desktop);
impl Drop for DesktopGuard {
    fn drop(&mut self) {
        self.0.join();
    }
}

#[test]
#[ignore = "requires LAN multicast; desktop startup without control permission"]
fn desktops_discover_each_other_with_control_off_and_remain_visible_after_toggle() {
    use personal_device_link::desktop::Desktop;
    let first_name = format!("PDL-first-{}", uuid::Uuid::new_v4());
    let second_name = format!("PDL-second-{}", uuid::Uuid::new_v4());
    let first = DesktopGuard(
        Desktop::new(
            Device::local(first_name.clone()).unwrap(),
            Arc::new(NoCapture),
        )
        .unwrap(),
    );
    let second = DesktopGuard(
        Desktop::new(
            Device::local(second_name.clone()).unwrap(),
            Arc::new(NoCapture),
        )
        .unwrap(),
    );
    for desktop in [&first.0, &second.0] {
        desktop.start_discovery().unwrap();
        desktop.start_network("0.0.0.0:0".parse().unwrap()).unwrap();
        desktop.stop_receiver();
    }
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        if first
            .0
            .snapshot()
            .peers
            .iter()
            .any(|p| p.name == second_name && p.online)
            && second
                .0
                .snapshot()
                .peers
                .iter()
                .any(|p| p.name == first_name && p.online)
        {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "both desktops must advertise without enabling control"
        );
        thread::sleep(Duration::from_millis(100));
    }
    for desktop in [&first.0, &second.0] {
        assert_eq!(desktop.snapshot().receiver.phase, "off");
        assert!(desktop.snapshot().receiver.code.is_none());
        assert!(!desktop
            .snapshot()
            .peers
            .iter()
            .any(|p| p.name == desktop.snapshot().device.name));
    }
    let peer = first
        .0
        .snapshot()
        .peers
        .into_iter()
        .find(|p| p.name == second_name)
        .unwrap();
    first.0.connect(Some(peer.id.clone()), None).unwrap();
    let deadline = Instant::now() + Duration::from_secs(5);
    while first.0.snapshot().connection.latency_ms.is_none() {
        assert!(
            Instant::now() < deadline,
            "discovered endpoint must accept TCP"
        );
        thread::sleep(Duration::from_millis(50));
    }
    second
        .0
        .start_receiver("0.0.0.0:0".parse().unwrap())
        .unwrap();
    second.0.stop_receiver();
    let deadline = Instant::now() + Duration::from_secs(3);
    while Instant::now() < deadline {
        assert!(first
            .0
            .snapshot()
            .peers
            .iter()
            .any(|p| p.id == peer.id && p.online));
        assert_eq!(first.0.snapshot().connection.phase, "connected");
        thread::sleep(Duration::from_millis(100));
    }
}
