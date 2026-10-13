use personal_device_link::network::protocol::*;
use std::io::{self, Cursor, Read};

struct Fragmented<R>(R);
impl<R: Read> Read for Fragmented<R> {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        let n = buf.len().min(1);
        self.0.read(&mut buf[..n])
    }
}

#[test]
fn fragmented_and_coalesced_frames_preserve_message_boundaries() {
    let messages = [
        Message::Hello {
            version: PROTOCOL_VERSION,
            device: Device {
                name: "我的 MacBook".into(),
                os: "macos".into(),
            },
            text: "Hello".into(),
        },
        Message::Ping { sequence: 42 },
        Message::Pong { sequence: 42 },
    ];
    let mut bytes = Vec::new();
    for message in &messages {
        write_message(&mut bytes, message).unwrap();
    }
    let mut fragmented = Fragmented(Cursor::new(bytes.clone()));
    let mut coalesced = Cursor::new(bytes);
    for message in messages {
        assert_eq!(
            read_message(&mut fragmented).unwrap(),
            Some(message.clone())
        );
        assert_eq!(read_message(&mut coalesced).unwrap(), Some(message));
    }
    assert_eq!(read_message(&mut fragmented).unwrap(), None);
    assert_eq!(read_message(&mut coalesced).unwrap(), None);
}

#[test]
fn rejects_zero_or_oversized_lengths_before_reading_payload() {
    for length in [0, MAX_FRAME_BYTES as u32 + 1, u32::MAX] {
        let error = read_message(&mut Cursor::new(length.to_be_bytes())).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::InvalidData);
    }
}

#[test]
fn distinguishes_clean_eof_from_truncated_frames() {
    assert_eq!(read_message(&mut Cursor::new([])).unwrap(), None);
    for bytes in [vec![0], vec![0, 0, 0], vec![0, 0, 0, 4, b'{']] {
        assert_eq!(
            read_message(&mut Cursor::new(bytes)).unwrap_err().kind(),
            io::ErrorKind::UnexpectedEof
        );
    }
}

#[test]
fn rejects_invalid_json_unknown_type_and_missing_payload() {
    for body in [
        "not json",
        "{\"type\":\"keyboard\",\"payload\":{}}",
        "{\"type\":\"ping\"}",
        "{\"type\":\"ping\",\"payload\":{\"sequence\":-1}}",
    ] {
        let mut bytes = (body.len() as u32).to_be_bytes().to_vec();
        bytes.extend_from_slice(body.as_bytes());
        assert_eq!(
            read_message(&mut Cursor::new(bytes)).unwrap_err().kind(),
            io::ErrorKind::InvalidData
        );
    }
}

#[test]
fn oversized_outgoing_message_writes_nothing() {
    let mut bytes = Vec::new();
    assert!(write_message(
        &mut bytes,
        &Message::Error {
            message: "x".repeat(MAX_FRAME_BYTES)
        }
    )
    .is_err());
    assert!(bytes.is_empty());
}

#[test]
fn validates_device_names_and_os() {
    for name in ["", "   ", "bad\nname", "\u{1b}[31m"] {
        assert!(Device::local(name.into()).is_err());
    }
    assert!(Device::local("x".repeat(129)).is_err());
    assert!(Device::local("工作 Mac".into()).is_ok());
}
