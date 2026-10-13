//! Low-level hooks live on a dedicated message-loop thread. They only update
//! local state and the bounded queue; socket I/O never runs in a hook callback.
use crate::input::{
    keyboard::WindowsKeyboard,
    options::{CaptureConfig, INJECTED_TAG},
    pointer::{Button, PointerEvent},
    queue::{InputEvent, InputQueue},
};
use std::{
    cell::RefCell,
    collections::BTreeSet,
    io, ptr,
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Arc,
    },
    thread,
    time::Duration,
};
use windows_sys::Win32::{
    Foundation::{LPARAM, LRESULT, POINT, WPARAM},
    System::LibraryLoader::GetModuleHandleW,
    UI::{Input::KeyboardAndMouse::GetAsyncKeyState, WindowsAndMessaging::*},
};

struct Context {
    config: CaptureConfig,
    queue: Arc<InputQueue>,
    running: Arc<AtomicBool>,
    keyboard: WindowsKeyboard,
    physical: BTreeSet<(u16, bool)>,
    local: bool,
    local_buttons: BTreeSet<Button>,
    remote_buttons: BTreeSet<Button>,
    anchor: Option<POINT>,
    ignored: BTreeSet<u32>,
    failed: bool,
}
thread_local! { static CONTEXT: RefCell<Option<Context>> = const { RefCell::new(None) }; }
impl Context {
    fn push(&mut self, event: InputEvent) {
        if self.queue.push(event).is_err() {
            self.failed = true;
            self.running.store(false, Ordering::Release);
        }
    }
    fn chord(&self) -> bool {
        self.physical.iter().any(|(code, _)| *code == 0x1d)
            && self.physical.iter().any(|(code, _)| *code == 0x38)
    }
    fn update_local(&mut self) {
        let local = self.config.options.pointer
            && (self.chord() || (self.local && !self.local_buttons.is_empty()));
        if local != self.local {
            self.local = local;
            self.anchor = None;
            if local {
                for button in std::mem::take(&mut self.remote_buttons) {
                    self.push(InputEvent::Pointer(PointerEvent::Button {
                        button,
                        pressed: false,
                    }));
                }
            }
        }
    }
}

unsafe extern "system" fn keyboard_hook(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code < 0 {
        return unsafe { CallNextHookEx(ptr::null_mut(), code, wparam, lparam) };
    }
    // The OS guarantees the hook structure is live for the duration of this call.
    let input = unsafe { &*(lparam as *const KBDLLHOOKSTRUCT) };
    if input.flags & LLKHF_INJECTED != 0 || input.dwExtraInfo == INJECTED_TAG as usize {
        return unsafe { CallNextHookEx(ptr::null_mut(), code, wparam, lparam) };
    }
    let consumed = CONTEXT.with(|slot| {
        let mut slot = slot.borrow_mut();
        let Some(state) = slot.as_mut() else {
            return false;
        };
        if !state.running.load(Ordering::Acquire) {
            return false;
        }
        let pressed = matches!(wparam as u32, WM_KEYDOWN | WM_SYSKEYDOWN);
        let scan = input.scanCode as u16;
        let extended = input.flags & LLKHF_EXTENDED != 0;
        if state.ignored.contains(&input.vkCode) {
            if !pressed {
                state.ignored.remove(&input.vkCode);
            }
            return false;
        }
        if pressed {
            state.physical.insert((scan, extended));
        } else {
            state.physical.remove(&(scan, extended));
        }
        if pressed && scan == 1 && state.chord() {
            state.running.store(false, Ordering::Release);
            return true;
        }
        state.update_local();
        if !state.config.options.keyboard {
            return false;
        }
        if let Some(key) = state.keyboard.event(
            scan,
            extended,
            pressed,
            &state.config.target_os,
            state.config.options.shortcut_mapping,
        ) {
            state.push(InputEvent::Key(key));
        }
        true
    });
    if consumed {
        1
    } else {
        unsafe { CallNextHookEx(ptr::null_mut(), code, wparam, lparam) }
    }
}

unsafe extern "system" fn mouse_hook(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code < 0 {
        return unsafe { CallNextHookEx(ptr::null_mut(), code, wparam, lparam) };
    }
    let input = unsafe { &*(lparam as *const MSLLHOOKSTRUCT) };
    if input.flags & LLMHF_INJECTED != 0 || input.dwExtraInfo == INJECTED_TAG as usize {
        return unsafe { CallNextHookEx(ptr::null_mut(), code, wparam, lparam) };
    }
    let consumed = CONTEXT.with(|slot| {
        let mut slot = slot.borrow_mut();
        let Some(state) = slot.as_mut() else {
            return false;
        };
        if !state.running.load(Ordering::Acquire) || !state.config.options.pointer {
            return false;
        }
        let kind = wparam as u32;
        // Let the message-loop's anchor warp through even on systems that do
        // not label SetCursorPos as injected. It carries no remote movement.
        if kind == WM_MOUSEMOVE
            && state
                .anchor
                .is_some_and(|p| p.x == input.pt.x && p.y == input.pt.y)
        {
            return false;
        }
        let button = match kind {
            WM_LBUTTONDOWN => Some((Button::Left, true)),
            WM_LBUTTONUP => Some((Button::Left, false)),
            WM_RBUTTONDOWN => Some((Button::Right, true)),
            WM_RBUTTONUP => Some((Button::Right, false)),
            WM_MBUTTONDOWN => Some((Button::Middle, true)),
            WM_MBUTTONUP => Some((Button::Middle, false)),
            _ => None,
        };
        if state.local {
            if let Some((button, down)) = button {
                if down {
                    state.local_buttons.insert(button);
                } else {
                    state.local_buttons.remove(&button);
                }
            }
            state.update_local();
            return false; // The final local button-up always belongs to the local app.
        }
        let event = match kind {
            WM_MOUSEMOVE => state.anchor.and_then(|anchor| {
                let (dx, dy) = (input.pt.x - anchor.x, input.pt.y - anchor.y);
                (dx != 0 || dy != 0).then_some(PointerEvent::Move { dx, dy })
            }),
            WM_MOUSEWHEEL => Some(PointerEvent::Scroll {
                horizontal: 0,
                vertical: ((input.mouseData >> 16) as i16) as i32,
            }),
            WM_MOUSEHWHEEL => Some(PointerEvent::Scroll {
                horizontal: ((input.mouseData >> 16) as i16) as i32,
                vertical: 0,
            }),
            _ => button.map(|(button, pressed)| {
                if pressed {
                    state.remote_buttons.insert(button);
                } else {
                    state.remote_buttons.remove(&button);
                }
                PointerEvent::Button { button, pressed }
            }),
        };
        if let Some(event) = event {
            if event.validate().is_err() {
                state.failed = true;
                state.running.store(false, Ordering::Release);
            } else {
                state.push(InputEvent::Pointer(event));
            }
        }
        true // Suppression keeps the physical cursor at the anchor, away from edges.
    });
    if consumed {
        1
    } else {
        unsafe { CallNextHookEx(ptr::null_mut(), code, wparam, lparam) }
    }
}

struct Hooks {
    keyboard: HHOOK,
    mouse: HHOOK,
    restore: Option<POINT>,
}
impl Drop for Hooks {
    fn drop(&mut self) {
        unsafe {
            if !self.keyboard.is_null() {
                UnhookWindowsHookEx(self.keyboard);
            }
            if !self.mouse.is_null() {
                UnhookWindowsHookEx(self.mouse);
            }
            if let Some(point) = self.restore {
                SetCursorPos(point.x, point.y);
            }
        }
    }
}
pub struct Capture {
    running: Arc<AtomicBool>,
    worker: Option<thread::JoinHandle<io::Result<()>>>,
}
impl Capture {
    pub fn start(
        queue: Arc<InputQueue>,
        running: Arc<AtomicBool>,
        config: CaptureConfig,
    ) -> io::Result<Self> {
        config.options.validate()?;
        let (sender, ready) = mpsc::sync_channel(1);
        let flag = Arc::clone(&running);
        let worker = thread::Builder::new()
            .name("windows-input-capture".into())
            .spawn(move || {
                let result = run_hooks(queue, Arc::clone(&flag), config, sender);
                flag.store(false, Ordering::Release);
                result
            })?;
        match ready.recv() {
            Ok(Ok(())) => Ok(Self {
                running,
                worker: Some(worker),
            }),
            result => {
                running.store(false, Ordering::Release);
                let _ = worker.join();
                Err(io::Error::other(match result {
                    Ok(Err(e)) => e,
                    _ => "Windows 捕获线程无法启动".into(),
                }))
            }
        }
    }
    pub fn finish(mut self) -> io::Result<()> {
        self.running.store(false, Ordering::Release);
        self.worker
            .take()
            .unwrap()
            .join()
            .map_err(|_| io::Error::other("Windows 捕获线程异常退出"))?
    }
}
impl Drop for Capture {
    fn drop(&mut self) {
        self.running.store(false, Ordering::Release);
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}
fn run_hooks(
    queue: Arc<InputQueue>,
    running: Arc<AtomicBool>,
    config: CaptureConfig,
    ready: mpsc::SyncSender<Result<(), String>>,
) -> io::Result<()> {
    let startup = (|| -> io::Result<Hooks> {
        if !running.load(Ordering::Acquire) {
            return Err(io::Error::other("已取消控制"));
        }
        // Poll only before hooks start. Hook callbacks track key transitions themselves.
        if [0x10, 0x11, 0x12, 0x5b, 0x5c, 1, 2, 4]
            .iter()
            .any(|vk| unsafe { GetAsyncKeyState(*vk) } < 0)
        {
            return Err(io::Error::other(
                "请松开 Shift、Ctrl、Alt、Win 与鼠标按钮后重新开始",
            ));
        }
        let mut position = POINT::default();
        if config.options.pointer && unsafe { GetCursorPos(&mut position) } == 0 {
            return Err(io::Error::last_os_error());
        }
        let ignored = (0..256)
            .filter(|vk| unsafe { GetAsyncKeyState(*vk) } < 0)
            .map(|vk| vk as u32)
            .collect();
        CONTEXT.with(|slot| {
            *slot.borrow_mut() = Some(Context {
                config: config.clone(),
                queue,
                running: Arc::clone(&running),
                keyboard: WindowsKeyboard::default(),
                physical: BTreeSet::new(),
                local: false,
                local_buttons: BTreeSet::new(),
                remote_buttons: BTreeSet::new(),
                anchor: None,
                ignored,
                failed: false,
            })
        });
        let mut hooks = Hooks {
            keyboard: ptr::null_mut(),
            mouse: ptr::null_mut(),
            restore: config.options.pointer.then_some(position),
        };
        unsafe {
            let module = GetModuleHandleW(ptr::null());
            hooks.keyboard = SetWindowsHookExW(WH_KEYBOARD_LL, Some(keyboard_hook), module, 0);
            if hooks.keyboard.is_null() {
                return Err(io::Error::last_os_error());
            }
            if config.options.pointer {
                hooks.mouse = SetWindowsHookExW(WH_MOUSE_LL, Some(mouse_hook), module, 0);
                if hooks.mouse.is_null() {
                    return Err(io::Error::last_os_error());
                }
            }
        }
        Ok(hooks)
    })();
    let hooks = match startup {
        Ok(hooks) => {
            let _ = ready.send(Ok(()));
            hooks
        }
        Err(e) => {
            let _ = ready.send(Err(e.to_string()));
            CONTEXT.with(|slot| slot.borrow_mut().take());
            return Err(e);
        }
    };
    let mut message = MSG::default();
    while running.load(Ordering::Acquire) {
        // SetCursorPos can trigger callbacks, so perform it outside the RefCell borrow.
        let anchor = CONTEXT.with(|slot| {
            let mut slot = slot.borrow_mut();
            let state = slot.as_mut().unwrap();
            if state.config.options.pointer && !state.local && state.anchor.is_none() {
                let point = POINT {
                    x: unsafe { GetSystemMetrics(SM_CXSCREEN) } / 2,
                    y: unsafe { GetSystemMetrics(SM_CYSCREEN) } / 2,
                };
                state.anchor = Some(point);
                Some(point)
            } else {
                None
            }
        });
        if let Some(point) = anchor {
            if unsafe { SetCursorPos(point.x, point.y) } == 0 {
                CONTEXT.with(|slot| {
                    if let Some(state) = slot.borrow_mut().as_mut() {
                        state.failed = true;
                    }
                });
                running.store(false, Ordering::Release);
            }
        }
        unsafe {
            while PeekMessageW(&mut message, ptr::null_mut(), 0, 0, PM_REMOVE) != 0 {
                if message.message == WM_QUIT {
                    running.store(false, Ordering::Release);
                    break;
                }
                TranslateMessage(&message);
                DispatchMessageW(&message);
            }
        }
        thread::sleep(Duration::from_millis(5));
    }
    drop(hooks);
    let failed = CONTEXT.with(|slot| slot.borrow_mut().take().is_some_and(|state| state.failed));
    if failed {
        Err(io::Error::other(
            "捕获队列或鼠标事件异常，已停止并恢复本机操作",
        ))
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input::keyboard::Key;
    #[test]
    fn maps_windows_scan_codes_without_installing_hooks() {
        let mut mapper = WindowsKeyboard::default();
        assert_eq!(
            mapper.event(0x1d, false, true, "macos", true).unwrap().key,
            Key::LeftMeta
        );
        assert_eq!(
            mapper.event(0x1d, false, false, "macos", true).unwrap().key,
            Key::LeftMeta
        );
    }
}
