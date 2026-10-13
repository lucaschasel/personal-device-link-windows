use super::control::CONTROL_TIMEOUT;
use super::{configure_stream, protocol::*, IO_TIMEOUT};
use crate::input::keyboard::KeyEvent;
use crate::input::pointer::PointerEvent;
use std::{
    io,
    net::{Shutdown, SocketAddr, TcpStream},
    time::{Duration, Instant},
};

pub struct Client {
    stream: TcpStream,
    pub peer: Device,
    pub capabilities: Capabilities,
    sequence: u64,
    controlling: bool,
    consent_wait: bool,
}

impl Client {
    pub fn cancel_socket(&self) -> io::Result<TcpStream> {
        self.stream.try_clone()
    }
    pub fn set_idle_timeout(&self, timeout: Duration) -> io::Result<()> {
        self.stream.set_read_timeout(Some(timeout))?;
        self.stream.set_write_timeout(Some(timeout))
    }
    pub fn connect(address: SocketAddr, device: Device) -> io::Result<Self> {
        Self::connect_with_timeout(address, device, IO_TIMEOUT)
    }

    /// A shorter per-I/O timeout allows trying multiple discovered interfaces.
    pub fn connect_with_timeout(
        address: SocketAddr,
        device: Device,
        timeout: Duration,
    ) -> io::Result<Self> {
        device.validate()?;
        let mut stream = TcpStream::connect_timeout(&address, timeout)?;
        configure_stream(&stream)?;
        stream.set_read_timeout(Some(timeout))?;
        stream.set_write_timeout(Some(timeout))?;
        write_message(
            &mut stream,
            &Message::Hello {
                version: PROTOCOL_VERSION,
                device,
                text: "Hello".to_owned(),
            },
        )?;
        match read_message(&mut stream)? {
            Some(Message::HelloAck {
                version,
                device,
                text,
                capabilities,
            }) if version == PROTOCOL_VERSION && text == "Hello" => {
                device.validate()?;
                configure_stream(&stream)?;
                Ok(Self {
                    stream,
                    peer: device,
                    capabilities,
                    sequence: 0,
                    controlling: false,
                    consent_wait: false,
                })
            }
            Some(Message::Error { message }) => {
                Err(invalid_data(format!("peer rejected hello: {message:?}")))
            }
            _ => Err(invalid_data("missing or invalid hello acknowledgement")),
        }
    }

    pub fn heartbeat(&mut self) -> io::Result<Duration> {
        self.sequence = self.sequence.wrapping_add(1);
        let started = Instant::now();
        write_message(
            &mut self.stream,
            &Message::Ping {
                sequence: self.sequence,
            },
        )?;
        let response = if self.controlling {
            super::read_with_deadline(&mut self.stream, CONTROL_TIMEOUT)?
        } else {
            read_message(&mut self.stream)?
        };
        match response {
            Some(Message::Pong { sequence }) if sequence == self.sequence => Ok(started.elapsed()),
            _ => Err(invalid_data("missing or mismatched heartbeat reply")),
        }
    }

    pub fn start_control(&mut self, token: String) -> io::Result<()> {
        self.start_control_with_options(token, crate::input::options::ControlOptions::ALL)
    }
    pub fn start_control_with_options(
        &mut self,
        token: String,
        options: crate::input::options::ControlOptions,
    ) -> io::Result<()> {
        options.validate()?;
        if (options.keyboard && !self.capabilities.keyboard)
            || (options.pointer && !self.capabilities.pointer)
        {
            return Err(invalid_data("对方未启用所选控制能力，请开启接收并重新连接"));
        }
        if !token.is_empty() && (token.len() > 128 || token.len() < 32) {
            return Err(invalid_data("invalid control code format"));
        }
        self.consent_wait = token.is_empty();
        self.stream
            .set_write_timeout(Some(Duration::from_millis(500)))?;
        write_message(
            &mut self.stream,
            &Message::ControlRequest { token, options },
        )?;
        match super::read_with_deadline(
            &mut self.stream,
            if self.consent_wait {
                crate::consent::WAIT_TIMEOUT
            } else {
                CONTROL_TIMEOUT
            },
        )? {
            Some(Message::ControlStarted {}) => {
                self.controlling = true;
                Ok(())
            }
            Some(Message::Error { message }) => {
                Err(invalid_data(format!("control denied: {message}")))
            }
            _ => Err(invalid_data("missing control acknowledgement")),
        }
    }

    /// Empty options pause input while retaining the authorized session.
    pub fn update_control(
        &mut self,
        options: crate::input::options::ControlOptions,
    ) -> io::Result<()> {
        if !self.controlling {
            return Err(invalid_data("input control has not started"));
        }
        if (options.keyboard && !self.capabilities.keyboard)
            || (options.pointer && !self.capabilities.pointer)
        {
            return Err(invalid_data("对方不支持所选控制功能"));
        }
        write_message(&mut self.stream, &Message::UpdateControl { options })?;
        match super::read_with_deadline(
            &mut self.stream,
            if self.consent_wait {
                crate::consent::WAIT_TIMEOUT
            } else {
                CONTROL_TIMEOUT
            },
        )? {
            Some(Message::ControlUpdated { options: applied }) if applied == options => Ok(()),
            Some(Message::Error { message }) => {
                Err(invalid_data(format!("control update denied: {message}")))
            }
            _ => Err(invalid_data(
                "missing or mismatched control update acknowledgement",
            )),
        }
    }
    pub fn send_key(&mut self, event: KeyEvent) -> io::Result<()> {
        if !self.controlling {
            return Err(invalid_data("keyboard control has not started"));
        }
        write_message(
            &mut self.stream,
            &Message::Keyboard {
                key: event.key,
                pressed: event.pressed,
                repeat: event.repeat,
            },
        )
    }

    pub fn send_pointer(&mut self, event: PointerEvent) -> io::Result<()> {
        if !self.controlling {
            return Err(invalid_data("input control has not started"));
        }
        event.validate()?;
        write_message(&mut self.stream, &Message::Pointer { event })
    }

    pub fn stop_control(&mut self) -> io::Result<()> {
        if !self.controlling {
            return Ok(());
        }
        self.controlling = false;
        write_message(&mut self.stream, &Message::StopControl {})?;
        match super::read_with_deadline(&mut self.stream, CONTROL_TIMEOUT)? {
            Some(Message::ControlStopped {}) => Ok(()),
            _ => Err(invalid_data("missing stop-control acknowledgement")),
        }
    }

    pub fn disconnect(mut self) -> io::Result<()> {
        write_message(
            &mut self.stream,
            &Message::Goodbye {
                reason: "client finished".to_owned(),
            },
        )?;
        self.stream.shutdown(Shutdown::Both)
    }
}
