//! Bluetooth roles are independent of the operating system and control direction.
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    Listen,
    Connect,
}

pub fn address(value: &str, channel: u16) -> Result<String, String> {
    let address: String = value
        .chars()
        .filter(|c| !matches!(c, ':' | '-' | ' '))
        .collect();
    if address.len() != 12
        || !address.bytes().all(|b| b.is_ascii_hexdigit())
        || !(1..=30).contains(&channel)
    {
        return Err("请输入对方设备显示的蓝牙地址和频道（1–30）".into());
    }
    Ok(address.to_uppercase())
}

impl Role {
    pub fn arguments(self, peer: &str, channel: u16) -> Result<Vec<String>, String> {
        match self {
            Self::Listen => Ok(vec!["--listen".into()]),
            Self::Connect => Ok(vec![
                "--connect".into(),
                address(peer, channel)?,
                channel.to_string(),
            ]),
        }
    }
    pub fn odd_stream_ids(self) -> bool {
        self == Self::Listen
    }
}

pub fn parse(arguments: &[String]) -> Result<(Role, Option<(String, u16)>), String> {
    match arguments {
        [mode] if mode == "--listen" => Ok((Role::Listen, None)),
        [mode, peer, channel] if mode == "--connect" || mode == "--mac" => {
            let channel = channel
                .parse::<u16>()
                .map_err(|_| "频道必须为 1–30".to_string())?;
            Ok((Role::Connect, Some((address(peer, channel)?, channel))))
        }
        _ => Err("用法：--listen 或 --connect 对方蓝牙地址 频道".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn roles_use_disjoint_stream_ids_and_validate_only_connect_fields() {
        assert_ne!(
            Role::Listen.odd_stream_ids(),
            Role::Connect.odd_stream_ids()
        );
        assert_eq!(Role::Listen.arguments("", 0).unwrap(), ["--listen"]);
        let args = Role::Connect.arguments("aa:bb:cc:dd:ee:ff", 30).unwrap();
        assert_eq!(
            parse(&args).unwrap(),
            (Role::Connect, Some(("AABBCCDDEEFF".into(), 30)))
        );
        for (peer, channel) in [("$(whoami)", 1), ("AABBCCDDEEFF", 0), ("AABBCCDDEEFF", 31)] {
            assert!(Role::Connect.arguments(peer, channel).is_err());
        }
        assert!(parse(&["--listen".into(), "extra".into()]).is_err());
    }
}
