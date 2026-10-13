use personal_device_link::{
    discovery::mdns::SCAN_DURATION,
    network::{protocol::Device, DEFAULT_PORT},
};
use std::{env, io, net::SocketAddr};

pub const HELP: &str = "Personal Device Link — bidirectional keyboard and pointer control

Usage:
  personal-device-link listen [--bind IP:PORT] [--name NAME] [--no-discovery] [--allow-control]
  personal-device-link discover [--seconds 5 | --watch]
  personal-device-link connect IP:PORT [--name NAME] [--once]
  personal-device-link connect --device NAME_OR_ID [--seconds 5] [--name NAME] [--once]
  personal-device-link control IP:PORT [--name NAME]
  personal-device-link control --device NAME_OR_ID [--seconds 5] [--name NAME]

Examples:
  personal-device-link listen --name Windows-PC
  personal-device-link discover
  personal-device-link connect --device Windows-PC --name MacBook --once

Listen defaults to 0.0.0.0:45832 and publishes an IPv4 mDNS service.
Use --bind 127.0.0.1:45832 for local-only TCP tests.
--seconds accepts 1..60. --watch prints Online/Offline changes until Ctrl+C.
Duplicate device names require the full ID displayed by discover.
--once verifies Hello and one heartbeat, then disconnects.
Without --once, a heartbeat is sent every 5 seconds.
Windows/macOS: listen --allow-control displays a fresh receiver code.
control explicitly starts BOTH keyboard and pointer capture after asking for the code.
Emergency stop: Mac Control+Option+Esc; Windows Ctrl+Alt+Esc.
Release all modifiers and mouse buttons before control.\nUse the desktop GUI for separate keyboard/pointer switches and file transfer.";

#[derive(Debug, PartialEq)]
pub enum Target {
    Address(SocketAddr),
    Device(String),
}

#[derive(Debug, PartialEq)]
pub enum Command {
    Help,
    Listen {
        bind: SocketAddr,
        name: String,
        no_discovery: bool,
        allow_control: bool,
    },
    Discover {
        seconds: u64,
        watch: bool,
    },
    Connect {
        target: Target,
        name: String,
        once: bool,
        seconds: u64,
    },
    Control {
        target: Target,
        name: String,
        seconds: u64,
    },
}

fn bad(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message.into())
}

pub fn parse(values: Vec<String>) -> io::Result<Command> {
    let mut args = values.into_iter().peekable();
    let Some(mode) = args.next() else {
        return Ok(Command::Help);
    };
    if matches!(mode.as_str(), "-h" | "--help") {
        return Ok(Command::Help);
    }
    if !matches!(mode.as_str(), "listen" | "connect" | "discover" | "control") {
        return Err(bad(format!("unknown command: {mode}")));
    }
    if matches!(args.peek().map(String::as_str), Some("-h" | "--help")) {
        return Ok(Command::Help);
    }
    let mut target = None;
    if matches!(mode.as_str(), "connect" | "control")
        && args.peek().is_some_and(|s| !s.starts_with("--"))
    {
        target = Some(Target::Address(
            args.next()
                .unwrap()
                .parse()
                .map_err(|_| bad("expected numeric IP:PORT"))?,
        ));
    }
    let mut bind = SocketAddr::from(([0, 0, 0, 0], DEFAULT_PORT));
    let mut name = env::var("COMPUTERNAME")
        .or_else(|_| env::var("HOSTNAME"))
        .unwrap_or_else(|_| format!("{}-device", env::consts::OS));
    let (mut once, mut watch, mut no_discovery) = (false, false, false);
    let mut allow_control = false;
    let mut seconds = SCAN_DURATION.as_secs();
    let mut seen = std::collections::HashSet::new();
    while let Some(flag) = args.next() {
        if !seen.insert(flag.clone()) {
            return Err(bad(format!("duplicate option: {flag}")));
        }
        match flag.as_str() {
            "--name" if mode != "discover" => {
                name = args.next().ok_or_else(|| bad("--name requires a value"))?
            }
            "--bind" if mode == "listen" => {
                bind = args
                    .next()
                    .ok_or_else(|| bad("--bind requires IP:PORT"))?
                    .parse()
                    .map_err(|_| bad("invalid bind address"))?
            }
            "--no-discovery" if mode == "listen" => no_discovery = true,
            "--allow-control" if mode == "listen" => allow_control = true,
            "--once" if mode == "connect" => once = true,
            "--device" if matches!(mode.as_str(), "connect" | "control") && target.is_none() => {
                let value = args
                    .next()
                    .ok_or_else(|| bad("--device requires a name or ID"))?;
                Device::local(value.clone())?;
                target = Some(Target::Device(value));
            }
            "--seconds" if matches!(mode.as_str(), "discover" | "connect" | "control") => {
                seconds = args
                    .next()
                    .ok_or_else(|| bad("--seconds requires a value"))?
                    .parse()
                    .map_err(|_| bad("--seconds must be an integer in 1..60"))?;
                if !(1..=60).contains(&seconds) {
                    return Err(bad("--seconds must be in 1..60"));
                }
            }
            "--watch" if mode == "discover" => watch = true,
            "-h" | "--help" => return Ok(Command::Help),
            _ => return Err(bad(format!("unsupported option for {mode}: {flag}"))),
        }
    }
    if seen.contains("--seconds") && (watch || matches!(target, Some(Target::Address(_)))) {
        return Err(bad(
            "--seconds is only for finite discovery or connecting by device name/ID",
        ));
    }
    if mode == "discover" {
        return Ok(Command::Discover { seconds, watch });
    }
    Device::local(name.clone())?;
    if mode == "listen" {
        return Ok(Command::Listen {
            bind,
            name,
            no_discovery,
            allow_control,
        });
    }
    if mode == "control" {
        return Ok(Command::Control {
            target: target.ok_or_else(|| bad("control requires IP:PORT or --device NAME_OR_ID"))?,
            name,
            seconds,
        });
    }
    Ok(Command::Connect {
        target: target.ok_or_else(|| bad("connect requires IP:PORT or --device NAME_OR_ID"))?,
        name,
        once,
        seconds,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    fn parse_args(values: &[&str]) -> io::Result<Command> {
        parse(values.iter().map(|s| (*s).to_owned()).collect())
    }

    #[test]
    fn rejects_invalid_or_conflicting_options() {
        for args in [
            vec!["connect"],
            vec!["control"],
            vec!["control", "--device", "PC", "--once"],
            vec!["connect", "--device", "PC", "--allow-control"],
            vec!["oops"],
            vec!["listen", "--once"],
            vec!["connect", "wrong"],
            vec!["listen", "--bind"],
            vec!["listen", "--name", ""],
            vec!["discover", "--seconds", "0"],
            vec!["discover", "--seconds", "61"],
            vec!["discover", "--seconds", "abc"],
            vec!["discover", "--watch", "--seconds", "5"],
            vec!["connect", "--device"],
            vec!["connect", "127.0.0.1:5", "--device", "PC"],
            vec!["connect", "127.0.0.1:5", "--seconds", "5"],
            vec!["listen", "--bind", "127.0.0.1:1", "--bind", "127.0.0.1:2"],
        ] {
            assert!(parse_args(&args).is_err(), "{args:?}");
        }
    }

    #[test]
    fn keyboard_control_requires_explicit_sender_and_receiver_commands() {
        assert_eq!(
            parse_args(&["control", "--device", "Windows-PC", "--name", "Mac"]).unwrap(),
            Command::Control {
                target: Target::Device("Windows-PC".into()),
                name: "Mac".into(),
                seconds: 5
            }
        );
        assert_eq!(
            parse_args(&["listen", "--name", "PC", "--allow-control"]).unwrap(),
            Command::Listen {
                bind: "0.0.0.0:45832".parse().unwrap(),
                name: "PC".into(),
                no_discovery: false,
                allow_control: true
            }
        );
    }

    #[test]
    fn preserves_manual_connection_and_loopback_options() {
        assert_eq!(parse_args(&["connect", "--help"]).unwrap(), Command::Help);
        assert_eq!(
            parse_args(&[
                "connect",
                "192.168.1.20:45832",
                "--name",
                "MacBook",
                "--once"
            ])
            .unwrap(),
            Command::Connect {
                target: Target::Address("192.168.1.20:45832".parse().unwrap()),
                name: "MacBook".into(),
                once: true,
                seconds: 5
            }
        );
        assert_eq!(
            parse_args(&[
                "listen",
                "--bind",
                "127.0.0.1:45832",
                "--name",
                "PC",
                "--no-discovery"
            ])
            .unwrap(),
            Command::Listen {
                bind: "127.0.0.1:45832".parse().unwrap(),
                name: "PC".into(),
                no_discovery: true,
                allow_control: false
            }
        );
    }

    #[test]
    fn parses_discovery_and_device_selection() {
        assert_eq!(
            parse_args(&["discover"]).unwrap(),
            Command::Discover {
                seconds: 5,
                watch: false
            }
        );
        assert_eq!(
            parse_args(&["discover", "--watch"]).unwrap(),
            Command::Discover {
                seconds: 5,
                watch: true
            }
        );
        assert_eq!(
            parse_args(&[
                "connect",
                "--device",
                "我的 PC",
                "--seconds",
                "10",
                "--name",
                "Mac"
            ])
            .unwrap(),
            Command::Connect {
                target: Target::Device("我的 PC".into()),
                name: "Mac".into(),
                once: false,
                seconds: 10
            }
        );
        assert_eq!(
            parse_args(&["listen", "--name", "PC"]).unwrap(),
            Command::Listen {
                bind: "0.0.0.0:45832".parse().unwrap(),
                name: "PC".into(),
                no_discovery: false,
                allow_control: false
            }
        );
    }
}
