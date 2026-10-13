use super::{
    keyboard::{Key, KeyEvent},
    pointer::{Button, PointerEvent},
};
use std::{collections::BTreeSet, io};

pub trait InputSink: Send {
    fn send(&mut self, key: Key, pressed: bool) -> io::Result<()>;
    fn pointer(&mut self, _event: PointerEvent) -> io::Result<()> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "pointer sink unavailable",
        ))
    }
}

/// Releases only keys owned by this session. The lease must outlive this object.
pub struct InputSession {
    sink: Box<dyn InputSink>,
    pressed: BTreeSet<Key>,
    buttons: BTreeSet<Button>,
}
impl InputSession {
    pub fn new(sink: Box<dyn InputSink>) -> Self {
        Self {
            sink,
            pressed: BTreeSet::new(),
            buttons: BTreeSet::new(),
        }
    }
    pub fn apply(&mut self, event: KeyEvent) -> io::Result<()> {
        let held = self.pressed.contains(&event.key);
        if event.repeat && (!event.pressed || !held || event.key.is_modifier()) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "invalid key repeat",
            ));
        }
        if event.pressed {
            if held && !event.repeat {
                return Ok(());
            }
            self.sink.send(event.key, true)?;
            self.pressed.insert(event.key);
        } else if held {
            self.sink.send(event.key, false)?;
            self.pressed.remove(&event.key);
        }
        Ok(())
    }
    pub fn apply_pointer(&mut self, event: PointerEvent) -> io::Result<()> {
        event.validate()?;
        if let PointerEvent::Button { button, pressed } = event {
            if self.buttons.contains(&button) == pressed {
                return Ok(());
            }
            self.sink.pointer(event)?;
            if pressed {
                self.buttons.insert(button);
            } else {
                self.buttons.remove(&button);
            }
            Ok(())
        } else {
            self.sink.pointer(event)
        }
    }
    pub fn release_all(&mut self) -> io::Result<()> {
        let mut error = None;
        // End drags before releasing any keyboard modifiers.
        for button in self.buttons.clone() {
            match self.sink.pointer(PointerEvent::Button {
                button,
                pressed: false,
            }) {
                Ok(()) => {
                    self.buttons.remove(&button);
                }
                Err(e) => error = Some(e),
            }
        }
        // Release ordinary keys before modifiers.
        let mut keys: Vec<_> = self.pressed.iter().copied().collect();
        keys.sort_by_key(|key| key.is_modifier());
        for key in keys {
            match self.sink.send(key, false) {
                Ok(()) => {
                    self.pressed.remove(&key);
                }
                Err(e) => error = Some(e),
            }
        }
        match error {
            Some(e) => Err(e),
            None => Ok(()),
        }
    }
}
impl Drop for InputSession {
    fn drop(&mut self) {
        if let Err(e) = self.release_all() {
            eprintln!("Could not release all remote keys/buttons: {e}");
        }
    }
}
