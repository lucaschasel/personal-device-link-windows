use crate::input::keyboard::Key;
use crate::input::pointer::PointerEvent;
use serde::{Deserialize, Serialize};
use std::io::{self, Read, Write};

pub const PROTOCOL_VERSION: u16 = 7;
pub const MAX_FRAME_BYTES: usize = 64 * 1024;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Capabilities {
    pub keyboard: bool,
    pub pointer: bool,
    pub files: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Device {
    pub name: String,
    pub os: String,
}

impl Device {
    pub fn local(name: String) -> io::Result<Self> {
        let device = Self {
            name,
            os: std::env::consts::OS.to_owned(),
        };
        device.validate()?;
        Ok(device)
    }

    pub fn validate(&self) -> io::Result<()> {
        for (value, limit) in [(&self.name, 128), (&self.os, 32)] {
            if value.trim().is_empty() || value.len() > limit || value.chars().any(char::is_control)
            {
                return Err(invalid_data("invalid device name or OS"));
            }
        }
        Ok(())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "type",
    content = "payload",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub enum Message {
    ClipboardHello {
        version: u16,
        token: String,
        device: Device,
    },
    ClipboardReady {
        device: Device,
    },
    Hello {
        version: u16,
        device: Device,
        text: String,
    },
    HelloAck {
        version: u16,
        device: Device,
        text: String,
        capabilities: Capabilities,
    },
    Ping {
        sequence: u64,
    },
    FileOffer {
        version: u16,
        device: Device,
        name: String,
        size: u64,
    },
    Pong {
        sequence: u64,
    },
    ControlRequest {
        token: String,
        options: crate::input::options::ControlOptions,
    },
    ControlStarted {},
    UpdateControl {
        options: crate::input::options::ControlOptions,
    },
    ControlUpdated {
        options: crate::input::options::ControlOptions,
    },
    Keyboard {
        key: Key,
        pressed: bool,
        repeat: bool,
    },
    Pointer {
        event: PointerEvent,
    },
    StopControl {},
    ControlStopped {},
    Goodbye {
        reason: String,
    },
    Error {
        message: String,
    },
}

pub fn invalid_data(message: impl Into<String>) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.into())
}

// TCP is a byte stream: a 4-byte big-endian length frames each UTF-8 JSON message.
pub fn write_message(writer: &mut impl Write, message: &Message) -> io::Result<()> {
    let body = serde_json::to_vec(message).map_err(|e| invalid_data(e.to_string()))?;
    if body.len() > MAX_FRAME_BYTES {
        return Err(invalid_data("control frame exceeds 64 KiB"));
    }
    writer.write_all(&(body.len() as u32).to_be_bytes())?;
    writer.write_all(&body)?;
    writer.flush()
}

// None means clean EOF at a frame boundary. A truncated header/body is an error.
// After any error the caller must close the connection, never retry mid-frame.
pub fn read_message(reader: &mut impl Read) -> io::Result<Option<Message>> {
    let mut header = [0_u8; 4];
    loop {
        match reader.read(&mut header[..1]) {
            Ok(0) => return Ok(None),
            Ok(_) => break,
            Err(e) if e.kind() == io::ErrorKind::Interrupted => continue,
            Err(e) => return Err(e),
        }
    }
    reader.read_exact(&mut header[1..])?;
    let length = u32::from_be_bytes(header) as usize;
    if length == 0 || length > MAX_FRAME_BYTES {
        return Err(invalid_data("invalid control frame length"));
    }
    let mut body = vec![0; length];
    reader.read_exact(&mut body)?;
    serde_json::from_slice(&body)
        .map(Some)
        .map_err(|e| invalid_data(format!("invalid control message: {e}")))
}
