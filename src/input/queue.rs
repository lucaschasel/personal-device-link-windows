use super::{keyboard::KeyEvent, pointer::PointerEvent};
use std::{
    collections::VecDeque,
    io,
    sync::{Condvar, Mutex},
    time::Duration,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputEvent {
    Key(KeyEvent),
    Pointer(PointerEvent),
}

/// A single ordered bounded queue for keyboard, movement, buttons and scrolling.
/// The lock only guards O(1) queue operations, never network or OS input calls.
/// Overflow stops the entire session instead of losing a button/key release.
pub struct InputQueue {
    events: Mutex<VecDeque<InputEvent>>,
    ready: Condvar,
    capacity: usize,
}
impl InputQueue {
    pub fn new(capacity: usize) -> Self {
        Self {
            events: Mutex::new(VecDeque::with_capacity(capacity)),
            ready: Condvar::new(),
            capacity,
        }
    }
    pub fn push(&self, event: InputEvent) -> io::Result<()> {
        let mut events = self
            .events
            .lock()
            .map_err(|_| io::Error::other("input queue poisoned"))?;
        if let (Some(InputEvent::Pointer(previous)), InputEvent::Pointer(next)) =
            (events.back_mut(), event)
        {
            if previous.merge(next) {
                return Ok(());
            }
        }
        if events.len() >= self.capacity {
            return Err(io::Error::other("input queue full"));
        }
        events.push_back(event);
        self.ready.notify_one();
        Ok(())
    }
    pub fn receive(&self, timeout: Duration) -> io::Result<Option<InputEvent>> {
        let events = self
            .events
            .lock()
            .map_err(|_| io::Error::other("input queue poisoned"))?;
        let (mut events, _) = self
            .ready
            .wait_timeout_while(events, timeout, |q| q.is_empty())
            .map_err(|_| io::Error::other("input queue poisoned"))?;
        Ok(events.pop_front())
    }
}
