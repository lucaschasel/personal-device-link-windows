use crate::network::protocol::{invalid_data, Device, PROTOCOL_VERSION};
use mdns_sd::{
    DaemonEvent, IfKind, Receiver, RecvTimeoutError, ResolvedService, ServiceDaemon, ServiceEvent,
    ServiceInfo,
};
use std::{
    collections::BTreeMap,
    io,
    net::{IpAddr, Ipv4Addr, SocketAddr},
    time::{Duration, Instant},
};
use uuid::Uuid;

pub const SERVICE_TYPE: &str = "_pdl._tcp.local.";
pub const SCAN_DURATION: Duration = Duration::from_secs(5);
const VERIFY_INTERVAL: Duration = Duration::from_secs(15);
const VERIFY_TIMEOUT: Duration = Duration::from_secs(10);
const OFFLINE_RETENTION: Duration = Duration::from_secs(60);
const MAX_DEVICES: usize = 256;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NearbyDevice {
    pub id: String,
    pub fullname: String,
    pub device: Device,
    pub addresses: Vec<SocketAddr>,
    /// mDNS presence, not authentication or a guarantee that TCP is reachable.
    pub online: bool,
}

fn usable_ipv4(ip: Ipv4Addr) -> bool {
    !ip.is_loopback() && !ip.is_unspecified() && !ip.is_multicast() && !ip.is_broadcast()
}

impl NearbyDevice {
    pub fn from_resolved(info: &ResolvedService) -> io::Result<Self> {
        if info.ty_domain != SERVICE_TYPE
            || !info.fullname.to_ascii_lowercase().ends_with(SERVICE_TYPE)
            || info.port == 0
            || info.get_property_val_str("version") != Some(&PROTOCOL_VERSION.to_string())
        {
            return Err(invalid_data("incompatible discovery record"));
        }
        let property = |key| {
            info.get_property_val_str(key)
                .ok_or_else(|| invalid_data(format!("missing discovery property: {key}")))
        };
        let id = Uuid::parse_str(property("id")?)
            .map_err(|_| invalid_data("invalid instance ID"))?
            .to_string();
        let device = Device {
            name: property("name")?.to_owned(),
            os: property("os")?.to_owned(),
        };
        device.validate()?;
        let mut addresses: Vec<_> = info
            .get_addresses_v4()
            .into_iter()
            .filter(|ip| usable_ipv4(*ip))
            .map(|ip| SocketAddr::from((ip, info.port)))
            .collect();
        addresses.sort_by_key(|address| {
            let private = matches!(address.ip(), IpAddr::V4(ip) if ip.is_private());
            (!private, *address)
        });
        addresses.dedup();
        if addresses.is_empty() || addresses.len() > 32 {
            return Err(invalid_data(
                "no usable IPv4 addresses, or too many addresses",
            ));
        }
        Ok(Self {
            id,
            fullname: info.fullname.to_ascii_lowercase(),
            device,
            addresses,
            online: true,
        })
    }
}

struct Entry {
    peer: NearbyDevice,
    offline_since: Option<Instant>,
}

/// Keys are DNS service instances, not display names. Two machines may share a name.
#[derive(Default)]
pub struct DeviceTable {
    entries: BTreeMap<String, Entry>,
}

impl DeviceTable {
    pub fn apply(
        &mut self,
        event: ServiceEvent,
        own_id: &str,
        now: Instant,
    ) -> Option<NearbyDevice> {
        match event {
            ServiceEvent::ServiceResolved(info) => {
                let peer = match NearbyDevice::from_resolved(&info) {
                    Ok(peer) => peer,
                    Err(_) => return self.remove(info.get_fullname(), now),
                };
                if peer.id == own_id {
                    return None;
                }
                if let Some(entry) = self.entries.get(&peer.fullname) {
                    if entry.peer == peer {
                        return None;
                    }
                } else if self.entries.len() >= MAX_DEVICES {
                    return None;
                }
                self.entries.insert(
                    peer.fullname.clone(),
                    Entry {
                        peer: peer.clone(),
                        offline_since: None,
                    },
                );
                Some(peer)
            }
            ServiceEvent::ServiceRemoved(ty, fullname) if ty == SERVICE_TYPE => {
                self.remove(&fullname, now)
            }
            _ => None,
        }
    }

    fn remove(&mut self, fullname: &str, now: Instant) -> Option<NearbyDevice> {
        let entry = self.entries.get_mut(&fullname.to_ascii_lowercase())?;
        if !entry.peer.online {
            return None;
        }
        entry.peer.online = false;
        entry.offline_since = Some(now);
        Some(entry.peer.clone())
    }

    pub fn prune(&mut self, now: Instant) {
        self.entries.retain(|_, entry| {
            entry
                .offline_since
                .is_none_or(|since| now.saturating_duration_since(since) < OFFLINE_RETENTION)
        });
    }

    pub fn snapshot(&self) -> Vec<NearbyDevice> {
        let mut peers: Vec<_> = self
            .entries
            .values()
            .map(|entry| entry.peer.clone())
            .collect();
        peers.sort_by(|a, b| (&a.device.name, &a.id).cmp(&(&b.device.name, &b.id)));
        peers
    }

    pub fn select(&self, name_or_id: &str) -> io::Result<NearbyDevice> {
        let matches: Vec<_> = self
            .entries
            .values()
            .filter(|entry| {
                entry.peer.online
                    && (entry.peer.device.name == name_or_id || entry.peer.id == name_or_id)
            })
            .collect();
        match matches.as_slice() {
            [entry] => Ok(entry.peer.clone()),
            [] => Err(io::Error::new(io::ErrorKind::NotFound,
                format!("no online device matches {name_or_id:?}; run discover --seconds 10 or connect by IP"))),
            _ => Err(invalid_data(format!("multiple devices match {name_or_id:?}; use the full ID shown by discover"))),
        }
    }
}

/// Builds records only for LAN IPv4 listeners. Loopback and IPv6 stay available
/// via direct TCP, without publishing an endpoint other hosts cannot use.
pub fn service_info(device: &Device, bind: SocketAddr, id: Uuid) -> io::Result<ServiceInfo> {
    device.validate()?;
    let IpAddr::V4(ip) = bind.ip() else {
        return Err(invalid_data(
            "automatic discovery currently requires an IPv4 listener",
        ));
    };
    if bind.port() == 0 || (!ip.is_unspecified() && !usable_ipv4(ip)) {
        return Err(invalid_data(
            "automatic discovery requires a bound LAN TCP endpoint",
        ));
    }
    let id = id.to_string();
    let instance = format!("pdl-{id}");
    let hostname = format!("{instance}.local.");
    let props = [
        ("id", id.as_str()),
        ("name", device.name.as_str()),
        ("os", device.os.as_str()),
        ("version", &PROTOCOL_VERSION.to_string()),
    ];
    let addresses = if ip.is_unspecified() {
        String::new()
    } else {
        ip.to_string()
    };
    let mut info = ServiceInfo::new(
        SERVICE_TYPE,
        &instance,
        &hostname,
        addresses.as_str(),
        bind.port(),
        &props[..],
    )
    .map_err(mdns_error)?;
    if ip.is_unspecified() {
        info = info.enable_addr_auto();
        info.set_interfaces(vec![IfKind::IPv4]);
    } else {
        info.set_interfaces(vec![IfKind::Addr(IpAddr::V4(ip))]);
    }
    Ok(info)
}

fn mdns_error(error: mdns_sd::Error) -> io::Error {
    io::Error::other(format!("mDNS: {error}"))
}

/// Owns daemon lifecycle so failed setup and normal exits both release sockets.
pub struct Discovery {
    daemon: ServiceDaemon,
    events: Receiver<ServiceEvent>,
    monitor: Receiver<DaemonEvent>,
    registered: Option<String>,
    own_id: String,
    table: DeviceTable,
    last_verify: Instant,
}

impl Discovery {
    pub fn own_id(&self) -> &str {
        &self.own_id
    }
    pub fn start(local: Option<(&Device, SocketAddr)>) -> io::Result<Self> {
        let daemon = ServiceDaemon::new().map_err(mdns_error)?;
        let setup = (|| {
            daemon.disable_interface(IfKind::IPv6).map_err(mdns_error)?;
            daemon.set_ip_check_interval(5).map_err(mdns_error)?;
            let monitor = daemon.monitor().map_err(mdns_error)?;
            let events = daemon.browse(SERVICE_TYPE).map_err(mdns_error)?;
            Ok((monitor, events))
        })();
        let (monitor, events) = match setup {
            Ok(channels) => channels,
            Err(e) => {
                if let Ok(done) = daemon.shutdown() {
                    let _ = done.recv_timeout(Duration::from_secs(1));
                }
                return Err(e);
            }
        };
        let id = Uuid::new_v4();
        let mut discovery = Self {
            daemon,
            events,
            monitor,
            registered: None,
            own_id: id.to_string(),
            table: DeviceTable::default(),
            last_verify: Instant::now(),
        };
        if let Some((device, bind)) = local {
            let info = service_info(device, bind, id)?;
            let fullname = info.get_fullname().to_owned();
            discovery.daemon.register(info).map_err(mdns_error)?;
            discovery.registered = Some(fullname);
        }
        Ok(discovery)
    }

    pub fn table(&self) -> &DeviceTable {
        &self.table
    }

    pub fn poll(&mut self, wait: Duration) -> io::Result<Option<NearbyDevice>> {
        // Surface asynchronous bind/registration failures instead of silently showing no devices.
        for event in self.monitor.try_iter() {
            if let DaemonEvent::Error(error) = event {
                return Err(mdns_error(error));
            }
        }
        let now = Instant::now();
        self.table.prune(now);
        // Never overlap verifies: repeated hints can otherwise postpone expiration forever.
        if now.duration_since(self.last_verify) >= VERIFY_INTERVAL {
            for peer in self.table.snapshot().into_iter().filter(|peer| peer.online) {
                self.daemon
                    .verify(peer.fullname, VERIFY_TIMEOUT)
                    .map_err(mdns_error)?;
            }
            self.last_verify = now;
        }
        match self.events.recv_timeout(wait) {
            Ok(event) => Ok(self.table.apply(event, &self.own_id, Instant::now())),
            Err(RecvTimeoutError::Timeout) => Ok(None),
            Err(RecvTimeoutError::Disconnected) => {
                Err(io::Error::other("mDNS discovery stopped unexpectedly"))
            }
        }
    }
}

impl Drop for Discovery {
    fn drop(&mut self) {
        if let Some(fullname) = self.registered.take() {
            if let Ok(done) = self.daemon.unregister(&fullname) {
                let _ = done.recv_timeout(Duration::from_secs(1));
            }
        }
        let _ = self.daemon.stop_browse(SERVICE_TYPE);
        if let Ok(done) = self.daemon.shutdown() {
            let _ = done.recv_timeout(Duration::from_secs(1));
        }
    }
}
