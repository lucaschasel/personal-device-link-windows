use mdns_sd::{IntoTxtProperties, ResolvedService, ServiceEvent};
use personal_device_link::{
    discovery::mdns::{service_info, DeviceTable, NearbyDevice, SERVICE_TYPE},
    network::protocol::Device,
};
use std::time::{Duration, Instant};
use uuid::Uuid;

fn record(name: &str, id: u128, ip: &str) -> ResolvedService {
    service_info(
        &Device {
            name: name.into(),
            os: "windows".into(),
        },
        format!("{ip}:45832").parse().unwrap(),
        Uuid::from_u128(id),
    )
    .unwrap()
    .as_resolved_service()
}
fn resolved(info: ResolvedService) -> ServiceEvent {
    ServiceEvent::ServiceResolved(Box::new(info))
}

#[test]
fn metadata_roundtrip_and_multi_interface_deduplication() {
    let mut info = record("工作 PC", 1, "192.168.1.20");
    info.addresses
        .insert("10.0.0.2".parse::<std::net::IpAddr>().unwrap().into());
    let peer = NearbyDevice::from_resolved(&info).unwrap();
    assert_eq!(peer.device.name, "工作 PC");
    assert_eq!(peer.device.os, "windows");
    assert_eq!(peer.addresses.len(), 2);
    let mut table = DeviceTable::default();
    assert!(table
        .apply(resolved(info.clone()), "", Instant::now())
        .is_some());
    assert!(table.apply(resolved(info), "", Instant::now()).is_none());
    assert_eq!(table.snapshot().len(), 1);
}

#[test]
fn self_exclusion_and_ambiguous_names() {
    let mut table = DeviceTable::default();
    let first = record("PC", 1, "192.168.1.20");
    assert!(table
        .apply(
            resolved(first.clone()),
            &Uuid::from_u128(1).to_string(),
            Instant::now()
        )
        .is_none());
    table.apply(resolved(first), "", Instant::now());
    table.apply(
        resolved(record("PC", 2, "192.168.1.21")),
        "",
        Instant::now(),
    );
    assert!(table
        .select("PC")
        .unwrap_err()
        .to_string()
        .contains("multiple devices"));
    assert_eq!(
        table
            .select(&Uuid::from_u128(2).to_string())
            .unwrap()
            .addresses[0]
            .ip()
            .to_string(),
        "192.168.1.21"
    );
    assert!(table.select("missing").is_err());
}

#[test]
fn removal_reappearance_address_update_and_expiry() {
    let mut table = DeviceTable::default();
    let start = Instant::now();
    let info = record("PC", 1, "192.168.1.20");
    table.apply(resolved(info.clone()), "", start);
    let remove = ServiceEvent::ServiceRemoved(SERVICE_TYPE.into(), info.fullname.clone());
    assert!(!table.apply(remove.clone(), "", start).unwrap().online);
    assert!(table.select("PC").is_err());
    assert!(table.apply(remove.clone(), "", start).is_none());
    assert!(
        table
            .apply(resolved(record("PC", 1, "192.168.1.22")), "", start)
            .unwrap()
            .online
    );
    assert_eq!(
        table.select("PC").unwrap().addresses[0].ip().to_string(),
        "192.168.1.22"
    );
    table.apply(remove, "", start);
    table.prune(start + Duration::from_secs(61));
    assert!(table.snapshot().is_empty());
}

#[test]
fn rejects_malformed_or_incompatible_advertisements() {
    let info = record("PC", 1, "192.168.1.20");
    let mut wrong_type = info.clone();
    wrong_type.ty_domain = "_other._tcp.local.".into();
    let mut zero_port = info.clone();
    zero_port.port = 0;
    let mut loopback = info.clone();
    loopback.addresses = ["127.0.0.1".parse::<std::net::IpAddr>().unwrap().into()].into();
    let mut bad_txt = info;
    bad_txt.txt_properties = [
        ("version", "999"),
        ("name", "PC"),
        ("os", "windows"),
        ("id", "bad"),
    ]
    .as_slice()
    .into_txt_properties();
    for info in [wrong_type, zero_port, loopback, bad_txt] {
        assert!(NearbyDevice::from_resolved(&info).is_err());
    }
}

#[test]
fn incompatible_update_cannot_leave_old_device_online() {
    let mut table = DeviceTable::default();
    let mut info = record("PC", 1, "192.168.1.20");
    table.apply(resolved(info.clone()), "", Instant::now());
    info.port = 0;
    assert!(
        !table
            .apply(resolved(info), "", Instant::now())
            .unwrap()
            .online
    );
    assert!(table.select("PC").is_err());
}

#[test]
fn publishes_only_bound_lan_addresses_and_real_port() {
    let device = Device::local("PC".into()).unwrap();
    for address in ["127.0.0.1:45832", "192.168.1.20:0", "[::1]:45832"] {
        assert!(service_info(&device, address.parse().unwrap(), Uuid::nil()).is_err());
    }
    let auto = service_info(&device, "0.0.0.0:55555".parse().unwrap(), Uuid::nil()).unwrap();
    assert!(auto.is_addr_auto());
    assert_eq!(auto.get_port(), 55555);
    assert!(auto.get_addresses().is_empty()); // Populated from active interfaces by the daemon.
    assert!(
        !service_info(&device, "192.168.1.20:5".parse().unwrap(), Uuid::nil())
            .unwrap()
            .is_addr_auto()
    );
}
