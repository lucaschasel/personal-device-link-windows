pub mod client;
pub mod control;
pub mod protocol;
pub mod server;

use std::{
    io::{self, Read},
    net::TcpStream,
    time::{Duration, Instant},
};

pub const DEFAULT_PORT: u16 = 45832;
pub const IO_TIMEOUT: Duration = Duration::from_secs(15);
pub const HEARTBEAT_INTERVAL: Duration = Duration::from_secs(5);

pub fn configure_stream(stream: &TcpStream) -> io::Result<()> {
    stream.set_nodelay(true)?;
    stream.set_read_timeout(Some(IO_TIMEOUT))?;
    stream.set_write_timeout(Some(IO_TIMEOUT))
}

/// A frame deadline, not a per-read timeout: slow partial frames cannot keep
/// remotely pressed keys held indefinitely.
pub fn read_with_deadline(
    stream: &mut TcpStream,
    timeout: Duration,
) -> io::Result<Option<protocol::Message>> {
    struct DeadlineReader<'a> {
        stream: &'a mut TcpStream,
        deadline: Instant,
    }
    impl Read for DeadlineReader<'_> {
        fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
            let remaining = self.deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "control frame deadline exceeded",
                ));
            }
            self.stream.set_read_timeout(Some(remaining))?;
            self.stream.read(buffer)
        }
    }
    protocol::read_message(&mut DeadlineReader {
        stream,
        deadline: Instant::now() + timeout,
    })
}
